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
}

/// The stand-in: its folder, its socket and what it shares with the threads
/// that answer.
pub(crate) struct StandIn {
    directory: PathBuf,
    socket: PathBuf,
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
        let shared = Arc::new(Shared {
            token: token.to_string(),
            shape,
            stop: Mutex::new(Stop::default()),
            changed: Condvar::new(),
            heard: Mutex::new(Vec::new()),
            closing: AtomicBool::new(false),
            watch_fault: Mutex::new(None),
        });
        let accepting = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if shared.closing.load(Ordering::Acquire) {
                        return;
                    }
                    let Ok(stream) = stream else { continue };
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || serve(&stream, &shared));
                }
            })
        };
        Self {
            directory,
            socket,
            shared,
            accepting: Some(accepting),
        }
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
