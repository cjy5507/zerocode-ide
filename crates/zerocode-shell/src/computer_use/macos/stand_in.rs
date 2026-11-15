//! A stand-in for the signed helper (t-66753 C): a thread of the test
//! process that listens where a launched helper would — a UNIX socket in a
//! 0700 folder of its own — and answers the way `main.swift` answers. Only
//! the process is stood in for. The window's side runs its own code
//! throughout: [`Session::connect`] reads the pid off the socket
//! (`LOCAL_PEERPID`), sends the token with every request and holds the
//! handshake's protocol and guard table to its own; the stand-in reads the
//! pid on its side too and refuses a wrong token in the helper's words.
//!
//! Nothing is launched, nothing reaches the person's screen or input, and no
//! signal is sent: the pid the window reads is the test process's own, whose
//! executable is no helper's, so the session's end never signals it.

use std::io::{BufRead as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::computer_use::{COMPUTER_USE_PROTOCOL_VERSION, guard_table};

use super::{Launched, Session, peer_pid};
use crate::computer_use::ComputerUseError;
use crate::computer_use::session::ProviderSession as _;

/// The helper's words for a request whose token is not the session's.
const WRONG_TOKEN: &str = "invalid computer-use agent token";
/// The helper's words for a peer it does not authorize.
const STRANGE_PEER: &str = "computer-use agent peer is not authorized";
/// The stop reason the person's chord writes in the helper (`StopReason`).
const CHORD: &str = zerocode_core::computer_use::STOP_REASON_HOTKEY;

/// What the stand-in answers a handshake with: the helper's protocol and the
/// guard table it counts by.
pub(crate) struct Shape {
    pub(crate) protocol: u64,
    pub(crate) budget: Value,
}

impl Shape {
    /// A helper built with this window: its protocol and the core's table.
    pub(crate) fn matching() -> Self {
        Self {
            protocol: COMPUTER_USE_PROTOCOL_VERSION,
            budget: Value::Object(guard_table()),
        }
    }
}

/// The helper's stop as its `status` answers it: the reason, and the count
/// that moves whenever the stop changes.
#[derive(Default)]
struct Stop {
    reason: Option<String>,
    revision: u64,
}

impl Stop {
    fn status(&self) -> Value {
        json!({
            "stopped": self.reason.is_some(),
            "reason": self.reason,
            "stopRevision": self.revision,
        })
    }
}

/// One request the stand-in heard: its method and params, the pid it read
/// off the socket, and whether it carried the token the stand-in expects.
#[derive(Clone, Debug)]
pub(crate) struct Heard {
    pub(crate) method: String,
    pub(crate) params: Value,
    pub(crate) peer: Option<i32>,
    pub(crate) token_ok: bool,
}

/// How the stand-in's waiting `stopWatch` goes wrong (t-68324 ①): the three
/// ways the window's side of it can end while its session stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WatchFault {
    /// The helper shuts the connection without an answer.
    HangUp,
    /// The helper refuses the request in its own words.
    Refuse,
    /// The helper answers, with no stop count to ask after.
    NoCount,
}

struct Shared {
    token: String,
    shape: Shape,
    stop: Mutex<Stop>,
    changed: Condvar,
    heard: Mutex<Vec<Heard>>,
    closing: AtomicBool,
    /// The fault the next `stopWatch` answer carries, once one is set.
    watch_fault: Mutex<Option<WatchFault>>,
    /// The door: whether the stand-in takes no connection for now, as a helper
    /// too busy to accept does, and whether its accepting thread stands there
    /// waiting. What connects meanwhile fills its queue.
    door: Mutex<Door>,
    door_moved: Condvar,
}

#[derive(Default)]
struct Door {
    shut: bool,
    waited_at: bool,
}

