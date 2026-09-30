//! Waiting for a pane child without asking it (t-17057).
//!
//! A parent that waited for a pane child used to look every 250 ms: two result
//! files opened, and `tmux list-panes` spawned to learn the pane was still
//! there. That is four process spawns a second for each child for as long as it
//! worked (12 `__posix_spawn` samples in 3 s, t-11961 cause 5), and on a window
//! whose tmux is a shim it is four round trips to the window's own thread too.
//!
//! Both things the parent looks for arrive by themselves:
//!
//! - **An answer** lands as a rename into the child's directory
//!   (`write_atomic`), which a kqueue watch on that directory reports as
//!   `NOTE_WRITE` within milliseconds.
//! - **A death** takes the child's sockets with it. The parent keeps one silent
//!   connection to the child's events channel — it sends nothing and the
//!   channel sends nothing unasked (frames flow only after `session.subscribe`)
//!   — and the connection ending is the child's process ending. Nothing has to
//!   spawn to learn it.
//!
//! What only time changes — the parent's cancel flag, the budget, the child's
//! transcript — is looked at once a second by the wait itself; this module
//! only says what woke it.
//!
//! macOS only: everywhere else [`ChildWatch::open`] answers `None` and the wait
//! goes on as it did (a 250 ms look), except that tmux is asked every few
//! seconds instead of at every look.

use std::net::SocketAddr;
use std::path::Path;

/// What ended a rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Woken {
    /// The child's directory changed: an answer, its channel file or its
    /// transcript's name may have landed.
    pub directory: bool,
    /// The connection to the child's channel ended: its process is gone.
    pub channel_closed: bool,
}

impl Woken {
    /// A rest that cannot say what happened — no watch, or a watch that failed:
    /// look at everything, as a wait without one always did.
    pub const UNKNOWN: Self = Self {
        directory: true,
        channel_closed: false,
    };

    /// The interval ran out and nothing happened.
    pub const NOTHING: Self = Self {
        directory: false,
        channel_closed: false,
    };
}

pub use imp::ChildWatch;

#[cfg(target_os = "macos")]
mod imp {
    use std::io::Read as _;
    use std::net::{SocketAddr, TcpStream};
    use std::os::fd::{AsRawFd as _, RawFd};
    use std::path::Path;
    use std::time::Duration;

    use nix::libc::{c_long, time_t, timespec};
    use nix::sys::event::{EventFilter, EventFlag, FilterFlag, KEvent, Kqueue};

    use super::Woken;

    /// Loopback answers at once or not at all; a channel that does not accept
    /// within this is not held.
    const CONNECT_BUDGET: Duration = Duration::from_millis(250);

    /// One kqueue over a child's directory and, once the child has a channel,
    /// one silent connection to it.
    pub struct ChildWatch {
        queue: Kqueue,
        directory: std::fs::File,
        channel: Option<HeldChannel>,
    }

    struct HeldChannel {
        stream: TcpStream,
        addr: SocketAddr,
    }

    /// A kqueue identifies a descriptor by its number, as an unsigned word.
    fn ident(fd: RawFd) -> usize {
        usize::try_from(fd).unwrap_or(usize::MAX)
    }

    fn blank() -> KEvent {
        KEvent::new(0, EventFilter::EVFILT_READ, EventFlag::empty(), FilterFlag::empty(), 0, 0)
    }

    fn timespec_of(wait: Duration) -> timespec {
        timespec {
            tv_sec: time_t::try_from(wait.as_secs()).unwrap_or(time_t::MAX),
            tv_nsec: c_long::from(wait.subsec_nanos()),
        }
    }

