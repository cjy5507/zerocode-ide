//! Loopback ports for tests that need one nobody else is on.
//!
//! One source file, `#[path]`-included by every test binary that needs it, so
//! there is one way to pick a port and not one per crate (t-74864).
//!
//! A port a test frees and asks for again, or a port it wants nobody to answer
//! on, cannot be held by a socket: macOS drops the SYN sent to a bound socket
//! that does not listen (the connect times out instead of being refused,
//! measured on macOS 26), and a socket that does listen answers. So the port is
//! held by a lease instead, from two sides:
//!
//! - **Outside every dynamic range.** `bind(0)` and every outgoing connection
//!   draw from the OS's dynamic range, so a port under it is out of their
//!   reach however long it is free. Where that range starts is asked of the OS
//!   (`/proc`, `sysctl`, `netsh`), not assumed, since an administrator can move
//!   it.
//! - **Locked among the tests.** The lease is an advisory lock on a file named
//!   for the port, taken before the port is bound and kept until the lease is
//!   dropped. Another test process, or another thread, asking for a port skips
//!   one whose lock is held. The OS lets go of the lock if the process dies, so
//!   nothing is left to clean up.
//!
//! A test that judges the release of a lease cannot do it in that shared
//! folder: another test may be given the freed port the next moment, and its
//! hold is not the lease's. [`QuietPort::claim_apart`] takes a folder of the
//! caller's own, which no other test names, and [`apart_slot`] a slice of a
//! band under the shared one, which no other test draws from and no other
//! such test overlaps (t-75231, t-75388).
//!
//! What stays open is a program outside the tests that binds that exact port
//! inside the gap. A port taken that way fails the bind the claim tries first,
//! and the claim moves on.

#![allow(dead_code)]

use std::collections::hash_map::RandomState;
use std::fs::{File, OpenOptions, TryLockError};
use std::hash::BuildHasher;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Where the dynamic range is taken to start when the OS does not say: the
/// lowest start any platform ships (Linux 32768; macOS and Windows 49152).
const UNREPORTED_DYNAMIC_START: u16 = 32_768;
/// How many ports a claim draws from, counted down from the dynamic range.
const BAND_SPAN: u16 = 4_096;
/// Under this the ports are privileged.
const LOWEST_PORT: u16 = 1_024;
/// The folder under the temp dir that holds the lock files.
const LOCK_DIR: &str = "zerocode-test-ports";

/// A lock file that could not be opened or locked at all — which says nothing
/// about a holder — with the OS's words.
#[derive(Debug)]
pub struct LockFailure {
    path: PathBuf,
    error: io::Error,
}

impl std::fmt::Display for LockFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.error)
    }
}

/// A port a test may count on until the value is dropped.
pub struct QuietPort {
    port: u16,
    _lock: File,
}

/// The ports a claim draws from when the OS says its dynamic range starts at
/// `reported`.
pub fn band_for(reported: Option<u16>) -> Range<u16> {
    let end = reported.unwrap_or(UNREPORTED_DYNAMIC_START);
    let start = end.saturating_sub(BAND_SPAN).max(LOWEST_PORT);
    assert!(
        end > start,
        "the OS starts its dynamic range at {end}, which leaves no port under it"
    );
    start..end
}

/// How many ports lie in the band under the shared one that the tests judging a
/// release share out among them.
const APART_SPAN: u16 = 512;
/// How many ports one test judging a release has to itself: sixteen claims
/// and room for the ones another program has taken.
const APART_SLOT_SPAN: u16 = 64;
/// The folder under the temp dir that holds the locks of the apart slots.
const SLOT_DIR: &str = "zerocode-test-port-slots";
/// Test binaries may briefly hold every slot at once. Bound their wait while
/// allowing a holder to finish instead of failing at the first full scan.
const SLOT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const SLOT_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// The folder the locks of the ports that every test draws from live in.
pub fn lock_dir() -> PathBuf {
    std::env::temp_dir().join(LOCK_DIR)
}

