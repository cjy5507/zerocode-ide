//! Opening a pane's pty in one step (t-85167).
//!
//! `portable_pty`'s Unix `openpty` asks `libc::openpty` for both ends and
//! sets close-on-exec on them afterwards. A child another thread of the
//! window starts in that gap inherits the new master and slave and keeps
//! them for its whole life: the pane's shell is not hung up when the window
//! closes the pane, and the pane's end shows late or never.
//!
//! Here the ends are opened with `O_CLOEXEC` in the call that makes them
//! (`posix_openpt`, then the slave by its name), so there is no gap. What
//! the pair does afterwards — the size, the reader and writer, the
//! foreground group — is what `portable_pty` does, written out for these two
//! descriptors, because its own types cannot be built from descriptors that
//! were opened elsewhere.
//!
//! macOS only. Every other system still opens through `portable_pty`, and
//! the gap stays there: on Linux a child another thread starts while a pane
//! opens keeps the new pane's ends for its whole life. Linux is not shipped
//! (and does not build here yet); closing the gap there is a start of its
//! own, with its own fork-and-exec to write and test.
//!
//! The reader, the writer and the size calls follow `portable-pty` 0.9
//! (`src/unix.rs`, MIT licence, copyright (c) 2018 Wez Furlong).

use std::cell::Cell;
use std::ffi::{CStr, CString, OsStr};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;

use portable_pty::{Child, CommandBuilder, MasterPty, PtyPair, PtySize, SlavePty};

/// A pty pair of `size` whose two ends are close-on-exec from the moment
/// they exist.
pub(crate) fn open_pair(size: PtySize) -> io::Result<PtyPair> {
    let (master, slave, name) = open_ends(size)?;
    Ok(PtyPair {
        slave: Box::new(UnixSlave { _end: slave }),
        master: Box::new(UnixMaster {
            fd: master,
            took_writer: Cell::new(false),
            name,
        }),
    })
}

/// The two ends of a new pty and the slave's path.
fn open_ends(size: PtySize) -> io::Result<(OwnedFd, OwnedFd, PathBuf)> {
    // SAFETY: opens a new descriptor; the result is checked below.
    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
    if master < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor just opened, owned from here on, so every early
    // return below closes it.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    // SAFETY: both calls take the master descriptor opened above.
    if unsafe { libc::grantpt(master.as_raw_fd()) } != 0
        || unsafe { libc::unlockpt(master.as_raw_fd()) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let name = slave_name(master.as_raw_fd())?;
    let c_name = CString::new(name.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    // SAFETY: a path open; the result is checked below.
    let slave = unsafe {
        libc::open(
            c_name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if slave < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor just opened, owned from here on.
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    set_size(slave.as_raw_fd(), size)?;
    Ok((master, slave, name))
}

/// The path of the slave of the pty whose master is `master`, asked in the
/// way that keeps no state between threads.
fn slave_name(master: RawFd) -> io::Result<PathBuf> {
    let mut buffer = [0 as libc::c_char; 128];
    // SAFETY: `TIOCPTYGNAME` writes a NUL-terminated name of at most 128
    // bytes into the buffer it is given, which is 128 long.
    let named = unsafe { libc::ioctl(master, libc::TIOCPTYGNAME as _, buffer.as_mut_ptr()) };
    if named != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call above wrote a NUL-terminated name into `buffer`.
    let name = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    Ok(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
}

fn set_size(fd: RawFd, size: PtySize) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: size.pixel_width,
        ws_ypixel: size.pixel_height,
    };
    // SAFETY: `TIOCSWINSZ` reads one `winsize`.
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &raw const size) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn get_size(fd: RawFd) -> io::Result<PtySize> {
    // SAFETY: `TIOCGWINSZ` writes one `winsize`; zero is a valid value.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &raw mut size) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PtySize {
        rows: size.ws_row,
        cols: size.ws_col,
        pixel_width: size.ws_xpixel,
        pixel_height: size.ws_ypixel,
    })
}

/// The master end. Closes with the value.
struct UnixMaster {
    fd: OwnedFd,
    took_writer: Cell<bool>,
    name: PathBuf,
}

/// The slave end. Closes with the value.
struct UnixSlave {
    /// Held only so that it closes with the value (the lane drops it once
    /// the child has the terminal).
    _end: OwnedFd,
}

/// A copy of the master that reads: the slave closing is the end of the
/// stream, not an error.
struct Reader(std::fs::File);

impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer) {
            Err(error) if error.raw_os_error() == Some(libc::EIO) => Ok(0),
            other => other,
        }
    }
}

