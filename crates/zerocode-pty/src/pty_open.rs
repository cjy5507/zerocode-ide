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
//! foreground group, the child's session and controlling terminal — is what
//! `portable_pty` does, written out for these two descriptors, because its
//! own types cannot be built from descriptors that were opened elsewhere.

use std::cell::Cell;
use std::ffi::{CStr, CString, OsStr};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt as _;
#[cfg(not(target_os = "macos"))]
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;

use portable_pty::{Child, CommandBuilder, MasterPty, PtyPair, PtySize, SlavePty};

/// A pty pair of `size` whose two ends are close-on-exec from the moment
/// they exist.
pub(crate) fn open_pair(size: PtySize) -> io::Result<PtyPair> {
    let (master, slave, name) = open_ends(size)?;
    Ok(PtyPair {
        slave: Box::new(UnixSlave { fd: slave }),
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
    #[cfg(target_os = "macos")]
    // SAFETY: `TIOCPTYGNAME` writes a NUL-terminated name of at most 128
    // bytes into the buffer it is given, which is 128 long.
    let named = unsafe { libc::ioctl(master, libc::TIOCPTYGNAME as _, buffer.as_mut_ptr()) };
    #[cfg(not(target_os = "macos"))]
    // SAFETY: `ptsname_r` writes at most `buffer.len()` bytes, NUL included.
    let named = unsafe { libc::ptsname_r(master, buffer.as_mut_ptr(), buffer.len()) };
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
    /// Held so that it closes with the value (the lane drops it once the
    /// child has the terminal); only a start that is not macOS's reads it.
    #[cfg_attr(target_os = "macos", allow(dead_code, reason = "held for its close"))]
    fd: OwnedFd,
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
        self.start(builder)
    }
}

impl UnixSlave {
    /// On macOS a lane starts its child without a fork, by the terminal's
    /// name ([`crate::posix_launch`]); nothing asks the slave to start one,
    /// and a start here would be the fork that module exists to avoid.
    #[cfg(target_os = "macos")]
    fn start(&self, _: CommandBuilder) -> Result<Box<dyn Child + Send + Sync>, anyhow::Error> {
        anyhow::bail!("a lane on macOS starts its child through posix_launch")
    }

    /// The child leads a session of its own whose controlling terminal is
    /// this slave; signals it inherited are put back to their defaults and
    /// every descriptor above 2 is closed before it runs, as
    /// `portable_pty` starts one. A builder's `umask` is not carried (its
    /// field is private, and no lane sets one), and a default program is
    /// refused (no lane asks for one).
    #[cfg(not(target_os = "macos"))]
    fn start(
        &self,
        builder: CommandBuilder,
    ) -> Result<Box<dyn Child + Send + Sync>, anyhow::Error> {
        let controlling_tty = builder.get_controlling_tty();
        let mut command = command_of(&builder)?;
        // SAFETY: the closure runs between fork and exec and calls only
        // async-signal-safe functions (`signal`, `sigprocmask`, `setsid`,
        // `ioctl`, `close`) — except `close_random_fds`, which
        // lists `/dev/fd` as `portable_pty` does.
        unsafe {
            command
                .stdin(self.fd.try_clone()?)
                .stdout(self.fd.try_clone()?)
                .stderr(self.fd.try_clone()?)
                .pre_exec(move || {
                    for signal in [
                        libc::SIGCHLD,
                        libc::SIGHUP,
                        libc::SIGINT,
                        libc::SIGQUIT,
                        libc::SIGTERM,
                        libc::SIGALRM,
                    ] {
                        libc::signal(signal, libc::SIG_DFL);
                    }
                    let empty: libc::sigset_t = std::mem::zeroed();
                    libc::sigprocmask(libc::SIG_SETMASK, &raw const empty, std::ptr::null_mut());
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if controlling_tty && libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    close_inherited();
                    Ok(())
                });
        }
        let mut child = command.spawn()?;
        child.stdin.take();
        child.stdout.take();
        child.stderr.take();
        Ok(Box::new(child))
    }
}

#[cfg(not(target_os = "macos"))]
/// The `Command` a builder describes, as `portable_pty` builds it from the
/// builder's own program, arguments, working directory and environment (its
/// own way is private to it): the program looked up on the builder's `PATH`,
/// shown to itself under the name it was given, started in the working
/// directory when that is one (else `HOME`, else `/`), with only the
/// builder's environment and a `SHELL` among it.
fn command_of(builder: &CommandBuilder) -> Result<std::process::Command, anyhow::Error> {
    anyhow::ensure!(
        !builder.is_default_prog(),
        "a default program is not started here"
    );
    let argv = builder.get_argv();
    let program = argv
        .first()
        .ok_or_else(|| anyhow::anyhow!("no program to start"))?;
    let directory = builder
        .get_cwd()
        .map(PathBuf::from)
        .filter(|directory| directory.is_dir())
        .or_else(|| builder.get_env("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut command = std::process::Command::new(program);
    command.arg0(program);
    command.args(&argv[1..]);
    command.current_dir(directory);
    command.env_clear();
    command.envs(builder.iter_full_env_as_str());
    if builder.get_env("SHELL").is_none() {
        command.env("SHELL", login_shell());
    }
    Ok(command)
}

#[cfg(not(target_os = "macos"))]
/// This user's login shell from the password database, `/bin/sh` if it
/// names none.
fn login_shell() -> String {
    // SAFETY: `getpwuid` answers null or a record that stays valid until the
    // next call of the password database on this thread, read at once below.
    let entry = unsafe { libc::getpwuid(libc::getuid()) };
    if entry.is_null() {
        return "/bin/sh".to_string();
    }
    // SAFETY: a non-null record holds a NUL-terminated shell path or null.
    let shell = unsafe { (*entry).pw_shell };
    if shell.is_null() {
        return "/bin/sh".to_string();
    }
    // SAFETY: checked non-null above; NUL-terminated by the library.
    let shell = unsafe { CStr::from_ptr(shell) };
    shell.to_string_lossy().into_owned()
}

#[cfg(not(target_os = "macos"))]
/// Close every descriptor above 2 the child was handed, as `portable_pty`
/// does for the ones a GUI process leaks.
fn close_inherited() {
    let Ok(directory) = std::fs::read_dir("/dev/fd") else {
        return;
    };
    let numbers: Vec<libc::c_int> = directory
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok()?.parse().ok())
        .filter(|number| *number > 2)
        .collect();
    for number in numbers {
        // SAFETY: closing a descriptor number the directory listed.
        unsafe { libc::close(number) };
    }
}

#[cfg(test)]
mod tests {
    use super::open_pair;
    use portable_pty::{PtyPair, PtySize};
    use std::io;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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
    /// process, so a pane's end still held after [`HELD_TOO_LONG`] is
    /// held by one that lives, and not by one still on its way to `exec`.
    const CHILD_LIFE: f64 = 1.5;
    /// Long-lived children one starter keeps at a time. Beside them it
    /// starts and waits for `true` as fast as it can, so a pane that opens
    /// has a process start going on around it nearly all the time.
    const LIVING_PER_STARTER: usize = 48;
    /// Panes held by a child that lived on, at which the run stops: one is
    /// the failure, a few say how often it happens.
    const ENOUGH_KEPT: usize = 3;
    /// A process in the middle of its start holds a copy of every descriptor
    /// of the window, close-on-exec or not, until its `exec` takes the
    /// close-on-exec ones away: a few milliseconds, longer on a loaded
    /// machine. A pane end still held after this is held by a child that
    /// went on living with it.
    const HELD_TOO_LONG: Duration = Duration::from_millis(600);
    /// The longest a stream is read for its end.
    const PATIENCE: Duration = Duration::from_secs(5);

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

    /// Children started by other threads for as long as it lives, as the
    /// window starts `git` and `ps` while a pane opens.
    struct Starters {
        stop: Arc<AtomicBool>,
        threads: Vec<std::thread::JoinHandle<()>>,
    }

    impl Starters {
        fn begin() -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let threads = (0..STARTERS)
                .map(|_| {
                    let stop = Arc::clone(&stop);
                    std::thread::spawn(move || {
                        let mut alive = Vec::new();
                        let mut started = 0u32;
                        // Lives differ by up to half a second, so the
                        // children of a starter end one by one and a new one
                        // takes each one's place, not all at once.
                        let mut quiet = || {
                            started += 1;
                            let mut command = Command::new("/bin/sleep");
                            command
                                .arg(format!(
                                    "{:.2}",
                                    CHILD_LIFE + f64::from(started % 10) * 0.05
                                ))
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null());
                            command
                        };
                        while !stop.load(Ordering::Relaxed) {
                            alive.retain_mut(|child: &mut std::process::Child| {
                                matches!(child.try_wait(), Ok(None))
                            });
                            if alive.len() < LIVING_PER_STARTER {
                                if let Ok(child) = quiet().spawn() {
                                    alive.push(child);
                                }
                            } else {
                                let _ = Command::new("/usr/bin/true")
                                    .stdin(Stdio::null())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .status();
                            }
                        }
                        for mut child in alive {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                    })
                })
                .collect();
            Self { stop, threads }
        }

        fn end(self) {
            self.stop.store(true, Ordering::Relaxed);
            for thread in self.threads {
                thread.join().expect("a starter thread");
            }
        }
    }

    /// How a pane's end was seen after its other end closed.
    #[derive(Default)]
    struct Seen {
        /// Ends still held after [`HELD_TOO_LONG`]: held by a child that lived on.
        kept: usize,
        /// Ends not hung up at once but hung up in time: a start going on.
        passing: usize,
        slowest: Duration,
        total: Duration,
    }

    impl Seen {
        /// `hung` is asked at once; a no is waited out for [`HELD_TOO_LONG`].
        fn note(&mut self, mut hung: impl FnMut() -> bool) {
            if hung() {
                return;
            }
            let begun = Instant::now();
            while !hung() {
                if begun.elapsed() >= HELD_TOO_LONG {
                    self.kept += 1;
                    return;
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            let held = begun.elapsed();
            self.passing += 1;
            self.slowest = self.slowest.max(held);
            self.total += held;
        }
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

    /// The control: with nobody holding the other end, the hangup is there
    /// once an end closes, and is not there while the end is open — so a
    /// miss under pressure means a holder, nothing else.
    #[test]
    #[ignore = "the pressure's own process, started by `the_pty_open_leaves_no_end_to_a_child_started_beside_it`"]
    fn alone_a_pane_end_hangs_up_when_the_other_closes_and_not_before() {
        let PtyPair { master, slave } = open_pair(SIZE).expect("a pty");
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

        let PtyPair { master, slave } = open_pair(SIZE).expect("a pty");
        let held = slave_by_name(master.as_ref());
        drop(slave);
        drop(master);
        assert!(
            hangs_up_within_patience(|| hung_up(held.as_raw_fd())),
            "the slave saw no hangup, no master left"
        );
    }

    /// No child another thread starts while panes open ends up holding a
    /// pane's master or slave. The red before the fix is a count of panes
    /// whose end hung up late; after it, none (1000 panes).
    #[test]
    #[ignore = "the pressure's own process, started by `the_pty_open_leaves_no_end_to_a_child_started_beside_it`"]
    fn alone_a_child_started_while_a_pane_opens_holds_none_of_its_ends() {
        let starters = Starters::begin();
        let (mut master_held, mut slave_held) = (Seen::default(), Seen::default());
        let began = Instant::now();
        let mut opened = 0;
        for round in 0..OPENS {
            if master_held.kept + slave_held.kept >= ENOUGH_KEPT {
                break;
            }
            opened += 1;
            let PtyPair { master, slave } = open_pair(SIZE).expect("a pty");
            if round % 2 == 0 {
                // Close the master; the slave (ours) should hang up.
                let held = slave_by_name(master.as_ref());
                drop(slave);
                drop(master);
                master_held.note(|| hung_up(held.as_raw_fd()));
            } else {
                // Close the slaves; the master should hang up.
                let held = slave_by_name(master.as_ref());
                drop(slave);
                drop(held);
                let fd = master.as_raw_fd().expect("a master descriptor");
                slave_held.note(|| hung_up(fd));
            }
        }
        let elapsed = began.elapsed();
        starters.end();
        eprintln!(
            "pty_open pressure: {opened} of {OPENS} panes in {elapsed:?}; master: {} held by a child that \
             lived on, {} held while a start passed (slowest {:?}, total {:?}); slave: {} held \
             by a child that lived on, {} held while a start passed (slowest {:?}, total {:?})",
            master_held.kept,
            master_held.passing,
            master_held.slowest,
            master_held.total,
            slave_held.kept,
            slave_held.passing,
            slave_held.slowest,
            slave_held.total,
        );
        assert_eq!(master_held.kept, 0, "a child holds a pane's master");
        assert_eq!(slave_held.kept, 0, "a child holds a pane's slave");
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
        let (master, slave, name) = super::open_ends(SIZE).expect("a pty");
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
        let PtyPair { master, .. } = open_pair(size).expect("a pty");
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

    /// The writer is taken once; dropping it ends the child's input.
    #[test]
    fn the_writer_is_taken_once() {
        let PtyPair { master, .. } = open_pair(SIZE).expect("a pty");
        let _writer = master.take_writer().expect("the first writer");
        assert!(master.take_writer().is_err(), "a second writer was given");
    }

    /// A child started on the slave leads a session whose controlling
    /// terminal is the pty: it is its own process group and the terminal's
    /// foreground group (`ps` pid, pgid, tpgid), `tty` names the slave, and
    /// the window size is the pane's. The reader sees the stream end when
    /// the child is gone.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_child_started_on_the_slave_leads_a_session_with_the_pty_as_its_terminal() {
        use std::io::Read as _;
        let PtyPair { master, slave } = open_pair(SIZE).expect("a pty");
        let name = master.tty_name().expect("a name");
        let mut command = portable_pty::CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(
            "echo TTY $(tty); echo IDS $(ps -o pid=,pgid=,tpgid= -p $$); echo SIZE $(stty size)",
        );
        let mut child = slave.spawn_command(command).expect("a child");
        drop(slave);
        let mut reader = master.try_clone_reader().expect("a reader");
        let mut said = String::new();
        let mut buffer = [0u8; 1024];
        let begun = Instant::now();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => said.push_str(&String::from_utf8_lossy(&buffer[..count])),
            }
            assert!(begun.elapsed() < PATIENCE * 4, "no end of stream: {said}");
        }
        child.wait().expect("the child ends");
        let line = |tag: &str| {
            said.lines()
                .find_map(|line| line.trim_end().strip_prefix(tag))
                .unwrap_or_else(|| panic!("no {tag} line in {said:?}"))
                .trim()
                .to_string()
        };
        assert_eq!(line("TTY "), name.to_string_lossy());
        let ids: Vec<&str> = line("IDS ").leak().split_whitespace().collect();
        assert_eq!(ids.len(), 3, "{ids:?}");
        assert!(
            ids[0] == ids[1] && ids[1] == ids[2],
            "not a session leader in the foreground of its terminal: {ids:?}"
        );
        assert_eq!(line("SIZE "), "24 80");
    }
}