/// Where the lock that holds `port` lives in `dir`.
pub fn lock_path_in(dir: &Path, port: u16) -> PathBuf {
    dir.join(port.to_string())
}

/// Where the lock that holds `port` lives.
pub fn lock_path(port: u16) -> PathBuf {
    lock_path_in(&lock_dir(), port)
}

/// The ports under [`band_for`]'s that no ordinary claim draws from, which
/// [`apart_slot`] shares out among the tests that judge a release.
fn apart_region(reported: Option<u16>) -> Range<u16> {
    let shared = band_for(reported);
    let start = shared.start.saturating_sub(APART_SPAN).max(LOWEST_PORT);
    assert!(
        shared.start - start >= APART_SPAN / 2,
        "no room under the shared band {shared:?} for a band of its own"
    );
    start..shared.start
}

/// A band of ports that one test, in this process or another, has to itself
/// for as long as the value lives.
pub struct ApartSlot {
    band: Range<u16>,
    _lock: File,
}

impl ApartSlot {
    pub fn band(&self) -> &Range<u16> {
        &self.band
    }
}

/// A slice of the band just under [`band_for`]'s that no ordinary claim draws
/// from and no other live slot overlaps, for a test that lets a lease go and
/// asks whether it did. Two such tests share no lock folder, so the lock
/// folder cannot keep them from being given each other's freed port; what
/// does is that their bands never meet. A slot is held by a lock on a file in
/// a folder every test names, and the OS lets go of it if the process dies.
pub fn apart_slot(reported: Option<u16>) -> ApartSlot {
    apart_slot_in(
        &std::env::temp_dir().join(SLOT_DIR),
        reported,
        SLOT_WAIT_TIMEOUT,
        std::thread::sleep,
    )
}

/// The same claim in a caller's lock folder, with a bounded wait. The pause
/// is supplied so the tests can observe an exhausted scan before releasing a
/// slot, without guessing when the claiming thread has run.
fn apart_slot_in(
    dir: &Path,
    reported: Option<u16>,
    timeout: Duration,
    mut pause: impl FnMut(Duration),
) -> ApartSlot {
    let region = apart_region(reported);
    let slots = (region.end - region.start) / APART_SLOT_SPAN;
    let waiting = Instant::now();
    loop {
        let mut failures = Vec::new();
        for slot in 0..slots {
            match lock_port(dir, slot) {
                Ok(Some(lock)) => {
                    let start = region.start + slot * APART_SLOT_SPAN;
                    return ApartSlot {
                        band: start..start + APART_SLOT_SPAN,
                        _lock: lock,
                    };
                }
                Ok(None) => {}
                Err(failure) => failures.push(failure),
            }
        }
        let remaining = timeout.saturating_sub(waiting.elapsed());
        assert!(
            !remaining.is_zero(),
            "timed out after {timeout:?}: {}; slot locks: {}",
            untaken_bands(slots, &region, &failures),
            dir.display()
        );
        pause(SLOT_RETRY_INTERVAL.min(remaining));
    }
}

/// Why a scan took none of the `slots` bands of `region`: every one held, or
/// how many of them could not be locked at all and what the first said.
fn untaken_bands(slots: u16, region: &Range<u16>, failures: &[LockFailure]) -> String {
    match failures {
        [] => format!("all {slots} bands of {region:?} are held by tests judging a release"),
        [first, ..] => format!(
            "of the {slots} bands of {region:?}, {} are held by tests judging a release and {} could not be locked (the first {first})",
            usize::from(slots).saturating_sub(failures.len()),
            failures.len()
        ),
    }
}

/// Where the OS says its dynamic range starts, when it says.
pub fn os_dynamic_start() -> Option<u16> {
    static START: OnceLock<Option<u16>> = OnceLock::new();
    *START.get_or_init(read_dynamic_start)
}

#[cfg(target_os = "linux")]
fn read_dynamic_start() -> Option<u16> {
    let text = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range").ok()?;
    lowest_port(&text)
}

