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
//! Two answers come from other threads (t-18917): a tmux ask runs on a thread
//! of its own, whose [`Waker`] ends the wait's rest when the confirmation of a
//! death is in, and that thread learns its tmux has exited from [`exited_by`] —
//! the kernel's word, not a look every few milliseconds.
//!
//! macOS only: everywhere else [`ChildWatch::open`] answers `None` and the wait
//! goes on as it did (a 250 ms look), except that tmux is asked every few
//! seconds instead of at every look; [`exited_by`] answers `None` and the ask
//! looks for the exit on a timer.

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
pub(crate) use imp::{exited_by, Waker};

#[cfg(target_os = "macos")]
mod imp {
    use std::io::Read as _;
    use std::net::{SocketAddr, TcpStream};
    use std::os::fd::{AsRawFd as _, RawFd};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use nix::errno::Errno;
    use nix::libc::{c_long, time_t, timespec};
    use nix::sys::event::{EventFilter, EventFlag, FilterFlag, KEvent, Kqueue};

    use super::Woken;

    /// Loopback answers at once or not at all; a channel that does not accept
    /// within this is not held.
    const CONNECT_BUDGET: Duration = Duration::from_millis(250);

    /// The one user event a watch carries: another thread's word that the wait
    /// has something to look at ([`Waker`]). User events have identities of
    /// their own, apart from descriptors.
    const WAKE: usize = 1;

    /// One kqueue over a child's directory and, once the child has a channel,
    /// one silent connection to it.
    pub struct ChildWatch {
        queue: Arc<Kqueue>,
        directory: std::fs::File,
        channel: Option<HeldChannel>,
    }

    /// Ends a rest of the watch it came from, from another thread (t-18917).
    /// The rest says nothing happened; the wait then looks at what the other
    /// thread answered. A wake with nobody resting is kept for the next rest.
    #[derive(Clone)]
    pub(crate) struct Waker {
        queue: Arc<Kqueue>,
    }

    impl Waker {
        pub(crate) fn wake(&self) {
            let poke = KEvent::new(
                WAKE,
                EventFilter::EVFILT_USER,
                EventFlag::empty(),
                FilterFlag::NOTE_TRIGGER,
                0,
                0,
            );
            let mut none: [KEvent; 0] = [];
            let _ = self.queue.kevent(&[poke], &mut none, Some(timespec_of(Duration::ZERO)));
        }
    }

