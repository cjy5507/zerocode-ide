//! Opening a pane's pty in one step (t-85167).
//!
//! `portable_pty`'s Unix `openpty` asks `libc::openpty` for both ends and
//! sets close-on-exec on them afterwards. A child another thread of the
//! window starts in that gap inherits the new master and slave and keeps
//! them for its whole life: the pane's shell is not hung up when the window
//! closes the pane, and the pane's end shows late or never.

use std::io;

use portable_pty::{PtyPair, PtySize};

/// A pty pair of `size` whose two ends are close-on-exec from the moment
/// they exist.
pub(crate) fn open_pair(size: PtySize) -> io::Result<PtyPair> {
    portable_pty::native_pty_system()
        .openpty(size)
        .map_err(|error| io::Error::other(error.to_string()))
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
    const STARTERS: usize = 6;
    /// How long each child lives: long enough to still be there when the
    /// pane's ends are looked at, short enough that the run is quick.
    const CHILD_LIFE: &str = "0.03";
    /// The longest a hangup is waited for once it did not come at once.
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
                        while !stop.load(Ordering::Relaxed) {
                            if let Ok(child) = Command::new("/bin/sleep")
                                .arg(CHILD_LIFE)
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn()
                            {
                                alive.push(child);
                            }
                            alive.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
                            if alive.len() >= 16 {
                                std::thread::sleep(Duration::from_millis(1));
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
        late: usize,
        slowest: Duration,
        total: Duration,
    }

    impl Seen {
        /// `hung` is asked once at once; a no is a holder that is not the
        /// pane, and is waited out to say how long it held.
        fn note(&mut self, mut hung: impl FnMut() -> bool) {
            if hung() {
                return;
            }
            self.late += 1;
            let begun = Instant::now();
            while !hung() && begun.elapsed() < PATIENCE {
                std::thread::sleep(Duration::from_millis(1));
            }
            let held = begun.elapsed();
            self.slowest = self.slowest.max(held);
            self.total += held;
        }
    }

    /// The control: with nobody else around the hangup is there at once
    /// when an end closes, and is not there while the end is open — so a
    /// miss under pressure means a holder, nothing else.
    #[test]
    fn a_pane_end_hangs_up_when_the_other_closes_and_not_before() {
        let PtyPair { master, slave } = open_pair(SIZE).expect("a pty");
        let held = slave_by_name(master.as_ref());
        drop(slave);
        let master_fd = master.as_raw_fd().expect("a master descriptor");
        assert!(!hung_up(held.as_raw_fd()), "hung up with the master open");
        assert!(!hung_up(master_fd), "hung up with a slave open");
        drop(held);
        assert!(
            hung_up(master_fd),
            "the master saw no hangup, no slave left"
        );

        let PtyPair { master, slave } = open_pair(SIZE).expect("a pty");
        let held = slave_by_name(master.as_ref());
        drop(slave);
        drop(master);
        assert!(
            hung_up(held.as_raw_fd()),
            "the slave saw no hangup, no master left"
        );
    }

    /// No child another thread starts while panes open ends up holding a
    /// pane's master or slave. The red before the fix is a count of panes
    /// whose end hung up late; after it, none (1000 panes).
    #[test]
    fn a_child_started_while_a_pane_opens_holds_none_of_its_ends() {
        let starters = Starters::begin();
        let (mut master_held, mut slave_held) = (Seen::default(), Seen::default());
        let began = Instant::now();
        for round in 0..OPENS {
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
            "pty_open pressure: {OPENS} panes in {elapsed:?}; master held by a child: {} \
             (slowest close {:?}, total {:?}); slave held by a child: {} (slowest close {:?}, \
             total {:?})",
            master_held.late,
            master_held.slowest,
            master_held.total,
            slave_held.late,
            slave_held.slowest,
            slave_held.total,
        );
        assert_eq!(master_held.late, 0, "a child holds a pane's master");
        assert_eq!(slave_held.late, 0, "a child holds a pane's slave");
    }
}