    impl ChildWatch {
        /// Watch `directory` for entries appearing, going and being renamed
        /// into it. `None` when it cannot be watched — the caller then looks
        /// on a timer.
        #[must_use]
        pub fn open(directory: &Path) -> Option<Self> {
            let directory = std::fs::File::open(directory).ok()?;
            let queue = Kqueue::new().ok()?;
            let change = KEvent::new(
                ident(directory.as_raw_fd()),
                EventFilter::EVFILT_VNODE,
                EventFlag::EV_ADD | EventFlag::EV_CLEAR,
                FilterFlag::NOTE_WRITE
                    | FilterFlag::NOTE_EXTEND
                    | FilterFlag::NOTE_ATTRIB
                    | FilterFlag::NOTE_LINK
                    | FilterFlag::NOTE_DELETE
                    | FilterFlag::NOTE_RENAME
                    | FilterFlag::NOTE_REVOKE,
                0,
                0,
            );
            let mut none: [KEvent; 0] = [];
            queue.kevent(&[change], &mut none, Some(timespec_of(Duration::ZERO))).ok()?;
            Some(Self {
                queue,
                directory,
                channel: None,
            })
        }

        /// Whether a connection to the child's channel is held.
        #[must_use]
        pub fn holds_channel(&self) -> bool {
            self.channel.is_some()
        }

        /// Hold one silent connection to the child's channel at `addr`, so that
        /// its ending wakes the wait. The same address is already held: nothing
        /// happens. A different one — the child came up again on another port —
        /// replaces the old. `false` when nobody answers there.
        pub fn hold_channel(&mut self, addr: SocketAddr) -> bool {
            if self.channel.as_ref().is_some_and(|held| held.addr == addr) {
                return true;
            }
            self.channel = None;
            let Ok(stream) = TcpStream::connect_timeout(&addr, CONNECT_BUDGET) else {
                return false;
            };
            if stream.set_nonblocking(true).is_err() {
                return false;
            }
            let change = KEvent::new(
                ident(stream.as_raw_fd()),
                EventFilter::EVFILT_READ,
                EventFlag::EV_ADD | EventFlag::EV_CLEAR,
                FilterFlag::empty(),
                0,
                0,
            );
            let mut none: [KEvent; 0] = [];
            if self
                .queue
                .kevent(&[change], &mut none, Some(timespec_of(Duration::ZERO)))
                .is_err()
            {
                return false;
            }
            self.channel = Some(HeldChannel { stream, addr });
            true
        }

        /// Rest until something happens or `timeout` passes.
        pub fn wait(&mut self, timeout: Duration) -> Woken {
            let mut events = [blank(); 4];
            let Ok(count) = self.queue.kevent(&[], &mut events, Some(timespec_of(timeout))) else {
                // An interrupted wait says nothing about the child.
                return Woken::UNKNOWN;
            };
            let mut woken = Woken::NOTHING;
            for event in &events[..count] {
                if event.ident() == ident(self.directory.as_raw_fd()) {
                    woken.directory = true;
                } else if self
                    .channel
                    .as_ref()
                    .is_some_and(|held| event.ident() == ident(held.stream.as_raw_fd()))
                {
                    woken.channel_closed |= self.channel_ended();
                }
            }
            woken
        }

