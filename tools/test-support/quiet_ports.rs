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
use std::fs::{File, OpenOptions};
use std::hash::BuildHasher;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the dynamic range is taken to start when the OS does not say: the
/// lowest start any platform ships (Linux 32768; macOS and Windows 49152).
const UNREPORTED_DYNAMIC_START: u16 = 32_768;
/// How many ports a claim draws from, counted down from the dynamic range.
const BAND_SPAN: u16 = 4_096;
/// Under this the ports are privileged.
const LOWEST_PORT: u16 = 1_024;
/// The folder under the temp dir that holds the lock files.
const LOCK_DIR: &str = "zerocode-test-ports";

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
    let region = apart_region(reported);
    let slots = (region.end - region.start) / APART_SLOT_SPAN;
    let dir = std::env::temp_dir().join(SLOT_DIR);
    for slot in 0..slots {
        if let Some(lock) = lock_port(&dir, slot) {
            let start = region.start + slot * APART_SLOT_SPAN;
            return ApartSlot {
                band: start..start + APART_SLOT_SPAN,
                _lock: lock,
            };
        }
    }
    panic!("all {slots} bands of {region:?} are held by tests judging a release");
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
/// it.
fn lock_port(dir: &Path, port: u16) -> Option<File> {
    let path = lock_path_in(dir, port);
    std::fs::create_dir_all(path.parent()?).ok()?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
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
        for step in 0..span {
            let offset = (first + step) % span;
            let port = band.start + u16::try_from(offset).expect("the offset is below the span");
            let Some(lock) = lock_port(dir, port) else {
                continue;
            };
            if let Ok(held) = bind(port) {
                return (Self { port, _lock: lock }, held);
            }
        }
        panic!("no free loopback port in {band:?}");
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