impl Shared {
    fn stop(&self) -> std::sync::MutexGuard<'_, Stop> {
        self.stop.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn watch_fault(&self) -> Option<WatchFault> {
        *self
            .watch_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn door(&self) -> std::sync::MutexGuard<'_, Door> {
        self.door.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The accepting thread waits here for as long as the door is shut, and
    /// says that it does.
    fn wait_at_the_door(&self) {
        let mut door = self.door();
        while door.shut {
            door.waited_at = true;
            self.door_moved.notify_all();
            door = self
                .door_moved
                .wait(door)
                .unwrap_or_else(PoisonError::into_inner);
        }
        door.waited_at = false;
    }

    fn move_the_door(&self, shut: bool) {
        self.door().shut = shut;
        self.door_moved.notify_all();
    }

    /// Wait until the accepting thread stands at the shut door: from then on
    /// it takes nothing out of the queue.
    fn wait_for_the_doorkeeper(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut door = self.door();
        while !door.waited_at {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("the stand-in's accepting thread never came to the door");
            door = self
                .door_moved
                .wait_timeout(door, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// The stand-in: its folder, its socket and what it shares with the threads
/// that answer.
pub(crate) struct StandIn {
    directory: PathBuf,
    socket: PathBuf,
    /// The listening socket, for the length of its queue.
    listening: std::os::unix::io::RawFd,
    shared: Arc<Shared>,
    accepting: Option<std::thread::JoinHandle<()>>,
}

impl StandIn {
    /// Listen as a helper launched with `token` would, answering a handshake
    /// with `shape`.
    pub(crate) fn start(token: &str, shape: Shape) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        // Short, as the launch's own folder is: a socket path has room for
        // 103 bytes.
        let directory = PathBuf::from(format!(
            "/tmp/zerocode-stand-in-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).expect("the stand-in's folder");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("the stand-in's folder is its own");
        let socket = directory.join("provider.sock");
        let listener = UnixListener::bind(&socket).expect("the stand-in listens");
        let listening = std::os::unix::io::AsRawFd::as_raw_fd(&listener);
        let shared = Arc::new(Shared {
            token: token.to_string(),
            shape,
            stop: Mutex::new(Stop::default()),
            changed: Condvar::new(),
            heard: Mutex::new(Vec::new()),
            closing: AtomicBool::new(false),
            watch_fault: Mutex::new(None),
            door: Mutex::new(Door::default()),
            door_moved: Condvar::new(),
        });
        let accepting = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                loop {
                    shared.wait_at_the_door();
                    let stream = listener.accept();
                    if shared.closing.load(Ordering::Acquire) {
                        return;
                    }
                    let Ok((stream, _)) = stream else { continue };
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || serve(&stream, &shared));
                }
            })
        };
        Self {
            directory,
            socket,
            listening,
            shared,
            accepting: Some(accepting),
        }
    }

    /// Take no connection for now, with a queue of one, and fill that queue:
    /// what comes back keeps it full until it is dropped. The next connection
    /// is refused at once, as the helper's is when the groups of a `parallel`
    /// connect faster than it accepts.
    pub(crate) fn shut_the_door(&self) -> Vec<UnixStream> {
        self.shared.move_the_door(true);
        // SAFETY: the stand-in owns this listening socket for its whole life.
        assert_eq!(
            unsafe { libc::listen(self.listening, 1) },
            0,
            "a queue of one"
        );
        // The accepting thread may be inside an accept already: this
        // connection takes it out, and then it waits at the door.
        let mut filling = vec![UnixStream::connect(&self.socket).expect("the stand-in listens")];
        self.shared.wait_for_the_doorkeeper();
        loop {
            match UnixStream::connect(&self.socket) {
                Ok(stream) => filling.push(stream),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    return filling;
                }
                Err(error) => panic!("the stand-in's queue: {error}"),
            }
            assert!(filling.len() < 64, "the queue never filled");
        }
    }

    /// Take connections again.
    pub(crate) fn open_the_door(&self) {
        self.shared.move_the_door(false);
    }

    /// What a launch hands the connection, for a window holding `token`: a
    /// session folder of its own, beside the stand-in's socket, with the
    /// token file the launch writes.
    pub(crate) fn launched(&self, token: &str) -> Launched {
        let directory = self.directory.join("session");
        let _ = std::fs::create_dir(&directory);
        let token_file = directory.join("provider.token");
        std::fs::write(&token_file, token).expect("the launch's token file");
        Launched {
            directory,
            socket: self.socket.clone(),
            token_file,
            token: token.to_string(),
        }
    }

    /// The window connects to the stand-in by its own road, holding `token`.
    pub(crate) fn connect_window(&self, token: &str) -> Result<Connected, ComputerUseError> {
        Session::connect(self.launched(token)).map(Connected)
    }

    /// The person's chord, as the helper takes it: the stop is theirs, its
    /// count moves, and a waiting request hears it.
    pub(crate) fn press_chord(&self) {
        let mut stop = self.shared.stop();
        stop.reason = Some(CHORD.to_string());
        stop.revision += 1;
        drop(stop);
        self.shared.changed.notify_all();
    }

    /// The waiting request goes wrong, as `fault` says: the one waiting now
    /// hears it, and so does every one after it.
    pub(crate) fn break_watch(&self, fault: WatchFault) {
        *self
            .shared
            .watch_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(fault);
        self.shared.changed.notify_all();
    }

    /// Every request heard so far, oldest first.
    pub(crate) fn heard(&self) -> Vec<Heard> {
        self.shared
            .heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Wait until a request with `method` has been heard `count` times, or
    /// `within` passes; answers how many were heard.
    pub(crate) fn wait_heard(&self, method: &str, count: usize, within: Duration) -> usize {
        let began = Instant::now();
        loop {
            let seen = self
                .heard()
                .iter()
                .filter(|heard| heard.method == method)
                .count();
            if seen >= count || began.elapsed() >= within {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        self.shared.changed.notify_all();
        self.shared.move_the_door(false);
        // The accept loop wakes on a connection of its own and sees it is
        // closing.
        if UnixStream::connect(&self.socket).is_ok()
            && let Some(accepting) = self.accepting.take()
        {
            let _ = accepting.join();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// A window session connected to a stand-in.
pub(crate) struct Connected(Session);

impl Connected {
    pub(crate) fn helper_pid(&self) -> Option<i32> {
        self.0.helper_pid()
    }

    pub(crate) fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, ComputerUseError> {
        self.0
            .request(method, params)
            .map_err(|failure| match failure {
                super::SessionFailure::Transport(error)
                | super::SessionFailure::Provider(error) => error,
            })
    }
}

/// One connection, answered request by request as the helper's connection
/// thread answers it.
fn serve(stream: &UnixStream, shared: &Shared) {
    // The helper's first read of a connection: who is on its other end.
    let peer = peer_pid(stream);
    let own = i32::try_from(std::process::id()).ok();
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let reader = std::io::BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        // The helper passes over a line it cannot read.
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let token_ok = request.get("token").and_then(Value::as_str) == Some(shared.token.as_str());
        shared
            .heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Heard {
                method: method.clone(),
                params: params.clone(),
                peer,
                token_ok,
            });
        let refused = |message: &str| json!({ "id": id, "ok": false, "error": { "code": "permission_denied", "message": message } });
        let said = if !token_ok {
            refused(WRONG_TOKEN)
        } else if peer.is_none() || peer != own {
            refused(STRANGE_PEER)
        } else {
            match answer(&method, &params, shared) {
                Ok(result) => json!({ "id": id, "ok": true, "result": result }),
                Err(message) => json!({
                    "id": id,
                    "ok": false,
                    "error": { "code": "invalid_argument", "message": message },
                }),
            }
        };
        // A hung-up watch answers nothing: the connection is shut on it.
        if method == "stopWatch" && shared.watch_fault() == Some(WatchFault::HangUp) {
            return;
        }
        if writeln!(writer, "{said}")
            .and_then(|()| writer.flush())
            .is_err()
            || method == "terminate"
        {
            return;
        }
    }
}

/// The helper's answer to one authenticated request.
fn answer(method: &str, params: &Value, shared: &Shared) -> Result<Value, String> {
    match method {
        "handshake" => Ok(json!({
            "platform": "darwin",
            "provider": "zerocode-computer-use-macos",
            "protocolVersion": shared.shape.protocol,
            "supports": { "desktop": { "guard": { "budget": shared.shape.budget } } },
        })),
        "status" => Ok(shared.stop().status()),
        "stopWatch" => {
            let wait = params
                .get("ms")
                .and_then(Value::as_u64)
                .ok_or("ms must be a whole number of milliseconds")?;
            let deadline = Instant::now() + Duration::from_millis(wait);
            let after = params.get("after").and_then(Value::as_u64);
            let mut stop = shared.stop();
            while after == Some(stop.revision)
                && !shared.closing.load(Ordering::Acquire)
                && shared.watch_fault().is_none()
            {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                stop = shared
                    .changed
                    .wait_timeout(stop, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            match shared.watch_fault() {
                Some(WatchFault::Refuse) => Err("the helper cannot wait for a stop".to_string()),
                Some(WatchFault::NoCount) => Ok(json!({ "stopped": false })),
                Some(WatchFault::HangUp) | None => Ok(stop.status()),
            }
        }
        "resume" => {
            let mut stop = shared.stop();
            if stop.reason.take().is_some() {
                stop.revision += 1;
            }
            let status = stop.status();
            drop(stop);
            shared.changed.notify_all();
            Ok(status)
        }
        "terminate" => Ok(json!({ "ok": true })),
        other => Err(format!("unknown method '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::computer_use_protocol::error_code;

    const TOKEN: &str = "stand-in-token";

    fn own_pid() -> Option<i32> {
        i32::try_from(std::process::id()).ok()
    }

    /// The window's road to a helper, the launch apart (t-66753 C3): it reads
    /// the helper's pid off the socket, removes the token file the launch
    /// wrote, sends the token with the handshake and takes the answer —
    /// every check on the window's side its own code. Before this round the
    /// only test of that road was `#[ignore]`d and launched the signed helper.
    #[test]
    fn the_window_completes_the_authenticated_handshake_with_a_stand_in() {
        let helper = StandIn::start(TOKEN, Shape::matching());
        let launched = helper.launched(TOKEN);
        let token_file = launched.token_file.clone();
        let session = Session::connect(launched).map(Connected);
        let heard = helper.heard();
        let session = session.expect("the window connects to a helper of its own build");
        assert_eq!(
            session.helper_pid(),
            own_pid(),
            "the window reads the helper's pid off the socket (LOCAL_PEERPID)"
        );
        assert!(
            !super::super::helper_runs_at(own_pid().expect("a pid")),
            "the test process is never taken for the helper: the session's end signals no one"
        );
        assert!(
            !token_file.exists(),
            "the token file is gone once the window connected"
        );
        let handshake = heard
            .iter()
            .find(|heard| heard.method == "handshake")
            .expect("the window shook hands");
        assert!(
            handshake.token_ok,
            "the handshake carried the session's token"
        );
        assert_eq!(
            handshake.peer,
            own_pid(),
            "the helper's side reads the window's pid off the socket"
        );
        drop(session);
        assert_eq!(
            helper.wait_heard("terminate", 1, Duration::from_secs(5)),
            1,
            "the session's end asks the helper to go"
        );
    }

    /// The groups of one `parallel` connect in the same instant, and the
    /// helper's queue holds eight (t-109501). A connection the full queue
    /// refused sent nothing: it is tried again, every request is answered, and
    /// the session — the one helper the window's stop reaches — is the one
    /// that answers them all.
    #[test]
    fn requests_past_the_helpers_full_queue_are_all_answered_on_the_one_session() {
        const AT_ONCE: usize = 16;
        let helper = StandIn::start(TOKEN, Shape::matching());
        let window = helper.connect_window(TOKEN).expect("connected");
        let filling = helper.shut_the_door();
        let answers: Vec<Result<Value, String>> = std::thread::scope(|scope| {
            let asking: Vec<_> = (0..AT_ONCE)
                .map(|_| {
                    scope.spawn(|| {
                        window.0.request("status", json!({})).map_err(|failure| {
                            let (super::super::SessionFailure::Transport(error)
                            | super::super::SessionFailure::Provider(error)) = failure;
                            format!("{}: {}", error.code, error.message)
                        })
                    })
                })
                .collect();
            // The queue stays full while they try: longer than a few tries.
            std::thread::sleep(Duration::from_millis(60));
            helper.open_the_door();
            asking
                .into_iter()
                .map(|asked| asked.join().expect("a request"))
                .collect()
        });
        drop(filling);
        for answer in &answers {
            assert!(answer.is_ok(), "the full queue cost a request: {answer:?}");
        }
        let heard = helper.heard();
        assert_eq!(
            heard
                .iter()
                .filter(|heard| heard.method == "status")
                .count(),
            AT_ONCE,
            "every request reached the one helper"
        );
        assert_eq!(
            heard
                .iter()
                .filter(|heard| heard.method == "handshake")
                .count(),
            1,
            "and no second session was made for any of them"
        );
    }

    /// A helper that lives and takes no connection through the wait refuses
    /// that request alone — nothing of it was sent — and the session stands:
    /// no second helper is started beside one that may still be at work, out
    /// of reach of the window's stop (t-109578).
    #[test]
    fn a_living_helper_that_takes_no_connection_refuses_the_request_and_keeps_the_session() {
        let helper = StandIn::start(TOKEN, Shape::matching());
        let window = helper.connect_window(TOKEN).expect("connected");
        // With no idle connection the next request needs a new one.
        let held = window
            .0
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let filling = helper.shut_the_door();
        let refused = match window.0.request("status", json!({})) {
            Err(super::super::SessionFailure::Provider(refused)) => refused,
            Err(super::super::SessionFailure::Transport(broken)) => {
                panic!(
                    "the session is discarded for a helper that lives: {}",
                    broken.message
                )
            }
            Ok(answer) => panic!("a full queue answered: {answer}"),
        };
        assert_eq!(refused.code, error_code::ACTION_TIMEOUT);
        assert_eq!(
            refused.delivery.as_deref(),
            Some(zerocode_core::computer_use_protocol::NOT_DELIVERED),
            "nothing of the request was sent"
        );
        helper.open_the_door();
        drop((filling, held));
        window
            .0
            .request("status", json!({}))
            .map_err(|_| "the same session answers once the helper takes connections again")
            .expect("answered");
        assert_eq!(
            helper
                .heard()
                .iter()
                .filter(|heard| heard.method == "handshake")
                .count(),
            1,
            "one session, one helper, from first to last"
        );
    }

    /// A session the window discards may still be held by another group's
    /// request, with a step under way at its helper, and the window's stop is
    /// aimed at the session that stands. So the helper of a discarded session
    /// is asked to go at the discard — not when its last request ends
    /// (t-109578).
    #[test]
    fn the_helper_of_a_discarded_session_is_asked_to_go_at_once() {
        let helper = StandIn::start(TOKEN, Shape::matching());
        let Connected(session) = helper.connect_window(TOKEN).expect("connected");
        let session = Arc::new(session);
        // Another group's request, still under way on the same session.
        let in_flight = Arc::clone(&session);
        let broke = ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            "read Computer Use response: the road broke",
        );
        let said = crate::computer_use::discard(&session, broke.clone());
        assert_eq!(said, broke, "the break is the answer");
        assert_eq!(
            helper.wait_heard("terminate", 1, Duration::from_secs(2)),
            1,
            "the helper was not asked to go while a request still held its session"
        );
        drop((session, in_flight));
    }

    /// `shutdown` ends the session for a fresh one (the permission roads call
    /// it). A request of another agent may still hold that session, so its
    /// helper is sent away at the shutdown — not when that request ends
    /// (t-109578).
    #[test]
    fn the_helper_of_a_session_that_is_shut_down_is_asked_to_go_at_once() {
        // The session that stands is the process's: one test holds it at a time.
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let helper = StandIn::start(TOKEN, Shape::matching());
        let Connected(session) = helper.connect_window(TOKEN).expect("connected");
        let session = Arc::new(session);
        *crate::computer_use::held_client()
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&session));
        crate::computer_use::shutdown();
        assert_eq!(
            helper.wait_heard("terminate", 1, Duration::from_secs(2)),
            1,
            "the helper was not asked to go while a request still held its session"
        );
        assert!(!crate::computer_use::session_stands());
        drop(session);
    }

    /// The window reads the permission rows whenever it is focused. A reading
    /// that moved no row leaves the session and its helper alone — every act
    /// under way goes on — and one that moved a row ends the session, its
    /// helper sent away, so the next call starts one that sees the new row
    /// (t-109675).
    #[test]
    fn a_permission_reading_ends_the_session_only_when_a_row_moved() {
        use zerocode_core::computer_use::{
            ComputerPermissionId, ComputerPermissionState, ComputerPermissionStatus,
        };
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let rows = |screenshots| {
            [
                (
                    ComputerPermissionId::Accessibility,
                    ComputerPermissionStatus::Granted,
                ),
                (ComputerPermissionId::Screenshots, screenshots),
            ]
            .map(|(id, status)| ComputerPermissionState { id, status })
        };
        let granted = rows(ComputerPermissionStatus::Granted);
        // The first reading: no session stands yet, and none is ended.
        *super::super::ROWS_READ
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        super::super::settle_session_for(&granted);

        let helper = StandIn::start(TOKEN, Shape::matching());
        let Connected(session) = helper.connect_window(TOKEN).expect("connected");
        let session = Arc::new(session);
        *crate::computer_use::held_client()
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&session));

        super::super::settle_session_for(&granted);
        assert!(
            crate::computer_use::session_stands(),
            "a reading that moved no row ended the session"
        );
        assert_eq!(
            helper.wait_heard("terminate", 1, Duration::from_millis(300)),
            0,
            "a reading that moved no row sent the helper away"
        );
        session
            .request("status", json!({}))
            .map_err(|_| "the session no longer answers")
            .expect("what the session has under way goes on");

        super::super::settle_session_for(&rows(ComputerPermissionStatus::NotGranted));
        assert!(
            !crate::computer_use::session_stands(),
            "a row moved and the session still stands"
        );
        assert_eq!(
            helper.wait_heard("terminate", 1, Duration::from_secs(2)),
            1,
            "a row moved and the helper was not sent away"
        );
        drop(session);
    }

    /// A reading is compared with the one before it, which it replaces.
    #[test]
    fn the_rows_moved_when_they_differ_from_the_last_reading() {
        use zerocode_core::computer_use::{
            ComputerPermissionId, ComputerPermissionState, ComputerPermissionStatus,
        };
        let rows = |status| {
            [ComputerPermissionState {
                id: ComputerPermissionId::Screenshots,
                status,
            }]
        };
        let mut read = None;
        let granted = rows(ComputerPermissionStatus::Granted);
        assert!(
            super::super::rows_moved(&mut read, &granted),
            "the first reading has none before it"
        );
        assert!(!super::super::rows_moved(&mut read, &granted));
        assert!(super::super::rows_moved(
            &mut read,
            &rows(ComputerPermissionStatus::NotGranted)
        ));
        assert!(
            super::super::rows_moved(&mut read, &granted),
            "granted again is a row that moved"
        );
        assert!(!super::super::rows_moved(&mut read, &granted));
    }

    /// A living helper that takes no connection is busy for a while and
    /// broken after that: past the patience the session goes — its helper is
    /// sent away first — so a helper that stopped accepting is not kept for
    /// good. One connection it takes, and the count begins again.
    #[test]
    fn a_helper_that_takes_no_connection_for_long_enough_is_no_busy_one() {
        use super::super::Untaken;
        let patience = Duration::from_secs(10);
        let began = Instant::now();
        let at = |seconds: u64| began + Duration::from_secs(seconds);
        let mut untaken = Untaken::default();
        assert!(
            !untaken.refused(at(0), at(2), patience),
            "two seconds: busy"
        );
        assert!(
            !untaken.refused(at(5), at(7), patience),
            "inside the patience"
        );
        assert!(
            untaken.refused(at(8), at(10), patience),
            "ten seconds since the first refusal, whatever asked meanwhile"
        );
        untaken.taken();
        assert!(
            !untaken.refused(at(11), at(13), patience),
            "a connection it took begins the count again"
        );
    }

    /// Only a queue that is full for a moment is waited out: a refusal from a
    /// helper that is gone is the answer at once, and one that lives but
    /// never takes the connection is given up when the wait is over.
    #[test]
    fn a_refused_connection_is_tried_again_only_while_the_helper_lives_and_the_wait_lasts() {
        let helper = StandIn::start(TOKEN, Shape::matching());
        let filling = helper.shut_the_door();
        let connect = |wait, lives: bool| {
            let started = Instant::now();
            let answer = super::super::connect_past_a_full_queue(&helper.socket, wait, || lives);
            (
                answer.map(drop).map_err(|error| error.kind()),
                started.elapsed(),
            )
        };
        let (gone, took) = connect(Duration::from_secs(5), false);
        assert_eq!(gone, Err(std::io::ErrorKind::ConnectionRefused));
        assert!(
            took < Duration::from_secs(1),
            "waited {took:?} for a helper that is gone"
        );
        let (never_taken, took) = connect(Duration::from_millis(40), true);
        assert_eq!(never_taken, Err(std::io::ErrorKind::ConnectionRefused));
        assert!(took >= Duration::from_millis(40), "gave up after {took:?}");
        drop(filling);
    }

    /// A helper of another protocol is refused at the handshake, in the
    /// window's words (t-66753 C3).
    #[test]
    fn a_helper_of_another_protocol_is_refused() {
        let helper = StandIn::start(
            TOKEN,
            Shape {
                protocol: COMPUTER_USE_PROTOCOL_VERSION + 1,
                ..Shape::matching()
            },
        );
        let refused = helper.connect_window(TOKEN).map(|_| ()).unwrap_err();
        assert_eq!(
            refused.code,
            error_code::PROVIDER_INCOMPATIBLE,
            "a helper of another protocol is refused: {refused:?}"
        );
        assert!(refused.message.contains("protocol"), "{refused:?}");
    }

    /// A helper counting by another guard table is refused at the handshake
    /// (t-66753 C3): an older table of rates is not the core's.
    #[test]
    fn a_helper_with_another_guard_table_is_refused() {
        let helper = StandIn::start(
            TOKEN,
            Shape {
                budget: json!({ "perSecond": 10, "burst": 20, "perSession": 5000 }),
                ..Shape::matching()
            },
        );
        let refused = helper.connect_window(TOKEN).map(|_| ()).unwrap_err();
        assert_eq!(
            refused.code,
            error_code::PROVIDER_INCOMPATIBLE,
            "a helper with another guard table is refused: {refused:?}"
        );
        assert!(refused.message.contains("guard policy"), "{refused:?}");
    }

    /// A window that does not hold the helper's token is refused: the helper
    /// says so and the window answers that refusal, holding no session
    /// (t-66753 C3).
    #[test]
    fn a_window_with_the_wrong_token_is_refused() {
        let helper = StandIn::start(TOKEN, Shape::matching());
        let refused = helper
            .connect_window("another-token")
            .map(|_| ())
            .unwrap_err();
        let heard = helper.heard();
        assert_eq!(
            refused.code,
            error_code::PERMISSION_DENIED,
            "a wrong token is refused: {refused:?}"
        );
        assert_eq!(refused.message, WRONG_TOKEN, "{refused:?}");
        assert!(
            heard
                .iter()
                .all(|heard| heard.method != "handshake" || !heard.token_ok),
            "no handshake passed with the wrong token: {heard:?}"
        );
    }
}