#[cfg(target_os = "macos")]
fn read_dynamic_start() -> Option<u16> {
    let said = std::process::Command::new("/usr/sbin/sysctl")
        .args([
            "-n",
            "net.inet.ip.portrange.first",
            "net.inet.ip.portrange.hifirst",
        ])
        .output()
        .ok()?;
    lowest_port(&String::from_utf8_lossy(&said.stdout))
}

#[cfg(windows)]
fn read_dynamic_start() -> Option<u16> {
    let said = std::process::Command::new("netsh")
        .args(["int", "ipv4", "show", "dynamicport", "tcp"])
        .output()
        .ok()?;
    netsh_start(&String::from_utf8_lossy(&said.stdout))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn read_dynamic_start() -> Option<u16> {
    None
}

/// The lowest number in a `sysctl` or `/proc` answer.
pub fn lowest_port(text: &str) -> Option<u16> {
    text.split_whitespace()
        .filter_map(|word| word.parse::<u16>().ok())
        .min()
}

/// The start port in `netsh int ipv4 show dynamicport tcp` text: the first
/// line that ends in a number after a colon, whatever language the label is in.
pub fn netsh_start(text: &str) -> Option<u16> {
    text.lines()
        .filter_map(|line| line.rsplit_once(':'))
        .find_map(|(_, number)| number.trim().parse::<u16>().ok())
}

/// Takes the lock on `port`'s file in `dir`, or `None` when another holder has
/// it. A file that cannot be opened or locked at all is an error with its path,
/// never `None`: read as held, it sent a claim round its whole wait and lost
/// what the OS said (R10 P2, t-75669).
fn lock_port(dir: &Path, port: u16) -> Result<Option<File>, LockFailure> {
    let path = lock_path_in(dir, port);
    let failure = |error| LockFailure {
        path: path.clone(),
        error,
    };
    std::fs::create_dir_all(dir).map_err(failure)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(failure)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(failure(error)),
    }
}

impl QuietPort {
    /// Claims a port in this machine's quiet band that `bind` accepts, and
    /// returns what `bind` made of it beside the lease.
    pub fn claim<T>(bind: impl FnMut(u16) -> io::Result<T>) -> (Self, T) {
        Self::claim_in(&band_for(os_dynamic_start()), bind)
    }

    /// [`claim`](Self::claim) in a band of the caller's choosing.
    pub fn claim_in<T>(band: &Range<u16>, bind: impl FnMut(u16) -> io::Result<T>) -> (Self, T) {
        Self::claim_apart(&lock_dir(), band, bind)
    }

    /// [`claim_in`](Self::claim_in) with the locks kept in `dir` instead of the
    /// folder every test shares, so that only the callers who name `dir` can
    /// meet over a port.
    pub fn claim_apart<T>(
        dir: &Path,
        band: &Range<u16>,
        mut bind: impl FnMut(u16) -> io::Result<T>,
    ) -> (Self, T) {
        let span = u64::from(band.end - band.start);
        // Only spreads the processes over the band; the lock is what keeps
        // two of them apart.
        let first = RandomState::new().hash_one(std::process::id()) % span;
        let mut failures = Vec::new();
        for step in 0..span {
            let offset = (first + step) % span;
            let port = band.start + u16::try_from(offset).expect("the offset is below the span");
            let lock = match lock_port(dir, port) {
                Ok(Some(lock)) => lock,
                Ok(None) => continue,
                Err(failure) => {
                    failures.push(failure);
                    continue;
                }
            };
            if let Ok(held) = bind(port) {
                return (Self { port, _lock: lock }, held);
            }
        }
        match failures.as_slice() {
            [] => panic!("no free loopback port in {band:?}"),
            [first, ..] => panic!(
                "no free loopback port in {band:?}: {} of its lock files could not be taken (the first {first})",
                failures.len()
            ),
        }
    }