    /// Rest until process `pid` exits or `deadline` passes, without reaping
    /// it (t-18917): `Some(true)` once it has exited, `Some(false)` at the
    /// deadline, `None` when the kernel will not watch it and the caller has
    /// to look for itself. One kqueue per process asked about; it is closed
    /// on the way out.
    pub(crate) fn exited_by(pid: u32, deadline: Instant) -> Option<bool> {
        let queue = Kqueue::new().ok()?;
        let watch = KEvent::new(
            usize::try_from(pid).ok()?,
            EventFilter::EVFILT_PROC,
            EventFlag::EV_ADD | EventFlag::EV_ONESHOT,
            FilterFlag::NOTE_EXIT,
            0,
            0,
        );
        let mut none: [KEvent; 0] = [];
        match queue.kevent(&[watch], &mut none, Some(timespec_of(Duration::ZERO))) {
            Ok(_) => {}
            // Exited before it could be watched: a process that has exited is
            // not one the kernel watches.
            Err(Errno::ESRCH) => return Some(true),
            Err(_) => return None,
        }
        let mut events = [blank()];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match queue.kevent(&[], &mut events, Some(timespec_of(left))) {
                Ok(0) if left.is_zero() => return Some(false),
                // The rest ran out: what is left of it is looked at again.
                Ok(0) | Err(Errno::EINTR) => {}
                Ok(_) => return Some(true),
                Err(_) => return None,
            }
        }
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
            let wake = KEvent::new(
                WAKE,
                EventFilter::EVFILT_USER,
                EventFlag::EV_ADD | EventFlag::EV_CLEAR,
                FilterFlag::empty(),
                0,
                0,
            );
            let mut none: [KEvent; 0] = [];
            queue
                .kevent(&[change, wake], &mut none, Some(timespec_of(Duration::ZERO)))
                .ok()?;
            Some(Self {
                queue: Arc::new(queue),
                directory,
                channel: None,
            })
        }

        /// Whether a connection to the child's channel is held.
        #[must_use]
        pub fn holds_channel(&self) -> bool {
            self.channel.is_some()
        }

        /// A handle another thread ends this watch's rests with.
        pub(crate) fn waker(&self) -> Waker {
            Waker {
                queue: Arc::clone(&self.queue),
            }
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
                // A [`Waker`]'s event only ends the rest: it says nothing
                // about the child.
                if matches!(event.filter(), Ok(EventFilter::EVFILT_USER)) {
                    continue;
                }
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
    use std::time::{Duration, Instant};

    use super::Woken;

    /// No watch on this platform: [`ChildWatch::open`] never answers one.
    pub enum ChildWatch {}

    /// No watch, so nothing to wake: a wait without one looks on a timer.
    #[derive(Clone)]
    pub(crate) enum Waker {}

    impl Waker {
        pub(crate) fn wake(&self) {
            match *self {}
        }
    }

    /// The kernel is not asked here: the caller looks for the exit itself.
    pub(crate) fn exited_by(_pid: u32, _deadline: Instant) -> Option<bool> {
        None
    }

    impl ChildWatch {
        #[must_use]
        pub fn open(_directory: &Path) -> Option<Self> {
            None
        }

        #[must_use]
        pub fn holds_channel(&self) -> bool {
            match *self {}
        }

        pub fn hold_channel(&mut self, _addr: SocketAddr) -> bool {
            match *self {}
        }

        pub(crate) fn waker(&self) -> Waker {
            match *self {}
        }

        pub fn wait(&mut self, _timeout: Duration) -> Woken {
            match *self {}
        }
    }

    #[test]
    fn unsupported_platforms_never_open_a_native_watcher() {
        assert!(ChildWatch::open(Path::new(".")).is_none());
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

    /// How long the first connection may take to show in the listener's queue.
    /// On loopback `connect` can return before it does, and a fresh process
    /// under load has been seen late; a connection that is on time ends the
    /// wait at once, so the bound costs nothing when it is met.
    const FIRST_ACCEPT_DEADLINE: Duration = Duration::from_secs(2);
    /// How long to keep looking for a second connection after the first. The
    /// second `connect` would have finished inside the same `hold_channel` call
    /// that made the first, so it only has to outlast the queue's lag.
    const SECOND_ACCEPT_QUIET: Duration = Duration::from_millis(100);

    /// The connections a non-blocking `listener` has taken: waits up to `first`
    /// for the first (0 if none comes), then counts until `quiet` passes with
    /// no new one.
    fn accepted_within(listener: &std::net::TcpListener, first: Duration, quiet: Duration) -> usize {
        let mut accepted = 0;
        let mut last = Instant::now();
        loop {
            match listener.accept() {
                Ok(_) => {
                    accepted += 1;
                    last = Instant::now();
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("accept failed: {error}"),
            }
            let patience = if accepted == 0 { first } else { quiet };
            if last.elapsed() >= patience {
                return accepted;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

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
        let accepted = accepted_within(&listener, FIRST_ACCEPT_DEADLINE, SECOND_ACCEPT_QUIET);
        assert_eq!(accepted, 1, "the same channel was connected to again");

        // Nobody listens on a port that was just released.
        let closed = {
            let gone = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            gone.local_addr().expect("addr")
        };
        assert!(!watch.hold_channel(closed));
        assert!(!watch.holds_channel(), "a refused channel replaced the held one with nothing held");
    }

    /// Another thread ends a rest at once, and the rest says nothing about the
    /// child — the wait looks at what that thread answered (t-18917). A wake
    /// with nobody resting is kept for the next rest.
    #[test]
    fn a_wake_from_another_thread_ends_a_rest_at_once_and_says_nothing_of_the_child() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut watch = ChildWatch::open(directory.path()).expect("watch");
        let waker = watch.waker();
        let waking = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            waker.wake();
            Instant::now()
        });
        let woken = watch.wait(Duration::from_secs(5));
        let seen = Instant::now();
        let woke = waking.join().expect("waker");
        assert_eq!(woken, Woken::NOTHING, "a wake was read as news of the child");
        assert!(
            seen.duration_since(woke) < Duration::from_millis(250),
            "the rest ended {:?} after the wake",
            seen.duration_since(woke)
        );

        watch.waker().wake();
        let started = Instant::now();
        assert_eq!(watch.wait(Duration::from_secs(5)), Woken::NOTHING);
        assert!(started.elapsed() < Duration::from_millis(250), "a wake before the rest was lost");
    }

    /// The kernel says when a process exits, without reaping it, and a
    /// deadline is kept when it does not (t-18917).
    #[test]
    fn an_exit_is_heard_at_once_and_a_deadline_is_kept() {
        let mut quick = std::process::Command::new("/bin/sleep").arg("0.1").spawn().expect("spawn");
        let started = Instant::now();
        assert_eq!(super::exited_by(quick.id(), started + Duration::from_secs(5)), Some(true));
        assert!(started.elapsed() < Duration::from_secs(1), "the exit was heard after {:?}", started.elapsed());
        assert!(quick.try_wait().expect("try_wait").is_some(), "the exited process was not left to reap");

        let mut slow = std::process::Command::new("/bin/sleep").arg("5").spawn().expect("spawn");
        let started = Instant::now();
        assert_eq!(super::exited_by(slow.id(), started + Duration::from_millis(300)), Some(false));
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(300) && took < Duration::from_millis(600),
            "a 300 ms deadline was kept at {took:?}"
        );
        let _ = slow.kill();
        let _ = slow.wait();
    }
}