        /// Read what the connection has, which is nothing unless it ended: the
        /// channel says nothing unasked, and a word that comes anyway is drained
        /// so it cannot wake the wait again. An ended connection is dropped.
        fn channel_ended(&mut self) -> bool {
            let Some(held) = self.channel.as_mut() else {
                return false;
            };
            let mut sink = [0_u8; 256];
            let ended = loop {
                match held.stream.read(&mut sink) {
                    Ok(0) => break true,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break false,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break true,
                }
            };
            if ended {
                self.channel = None;
            }
            ended
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;

    use super::Woken;

    /// No watch on this platform: [`ChildWatch::open`] never answers one.
    pub struct ChildWatch;

    impl ChildWatch {
        #[must_use]
        pub fn open(_directory: &Path) -> Option<Self> {
            None
        }

        #[must_use]
        pub fn holds_channel(&self) -> bool {
            false
        }

        pub fn hold_channel(&mut self, _addr: SocketAddr) -> bool {
            false
        }

        pub fn wait(&mut self, timeout: Duration) -> Woken {
            std::thread::sleep(timeout);
            Woken::UNKNOWN
        }
    }
}

/// The address a child's discovery file names, if it names one that parses.
///
/// The file is three lines (`ChannelCoordinates`); only the first is wanted
/// here, and a half-written or stale one is simply not an address.
#[must_use]
pub fn channel_address(discovery: &Path) -> Option<SocketAddr> {
    super::ChannelCoordinates::read(discovery)
        .ok()?
        .addr
        .parse()
        .ok()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    use super::{ChildWatch, Woken};

    /// A rename into the directory — how every child publishes anything —
    /// wakes the watch at once, not at the next look.
    #[test]
    fn a_rename_into_the_directory_wakes_the_watch_at_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut watch = ChildWatch::open(directory.path()).expect("a directory can be watched");
        let landing = directory.path().to_path_buf();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let temporary = landing.join(".result.json.tmp");
            std::fs::write(&temporary, b"{}").expect("write");
            std::fs::rename(&temporary, landing.join("result.json")).expect("rename");
            Instant::now()
        });
        let woken = watch.wait(Duration::from_secs(5));
        let seen = Instant::now();
        let landed = writer.join().expect("writer");
        assert!(woken.directory, "the rename was not reported: {woken:?}");
        assert!(!woken.channel_closed);
        assert!(
            seen.duration_since(landed) < Duration::from_millis(250),
            "the watch woke {:?} after the file landed",
            seen.duration_since(landed)
        );
    }

    /// A quiet directory does not wake a rest, and the rest lasts what it was
    /// told to.
    #[test]
    fn a_quiet_directory_lets_the_rest_run_out() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut watch = ChildWatch::open(directory.path()).expect("watch");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        std::fs::write(elsewhere.path().join("noise"), b"x").expect("write elsewhere");
        let started = Instant::now();
        let woken = watch.wait(Duration::from_millis(300));
        assert_eq!(woken, Woken::NOTHING);
        assert!(started.elapsed() >= Duration::from_millis(280), "the rest ended early");
    }

    /// The child's process ending closes its channel's connection, and that —
    /// not a look at tmux — is what tells the parent.
    #[test]
    fn a_held_channel_ending_is_a_death_and_a_quiet_one_is_not() {
        let directory = tempfile::tempdir().expect("tempdir");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let mut watch = ChildWatch::open(directory.path()).expect("watch");
        assert!(watch.hold_channel(addr), "nobody answered on the child's own port");
        assert!(watch.holds_channel());
        let (mut peer, _) = listener.accept().expect("accept");

        // Silent, and a word sent anyway, do not read as an ending.
        assert_eq!(watch.wait(Duration::from_millis(200)), Woken::NOTHING);
        peer.write_all(b"unasked\n").expect("write");
        let spoken = watch.wait(Duration::from_millis(500));
        assert!(!spoken.channel_closed, "a word from the child read as its death: {spoken:?}");
        assert!(watch.holds_channel());

        let closing = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(peer);
            Instant::now()
        });
        let woken = watch.wait(Duration::from_secs(5));
        let seen = Instant::now();
        let closed = closing.join().expect("closer");
        assert!(woken.channel_closed, "the ending was not reported: {woken:?}");
        assert!(!watch.holds_channel(), "an ended connection was kept");
        assert!(seen.duration_since(closed) < Duration::from_millis(250));
    }

    /// The same address is one connection; another address replaces it.
    #[test]
    fn holding_the_same_channel_twice_connects_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let mut watch = ChildWatch::open(directory.path()).expect("watch");
        let addr = listener.local_addr().expect("addr");
        assert!(watch.hold_channel(addr));
        assert!(watch.hold_channel(addr));
        let mut accepted = 0;
        while listener.accept().is_ok() {
            accepted += 1;
        }
        assert_eq!(accepted, 1, "the same channel was connected to again");

        // Nobody listens on a port that was just released.
        let closed = {
            let gone = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            gone.local_addr().expect("addr")
        };
        assert!(!watch.hold_channel(closed));
        assert!(!watch.holds_channel(), "a refused channel replaced the held one with nothing held");
    }
}