    /// A loopback port nobody listens on, so a connection to it is refused, and
    /// nobody else is given while the lease lasts.
    pub fn silent() -> Self {
        let (lease, listener) = Self::claim(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)));
        drop(listener);
        lease
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port))
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    /// These tests claim only file locks, never sockets or the shared folder.
    struct LockDir(PathBuf);

    impl LockDir {
        fn new() -> Self {
            let nonce = RandomState::new().hash_one(std::process::id());
            Self(std::env::temp_dir().join(format!("zerocode-slot-test-{nonce}")))
        }
    }

    impl Drop for LockDir {
        fn drop(&mut self) {
            // A test may have put a file where the folder goes.
            let removed = if self.0.is_dir() {
                std::fs::remove_dir_all(&self.0)
            } else {
                std::fs::remove_file(&self.0)
            };
            removed.expect("remove the test's slot lock folder");
        }
    }

    fn fill_slots(dir: &Path) -> Vec<ApartSlot> {
        (0..8)
            .map(|_| apart_slot_in(dir, None, Duration::ZERO, |_| unreachable!("a free slot")))
            .collect()
    }

    #[test]
    fn an_exhausted_slot_claim_waits_until_a_holder_releases_one() {
        let dir = LockDir::new();
        let mut held = fill_slots(&dir.0);
        let released_band = held.last().expect("eight slots").band().clone();
        let (waiting, exhausted) = sync_channel(1);
        std::thread::scope(|scope| {
            let lock_dir = &dir.0;
            let borrower = scope.spawn(move || {
                // The pause never blocks on the test: it reports the first
                // exhausted scan and then waits out its interval, so a rescan
                // that misses the freed slot scans again, and only the claim's
                // own deadline ends the wait (gate l78n3 g2 hung on a pause
                // that waited for a second release nobody would send).
                let mut reported = false;
                apart_slot_in(lock_dir, None, SLOT_WAIT_TIMEOUT, |interval| {
                    if !reported {
                        reported = true;
                        waiting.send(()).expect("report the exhausted scan");
                    }
                    std::thread::sleep(interval);
                })
            });
            exhausted
                .recv_timeout(SLOT_WAIT_TIMEOUT)
                .expect("the borrower must wait with all eight slots still held");
            drop(held.pop());
            let borrowed = borrower.join().expect("the waiting borrower gets a slot");
            assert_eq!(borrowed.band(), &released_band);
        });
    }

    #[test]
    fn an_exhausted_slot_claim_times_out_and_names_its_lock_folder() {
        let dir = LockDir::new();
        let _held = fill_slots(&dir.0);
        let panic = std::panic::catch_unwind(|| {
            apart_slot_in(&dir.0, None, Duration::from_millis(20), std::thread::sleep)
        })
        .err()
        .expect("a claim must time out while every slot stays held");
        let message = panic.downcast_ref::<String>().expect("a panic message");
        assert!(message.contains("timed out after 20ms"), "{message}");
        assert!(message.contains("all 8 bands"), "{message}");
        assert!(message.contains(&dir.0.display().to_string()), "{message}");
    }

    /// A lock that cannot be opened is held by nobody: a claim that read it
    /// as held waited its whole thirty seconds and then blamed holders, the
    /// OS's own words lost (R10 P2, t-75669 — EMFILE when parallel tests had
    /// spent the file handles). A file where the lock folder goes cannot be
    /// locked in; a read-only folder does not stand in, since the durable
    /// writes put its permissions back.
    #[test]
    fn a_lock_folder_that_cannot_be_opened_is_said_with_the_os_words_not_read_as_held() {
        let dir = LockDir::new();
        std::fs::write(&dir.0, "not a folder").expect("a file where the folder goes");
        let cause = std::fs::create_dir_all(&dir.0)
            .expect_err("a file is no folder")
            .to_string();
        let panic = std::panic::catch_unwind(|| {
            apart_slot_in(&dir.0, None, Duration::from_millis(20), std::thread::sleep)
        })
        .err()
        .expect("no slot can be taken in a file");
        let message = panic.downcast_ref::<String>().expect("a panic message");
        assert!(message.contains("8 could not be locked"), "{message}");
        assert!(message.contains(&cause), "{message}");
        assert!(message.contains(&dir.0.display().to_string()), "{message}");
    }
}