/// A copy of the master that writes. Dropping it tells the terminal the
/// input is over: a newline and then the terminal's own end-of-file
/// character, as `portable_pty` does.
struct Writer(std::fs::File);

impl Drop for Writer {
    fn drop(&mut self) {
        // SAFETY: `tcgetattr` fills a `termios`; zero is a valid value.
        let mut flags: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: as above, on the descriptor this value owns.
        if unsafe { libc::tcgetattr(self.0.as_raw_fd(), &raw mut flags) } == 0 {
            let eof = flags.c_cc[libc::VEOF];
            if eof != 0 {
                let _ = self.0.write_all(&[b'\n', eof]);
            }
        }
    }
}

impl Write for Writer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl MasterPty for UnixMaster {
    fn resize(&self, size: PtySize) -> Result<(), anyhow::Error> {
        set_size(self.fd.as_raw_fd(), size)
            .map_err(|error| anyhow::anyhow!("failed to ioctl(TIOCSWINSZ): {error:?}"))
    }

    fn get_size(&self) -> Result<PtySize, anyhow::Error> {
        get_size(self.fd.as_raw_fd())
            .map_err(|error| anyhow::anyhow!("failed to ioctl(TIOCGWINSZ): {error:?}"))
    }

    fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>, anyhow::Error> {
        Ok(Box::new(Reader(std::fs::File::from(self.fd.try_clone()?))))
    }

    fn take_writer(&self) -> Result<Box<dyn Write + Send>, anyhow::Error> {
        if self.took_writer.replace(true) {
            anyhow::bail!("cannot take writer more than once");
        }
        Ok(Box::new(Writer(std::fs::File::from(self.fd.try_clone()?))))
    }

    fn process_group_leader(&self) -> Option<libc::pid_t> {
        // SAFETY: reads the foreground group of the terminal behind `fd`.
        match unsafe { libc::tcgetpgrp(self.fd.as_raw_fd()) } {
            pid if pid > 0 => Some(pid),
            _ => None,
        }
    }

    fn as_raw_fd(&self) -> Option<RawFd> {
        Some(self.fd.as_raw_fd())
    }

    fn tty_name(&self) -> Option<PathBuf> {
        Some(self.name.clone())
    }
}

impl SlavePty for UnixSlave {
    fn spawn_command(
        &self,
        builder: CommandBuilder,
    ) -> Result<Box<dyn Child + Send + Sync>, anyhow::Error> {
        // A lane on macOS starts its child without a fork, by the terminal's
        // name ([`crate::posix_launch`]); nothing asks the slave to start
        // one, and a start here would be the fork that module exists to avoid.
        let _ = builder;
        anyhow::bail!("a lane on macOS starts its child through posix_launch")
    }
}

#[cfg(test)]
mod tests {
    use portable_pty::{PtyPair, PtySize, native_pty_system};
    use std::io;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, PoisonError, RwLock};
    use std::time::{Duration, Instant};

    const SIZE: PtySize = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };

    /// Panes opened under pressure.
    const OPENS: usize = 1000;
    /// Threads that start children while the panes open.
    const STARTERS: usize = 4;
    /// How long the children that stay live: well past any start of a
    /// process, so a pane's end still held once every start has returned is
    /// held by one that lives.
    const CHILD_LIFE: f64 = 1.5;
    /// Long-lived children one starter keeps at a time. Beside them it
    /// starts and waits for `true` as fast as it can, so a pane that opens
    /// has a process start going on around it nearly all the time.
    const LIVING_PER_STARTER: usize = 48;
    /// Panes held by a child that lived on, at which the run stops: one is
    /// the failure, a few say how often it happens.
    const ENOUGH_KEPT: usize = 3;
    /// The longest a stream is read for its end.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// A pair, asked for again where the kernel says another open took the
    /// pty (the same rule a lane follows): the tests of this binary and of
    /// the others open ptys at the same moment.
    fn a_pty(size: PtySize) -> PtyPair {
        open_here(size).expect("a pty")
    }

    /// The way a lane opens a pane.
    fn open_here(size: PtySize) -> io::Result<PtyPair> {
        crate::lane::ask_for_pty(|| super::open_pair(size), io::Error::raw_os_error)
    }

    /// The way before t-85167: `portable_pty`'s own `openpty`, asked up to
    /// five times, for the measurement of what the pressure sees there.
    fn open_before(size: PtySize) -> io::Result<PtyPair> {
        let mut asked = 1;
        loop {
            match native_pty_system().openpty(size) {
                Ok(pair) => return Ok(pair),
                Err(_) if asked < 5 => asked += 1,
                Err(error) => return Err(io::Error::other(error.to_string())),
            }
        }
    }

    fn hung_up(fd: RawFd) -> bool {
        let mut poll = libc::pollfd {
            fd,
            // macOS reports a hangup only to a poll that asks to read.
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one `pollfd`, a zero timeout.
        let answer = unsafe { libc::poll(&raw mut poll, 1, 0) };
        answer > 0 && poll.revents & libc::POLLHUP != 0
    }

    /// The slave of `master`'s pty, opened by name with no claim on it.
    fn slave_by_name(master: &dyn portable_pty::MasterPty) -> OwnedFd {
        let name = master.tty_name().expect("the pty has a slave name");
        let name = std::ffi::CString::new(name.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a path open; the descriptor is owned below.
        let fd = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        assert!(fd >= 0, "open the slave: {}", io::Error::last_os_error());
        // SAFETY: opened above, owned once.
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    /// Held for reading by a starter for as long as one of its `spawn`
    /// calls has not returned. A process in the middle of its start holds a
    /// copy of every descriptor of the window, close-on-exec or not, until
    /// its `exec` takes the close-on-exec ones away; `posix_spawn` returns to
    /// its caller after that `exec`, so once no `spawn` is in flight no
    /// start is left holding a copy.
    type Gate = RwLock<()>;

    /// Children started by other threads for as long as it lives, as the
    /// window starts `git` and `ps` while a pane opens. Dropping it stops the
    /// threads, so a test that panics does not leave them running under the
    /// next one.
    struct Starters {
        stop: Arc<AtomicBool>,
        gate: Arc<Gate>,
        threads: Vec<std::thread::JoinHandle<usize>>,
    }

    impl Starters {
        fn begin() -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let gate = Arc::new(Gate::new(()));
            let threads = (0..STARTERS)
                .map(|_| {
                    let stop = Arc::clone(&stop);
                    let gate = Arc::clone(&gate);
                    std::thread::spawn(move || {
                        let mut alive = Vec::new();
                        let mut started = 0usize;
                        let mut generation = 0u32;
                        // Lives differ by up to half a second, so the
                        // children of a starter end one by one and a new one
                        // takes each one's place, not all at once.
                        let mut quiet = || {
                            generation += 1;
                            let mut command = Command::new("/bin/sleep");
                            command
                                .arg(format!(
                                    "{:.2}",
                                    CHILD_LIFE + f64::from(generation % 10) * 0.05
                                ))
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null());
                            command
                        };
                        let spawn = |mut command: Command| {
                            let _in_flight = gate.read().unwrap_or_else(PoisonError::into_inner);
                            command.spawn()
                        };
                        while !stop.load(Ordering::Relaxed) {
                            alive.retain_mut(|child: &mut std::process::Child| {
                                matches!(child.try_wait(), Ok(None))
                            });
                            if alive.len() < LIVING_PER_STARTER {
                                if let Ok(child) = spawn(quiet()) {
                                    started += 1;
                                    alive.push(child);
                                }
                            } else {
                                let mut command = Command::new("/usr/bin/true");
                                command
                                    .stdin(Stdio::null())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null());
                                if let Ok(mut child) = spawn(command) {
                                    started += 1;
                                    let _ = child.wait();
                                }
                            }
                        }
                        for mut child in alive {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        started
                    })
                })
                .collect();
            Self {
                stop,
                gate,
                threads,
            }
        }

        fn gate(&self) -> &Gate {
            &self.gate
        }

        /// Stops the threads; how many children they started.
        fn end(mut self) -> usize {
            self.stop.store(true, Ordering::Relaxed);
            self.threads
                .drain(..)
                .map(|thread| thread.join().expect("a starter thread"))
                .sum()
        }
    }

    impl Drop for Starters {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            for thread in self.threads.drain(..) {
                let _ = thread.join();
            }
        }
    }

    /// How a pane's end was seen after its other end closed.
    #[derive(Default)]
    struct Seen {
        /// Ends still not hung up once every start in flight had returned:
        /// held by a child that lived on.
        kept: usize,
        /// Ends not hung up at once but hung up when the starts in flight
        /// returned: a start going on.
        passing: usize,
        /// The longest and the whole time spent waiting for those starts.
        slowest: Duration,
        total: Duration,
    }

    impl Seen {
        /// `hung` is asked at once. A no waits for the starts in flight —
        /// it takes the gate for writing, which every `spawn` in flight
        /// holds for reading and which keeps new ones out — and is asked
        /// once more: still no is a holder. No clock decides.
        fn note(&mut self, gate: &Gate, mut hung: impl FnMut() -> bool) {
            if hung() {
                return;
            }
            let begun = Instant::now();
            let settled = gate.write().unwrap_or_else(PoisonError::into_inner);
            let waited = begun.elapsed();
            if hung() {
                self.passing += 1;
                self.slowest = self.slowest.max(waited);
                self.total += waited;
            } else {
                self.kept += 1;
            }
            drop(settled);
        }
    }

    /// What one run of the pressure saw.
    struct Pressure {
        opened: usize,
        elapsed: Duration,
        /// Children the starters started during the run.
        started: usize,
        open_total: Duration,
        open_slowest: Duration,
        master: Seen,
        slave: Seen,
    }

    impl Pressure {
        fn kept(&self) -> usize {
            self.master.kept + self.slave.kept
        }

        fn report(&self, way: &str) {
            let seen = |name: &str, seen: &Seen| {
                format!(
                    "{name}: {} held by a child that lived on, {} held while a start passed \
                     (slowest {:?}, total {:?})",
                    seen.kept, seen.passing, seen.slowest, seen.total
                )
            };
            eprintln!(
                "pty_open pressure ({way}): {} of {OPENS} panes in {:?}, {} children started, \
                 open time total {:?} slowest {:?}; {}; {}",
                self.opened,
                self.elapsed,
                self.started,
                self.open_total,
                self.open_slowest,
                seen("master", &self.master),
                seen("slave", &self.slave),
            );
        }
    }

    /// Opens [`OPENS`] panes with `open` while children start beside them,
    /// and sees whether each pane's end hangs up when its other end closes.
    /// Stops at `stop_at` panes held by a child that lived on.
    fn pressure(open: fn(PtySize) -> io::Result<PtyPair>, stop_at: usize) -> Pressure {
        let starters = Starters::begin();
        let mut seen = Pressure {
            opened: 0,
            elapsed: Duration::ZERO,
            started: 0,
            open_total: Duration::ZERO,
            open_slowest: Duration::ZERO,
            master: Seen::default(),
            slave: Seen::default(),
        };
        let began = Instant::now();
        for round in 0..OPENS {
            if seen.kept() >= stop_at {
                break;
            }
            seen.opened += 1;
            let opening = Instant::now();
            let PtyPair { master, slave } = open(SIZE).expect("a pty");
            let took = opening.elapsed();
            seen.open_total += took;
            seen.open_slowest = seen.open_slowest.max(took);
            if round % 2 == 0 {
                // Close the master; the slave (ours) should hang up.
                let held = slave_by_name(master.as_ref());
                drop(slave);
                drop(master);
                seen.master
                    .note(starters.gate(), || hung_up(held.as_raw_fd()));
            } else {
                // Close the slaves; the master should hang up.
                let held = slave_by_name(master.as_ref());
                drop(slave);
                drop(held);
                let fd = master.as_raw_fd().expect("a master descriptor");
                seen.slave.note(starters.gate(), || hung_up(fd));
            }
        }
        seen.elapsed = began.elapsed();
        seen.started = starters.end();
        seen
    }

    /// Whether `hung` answers yes within [`PATIENCE`]: a hangup already
    /// there passes at once, the patience is spent only by a failure.
    fn hangs_up_within_patience(mut hung: impl FnMut() -> bool) -> bool {
        let begun = Instant::now();
        while !hung() {
            if begun.elapsed() >= PATIENCE {
                return false;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        true
    }

    /// A hangup that comes only when a start in flight returns is a start
    /// passing, not a holder — however long the start takes on a loaded
    /// machine (t-84451 saw a process stall for seconds before its `exec`).
    /// Here the start takes 900 ms.
    #[test]
    fn a_hangup_that_waits_for_a_start_in_flight_is_not_a_holder() {
        let gate = Arc::new(Gate::new(()));
        let hung = Arc::new(AtomicBool::new(false));
        let (entered, start_entered) = std::sync::mpsc::channel();
        let start = {
            let (gate, hung) = (Arc::clone(&gate), Arc::clone(&hung));
            std::thread::spawn(move || {
                let _in_flight = gate.read().unwrap();
                entered.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(900));
                hung.store(true, Ordering::SeqCst);
            })
        };
        start_entered.recv().unwrap();
        let mut seen = Seen::default();
        seen.note(&gate, || hung.load(Ordering::SeqCst));
        start.join().unwrap();
        assert_eq!(seen.kept, 0, "a start still in flight was counted a holder");
        assert_eq!(seen.passing, 1);
        assert!(
            seen.slowest >= Duration::from_millis(800),
            "{:?}",
            seen.slowest
        );
    }

    /// The other side of it: no start in flight and no hangup is a holder,
    /// at once.
    #[test]
    fn no_hangup_with_no_start_in_flight_is_a_holder() {
        let gate = Gate::new(());
        let mut seen = Seen::default();
        let begun = Instant::now();
        seen.note(&gate, || false);
        assert_eq!((seen.kept, seen.passing), (1, 0));
        assert!(begun.elapsed() < Duration::from_millis(500));
    }

    /// The pressure's starters stop when they are dropped, and an
    /// early return or a panic in the test leaves none behind.
    #[test]
    fn starters_stop_when_dropped() {
        let starters = Starters::begin();
        let stop = Arc::clone(&starters.stop);
        drop(starters);
        assert!(stop.load(Ordering::Relaxed));
        // Every thread was joined: only this test's own `Arc` is left.
        assert_eq!(Arc::strong_count(&stop), 1);
    }

    /// The control: with nobody holding the other end, the hangup is there
    /// once an end closes, and is not there while the end is open — so a
    /// miss under pressure means a holder, nothing else.
    #[test]
    #[ignore = "the pressure's own process, started by `the_pty_open_leaves_no_end_to_a_child_started_beside_it`"]
    fn alone_a_pane_end_hangs_up_when_the_other_closes_and_not_before() {
        let PtyPair { master, slave } = a_pty(SIZE);
        let held = slave_by_name(master.as_ref());
        drop(slave);
        let master_fd = master.as_raw_fd().expect("a master descriptor");
        assert!(!hung_up(held.as_raw_fd()), "hung up with the master open");
        assert!(!hung_up(master_fd), "hung up with a slave open");
        drop(held);
        assert!(
            hangs_up_within_patience(|| hung_up(master_fd)),
            "the master saw no hangup, no slave left"
        );

        let PtyPair { master, slave } = a_pty(SIZE);
        let held = slave_by_name(master.as_ref());
        drop(slave);
        drop(master);
        assert!(
            hangs_up_within_patience(|| hung_up(held.as_raw_fd())),
            "the slave saw no hangup, no master left"
        );
    }

    /// No child another thread starts while panes open ends up holding a
    /// pane's master or slave: once the starts in flight have returned, every
    /// pane's end has hung up (1000 panes), and children were in fact being
    /// started.
    #[test]
    #[ignore = "the pressure's own process, started by `the_pty_open_leaves_no_end_to_a_child_started_beside_it`"]
    fn alone_a_child_started_while_a_pane_opens_holds_none_of_its_ends() {
        let seen = pressure(open_here, ENOUGH_KEPT);
        seen.report("pty_open");
        assert!(seen.started > 0, "no child was started: nothing was tested");
        assert_eq!(seen.master.kept, 0, "a child holds a pane's master");
        assert_eq!(seen.slave.kept, 0, "a child holds a pane's slave");
    }

    /// The same pressure against the way before t-85167 (`portable_pty`'s
    /// own `openpty`) catches panes held by a child that lived on. Run by
    /// hand (`--ignored --exact`), not in the gate: it is a measurement of a
    /// race, and that it caught nothing in one run says little.
    #[test]
    #[ignore = "a measurement of the way before t-85167, run by hand"]
    fn alone_the_way_before_is_caught_by_the_same_pressure() {
        let seen = pressure(open_before, usize::MAX);
        seen.report("portable_pty");
        assert!(seen.started > 0, "no child was started: nothing was tested");
        assert!(
            seen.kept() > 0,
            "the pressure caught nothing in {} panes",
            seen.opened
        );
    }

    /// The pressure's tests, each in a process of its own with the pane
    /// opens: a child another test of this binary starts holds a copy of
    /// every descriptor until its `exec`, and one that stalls there for long
    /// would read as a holder this pty open left. Same shape as
    /// `pane_leftovers`' held-back hangup (t-84451).
    const ALONE: [&str; 2] = [
        "pty_open::tests::alone_a_pane_end_hangs_up_when_the_other_closes_and_not_before",
        "pty_open::tests::alone_a_child_started_while_a_pane_opens_holds_none_of_its_ends",
    ];

    #[test]
    fn the_pty_open_leaves_no_end_to_a_child_started_beside_it() {
        let exe = std::env::current_exe().expect("this test binary");
        let alone = Command::new(exe)
            .args([
                "--exact",
                ALONE[0],
                ALONE[1],
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .output()
            .expect("the pressure ran in a process of its own");
        let said = String::from_utf8_lossy(&alone.stdout);
        eprintln!("{said}");
        assert!(
            alone.status.success() && said.contains("test result: ok. 2 passed"),
            "the pressure failed in its own process: {said}{}",
            String::from_utf8_lossy(&alone.stderr)
        );
    }

    fn close_on_exec(fd: RawFd) -> bool {
        // SAFETY: reads the flags of a descriptor the caller holds.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        flags >= 0 && flags & libc::FD_CLOEXEC != 0
    }

    /// Both ends carry close-on-exec as `posix_openpt` and the slave's
    /// `open` returned them, read back with `fcntl`: the flag is set by the
    /// opening call itself on this system, not by a call after it.
    #[test]
    fn both_ends_are_close_on_exec_as_opened() {
        let (master, slave, name) =
            crate::lane::ask_for_pty(|| super::open_ends(SIZE), io::Error::raw_os_error)
                .expect("a pty");
        assert!(close_on_exec(master.as_raw_fd()), "the master leaks");
        assert!(close_on_exec(slave.as_raw_fd()), "the slave leaks");
        assert!(name.starts_with("/dev/"), "{name:?}");
    }

    /// The pane's size is the kernel's, from the open and after a resize;
    /// the slave's name is the one the master gives out.
    #[test]
    fn the_size_is_set_and_resized_and_the_slave_has_its_name() {
        let size = PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        };
        let PtyPair { master, .. } = a_pty(size);
        let got = master.get_size().expect("a size");
        assert_eq!((got.rows, got.cols), (30, 100));
        master
            .resize(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a resize");
        let got = master.get_size().expect("a size");
        assert_eq!((got.rows, got.cols), (40, 120));
        let name = master.tty_name().expect("a name");
        assert!(name.exists(), "{name:?}");
    }

    /// The writer is taken once.
    #[test]
    fn the_writer_is_taken_once() {
        let PtyPair { master, .. } = a_pty(SIZE);
        let _writer = master.take_writer().expect("the first writer");
        assert!(master.take_writer().is_err(), "a second writer was given");
    }

    /// What `fd` has to read now: `Err` with the `errno` where there is
    /// nothing yet (the slave is opened without blocking), an empty read at
    /// the end of the input.
    fn read_now(fd: RawFd) -> io::Result<Vec<u8>> {
        let mut buffer = [0u8; 16];
        // SAFETY: reads at most the 16 bytes of the buffer.
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer[..count as usize].to_vec())
    }

    /// Dropping the writer tells the terminal the input is over: a newline,
    /// and then the terminal's own end-of-file character. The program on
    /// the slave reads the newline as a line and then the end of its input;
    /// before the drop it has read nothing.
    #[test]
    fn dropping_the_writer_ends_the_input_with_a_newline_and_then_end_of_file() {
        let PtyPair { master, slave } = a_pty(SIZE);
        let held = slave_by_name(master.as_ref());
        drop(slave);
        let writer = master.take_writer().expect("a writer");
        let nothing = read_now(held.as_raw_fd()).expect_err("nothing written yet");
        assert_eq!(nothing.raw_os_error(), Some(libc::EAGAIN));
        drop(writer);
        assert_eq!(read_now(held.as_raw_fd()).expect("a line"), b"\n");
        assert_eq!(
            read_now(held.as_raw_fd()).expect("the end of the input"),
            b"",
            "the newline was not followed by the end-of-file character"
        );
    }
}
