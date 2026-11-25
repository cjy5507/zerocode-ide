//! A Claude pane's local MCP server, started by its first call.
//!
//! Claude Code starts every MCP server it is told of when a session opens, to
//! ask each what it offers, and keeps each running for as long as the session
//! does, called or not: on one machine on 2026-11-16, thirteen panes held nine
//! server processes each, about 345 MiB a pane and 4.5 GiB in all. The window
//! writes a Claude pane's local servers through the bundled `zerocode-mirror`
//! with [`MARKER`] in the entry's environment, and the mirror runs [`serve`]
//! in front of the real command:
//!
//! - **With a listing** — what the same server (its name and its command line)
//!   answered to the listing methods before, kept under the window's data
//!   folder for [`LISTING_STANDS_FOR`] — those methods are answered from it
//!   and the real server is not started.
//! - **The first other request starts it.** The client's own `initialize` is
//!   sent first, under an id of the stand-in's own, then
//!   `notifications/initialized`, then the request. What the server lists
//!   then is compared with the listing, and a change is kept for the next
//!   session.
//! - **Without a listing** — a server seen for the first time, a listing a day
//!   old, a client asking another protocol version — the real server starts
//!   at once and what it answers is kept, once its start and its tools were
//!   answered and not refused: a `tools/list` that fails is asked again by
//!   the next session. One the session does not use is stopped
//!   [`IDLE_AFTER_LEARNING`] later and started again by a call, as above.
//! - **A server that does not start** answers that request with an error that
//!   names it ([`NOT_STARTED`]), and its listing is forgotten: the next
//!   session starts it at once, where Claude's own status shows the failure.
//!
//! The stand-in is handed what the server is handed — the environment and the
//! expanded arguments — and passes them on; it writes neither anywhere. A
//! listing holds what the server answered and is found by the server's name
//! and a hash of its command line ([`listing_key`]); nothing is logged.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The variable an entry the window wrote carries, its value the server's
/// name: the mirror stands in for the server only while it is set, and takes
/// it out of what the real server is handed.
pub const MARKER: &str = "ZEROCODE_MCP_ON_FIRST_CALL";

/// A folder for the listings other than the window's own — for a test, or a
/// measurement whose sandbox shuts the window's data folder. Taken out of what
/// the real server is handed, as [`MARKER`] is.
pub const LISTINGS_ENV: &str = "ZEROCODE_MCP_LISTINGS";

/// How long a listing answers for its server. A server something calls is
/// asked again at that call whatever the listing's age, so this bounds one
/// thing only: how long a change to a server nothing calls goes unseen.
pub const LISTING_STANDS_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a server started by a call has to answer its `initialize`: an
/// `npx -y …@latest` server fetches its package first, which took tens of
/// seconds on a cold cache, and an `mcp-remote` bridge whose login ran out
/// answers only once the person has signed in again in a browser.
pub const START_LIMIT: Duration = Duration::from_secs(300);

/// How long a server started only to learn what it offers runs on before it
/// is stopped, when the session has called nothing of it — the rule zo keeps
/// for its own sessions (`release_unused_servers`).
pub const IDLE_AFTER_LEARNING: Duration = Duration::from_secs(90);

/// The JSON-RPC code a request the server could not be handed is answered
/// with: the implementation-defined server-error range.
pub const NOT_STARTED: i64 = -32000;

/// The folder under the window's data folder that holds the listings.
const LISTINGS_FOLDER: &str = "mcp-listings";
/// The shape of a listing file; one of another shape is no listing.
const LISTING_VERSION: u32 = 1;
/// The methods a listing answers: what a client asks a server as a session
/// opens. Claude Code 2.1.322 asked `server/discover`, `initialize` and
/// `tools/list` of a server offering tools (2026-11-12).
const REMEMBERED: [&str; 6] = [
    "server/discover",
    "initialize",
    "tools/list",
    "prompts/list",
    "resources/list",
    "resources/templates/list",
];
/// Use signed 32-bit ids for servers that read every id as an int32.
/// The stand-in uses negative ids for its own requests and positive ids
/// for requests it forwards, regardless of the client's original id.
const OWN_INITIALIZE: i32 = -2;
const OWN_RELIST: i32 = -3;
const OWN_LOG_LEVEL: i32 = -4;
/// How long a server is given to go once told to, before the next signal.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// What one server answered to the listing methods, as a file holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub version: u32,
    /// The server's configured name, for a person reading the folder.
    pub server: String,
    /// The protocol version the client asked for when the server answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    pub listed_unix: u64,
    /// Each method's answer: its `result` or its `error`, nothing else.
    pub answers: BTreeMap<String, Value>,
}

impl Listing {
    fn new(server: &str) -> Self {
        Self {
            version: LISTING_VERSION,
            server: server.to_string(),
            protocol: None,
            listed_unix: 0,
            answers: BTreeMap::new(),
        }
    }

    /// Whether the listing can open a session: of this shape, younger than
    /// `stands_for` at `now` (one dated after `now` — a clock set back — is
    /// not), and holding the server's start and its tools.
    #[must_use]
    pub fn stands(&self, now: SystemTime, stands_for: Duration) -> bool {
        let listed = UNIX_EPOCH + Duration::from_secs(self.listed_unix);
        self.version == LISTING_VERSION
            && now.duration_since(listed).is_ok_and(|age| age < stands_for)
            && self.complete()
    }

    /// Whether the server's start and its tools are held as answers. A server
    /// that answered `tools/list` with an error while it was learned is not
    /// listed: kept, the error would answer every session for a day with no
    /// tool to call, and so nothing would start the server to ask again.
    fn complete(&self) -> bool {
        let answered = |method: &str| {
            self.answers
                .get(method)
                .is_some_and(|answer| answer.get("result").is_some())
        };
        answered("initialize") && answered("tools/list")
    }

    /// The answer to `method` under the client's `id`, when one is kept.
    #[must_use]
    pub fn answer(&self, method: &str, id: &Value) -> Option<Value> {
        let kept = self.answers.get(method)?.as_object()?;
        let mut answer = serde_json::Map::new();
        answer.insert("jsonrpc".into(), "2.0".into());
        answer.insert("id".into(), id.clone());
        for field in ["result", "error"] {
            if let Some(value) = kept.get(field) {
                answer.insert(field.into(), value.clone());
            }
        }
        Some(Value::Object(answer))
    }
}

/// The name a server's listing is filed under: its configured name, kept to
/// the characters a file name takes, and the first 16 hex digits of a SHA-256
/// over its command line — so a server started another way is asked anew,
/// and no argument is readable from the name.
#[must_use]
pub fn listing_key(name: &str, real: &OsStr, args: &[OsString]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(real.to_string_lossy().as_bytes());
    for arg in args {
        hash.update([0]);
        hash.update(arg.to_string_lossy().as_bytes());
    }
    let digest: String = hash
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let shown: String = name
        .chars()
        .take(64)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{shown}-{digest}")
}

/// Where listings are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listings {
    pub dir: PathBuf,
}

impl Listings {
    /// [`LISTINGS_ENV`] when it names a folder, else `mcp-listings` under the
    /// window's data folder; `None` where neither can be told.
    #[must_use]
    pub fn for_this_machine() -> Option<Self> {
        if let Some(dir) = std::env::var_os(LISTINGS_ENV).filter(|dir| !dir.is_empty()) {
            return Some(Self {
                dir: PathBuf::from(dir),
            });
        }
        window_data_dir().map(|data| Self {
            dir: data.join(LISTINGS_FOLDER),
        })
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// The listing under `key`, whatever its age.
    #[must_use]
    pub fn recall(&self, key: &str) -> Option<Listing> {
        let text = std::fs::read_to_string(self.file(key)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Keep `listing` under `key`: written beside its place and renamed over
    /// it, so a session reading meanwhile finds the old listing or the new
    /// one and never a part of either. The folder and the file are the
    /// person's alone.
    pub fn remember(&self, key: &str, listing: &Listing) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let text = serde_json::to_string(listing).map_err(io::Error::other)?;
        let partial = self
            .dir
            .join(format!("{key}.{}.partial", std::process::id()));
        let written = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options.open(&partial)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&partial, self.file(key))
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        written
    }

    /// Stop answering from the listing under `key`: the next session starts
    /// the server at once.
    pub fn forget(&self, key: &str) {
        let _ = std::fs::remove_file(self.file(key));
    }
}

/// The window's data folder (`dev.zerocode.app` under the platform's data
/// root), as the window's own paths name it.
fn window_data_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let root = if cfg!(target_os = "macos") {
        PathBuf::from(var("HOME")?)
            .join("Library")
            .join("Application Support")
    } else if cfg!(windows) {
        PathBuf::from(var("LOCALAPPDATA")?)
    } else {
        var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(var("HOME")?).join(".local").join("share")))?
    };
    Some(root.join(crate::app::IDENTIFIER))
}

/// The answer a request the server could not be handed gets.
#[must_use]
pub fn refusal(id: &Value, server: &str, why: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": NOT_STARTED,
            "message": format!(
                "MCP server \"{server}\" {why} (ZeroCode starts it on its first call)"
            ),
        },
    })
}

/// A server the stand-in started, while it runs. Its input has a lock of its
/// own: a write that waits on a server busy writing back must not hold the
/// lock the reader of that server needs.
struct Running {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

/// What the stand-in knows; one lock over all of it.
struct State {
    /// What the server answered, from the file or as it answers now.
    listing: Listing,
    /// Whether the listing answers the client while the server is not running.
    answering: bool,
    client_initialize: Option<Value>,
    running: Option<Running>,
    /// Which start the running server is; a stopped one's late words are not
    /// passed on.
    generation: u64,
    /// The last positive id issued to a client request for this server process.
    last_client_id: i32,
    /// The listing requests passed to the server, by server-side id.
    learning: HashMap<i32, String>,
    /// The client's original ids, keyed by the ids used with the server.
    pending: HashMap<i32, Value>,
    /// Whether the session asked the server anything but what it offers.
    used: bool,
    /// A `logging/setLevel` answered while the server was not running, sent
    /// to it when it starts.
    log_level: Option<Value>,
    /// Where the answer to the stand-in's own `initialize` goes.
    handshake: Option<mpsc::Sender<Value>>,
}

/// One stand-in: the server it speaks for, and where it speaks to the client.
pub struct StandIn {
    name: String,
    key: String,
    real: OsString,
    args: Vec<OsString>,
    listings: Option<Listings>,
    idle_after_learning: Duration,
    state: Mutex<State>,
    out: Mutex<Box<dyn Write + Send>>,
}

/// Run the stand-in for `name` over this process's standard streams, in front
/// of `real` with `args`; answers the exit code.
#[must_use]
pub fn serve(name: &str, real: &OsStr, args: &[OsString]) -> i32 {
    let stand_in = Arc::new(StandIn::new(
        name,
        real,
        args,
        Listings::for_this_machine(),
        Box::new(io::stdout()),
    ));
    #[cfg(unix)]
    signals::watch(Arc::clone(&stand_in));
    stand_in.run(io::stdin().lock())
}

impl StandIn {
    /// A stand-in that reads listings from `listings` (none: every session
    /// starts the server at once) and writes the client's side to `out`.
    #[must_use]
    pub fn new(
        name: &str,
        real: &OsStr,
        args: &[OsString],
        listings: Option<Listings>,
        out: Box<dyn Write + Send>,
    ) -> Self {
        let key = listing_key(name, real, args);
        let recalled = listings
            .as_ref()
            .and_then(|listings| listings.recall(&key))
            .filter(|listing| listing.stands(SystemTime::now(), LISTING_STANDS_FOR));
        let answering = recalled.is_some();
        Self {
            name: name.to_string(),
            key,
            real: real.to_os_string(),
            args: args.to_vec(),
            listings,
            idle_after_learning: IDLE_AFTER_LEARNING,
            state: Mutex::new(State {
                listing: recalled.unwrap_or_else(|| Listing::new(name)),
                answering,
                client_initialize: None,
                running: None,
                generation: 0,
                last_client_id: 0,
                learning: HashMap::new(),
                pending: HashMap::new(),
                used: false,
                log_level: None,
                handshake: None,
            }),
            out: Mutex::new(out),
        }
    }

    /// The same, stopping a server started only to learn after `idle`.
    #[must_use]
    pub fn with_idle_after_learning(mut self, idle: Duration) -> Self {
        self.idle_after_learning = idle;
        self
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read the client's messages until it hangs up, then stop the server.
    pub fn run(self: &Arc<Self>, input: impl BufRead) -> i32 {
        for line in input.lines() {
            let Ok(line) = line else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            if !self.from_client(&line) {
                break;
            }
        }
        self.end(None);
        0
    }

    /// One line from the client; `false` once the client cannot be written to.
    fn from_client(self: &Arc<Self>, line: &str) -> bool {
        let Ok(mut message) = serde_json::from_str::<Value>(line) else {
            // Not a message: the server's to make sense of, while it runs.
            self.to_server_text(line);
            return true;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = message.get("id").filter(|id| !id.is_null()).cloned();
        match (method, id) {
            (Some(method), Some(id)) => self.request(&method, id, message),
            (Some(method), None) if method == "notifications/cancelled" => {
                // Cancellation refers to a client request already forwarded
                // to this server. A finished request has no id to cancel.
                let wire_id = message.pointer("/params/requestId").and_then(|id| {
                    self.lock()
                        .pending
                        .iter()
                        .find_map(|(&wire_id, client_id)| (client_id == id).then_some(wire_id))
                });
                if let Some(wire_id) = wire_id {
                    message["params"]["requestId"] = wire_id.into();
                    self.to_server(&message);
                }
                true
            }
            // A notification, or the client's answer to a request of the
            // server's: passed on while the server runs. One that comes while
            // it does not concerns nothing it holds — and
            // `notifications/initialized` is sent by the stand-in itself when
            // it starts the server.
            _ => {
                self.to_server(&message);
                true
            }
        }
    }

    fn request(self: &Arc<Self>, method: &str, id: Value, mut message: Value) -> bool {
        let mut state = self.lock();
        if method == "initialize" {
            let asked = message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str);
            if state.answering && state.listing.protocol.as_deref() != asked {
                // Another protocol than the listing was answered in: the
                // server answers it itself.
                state.answering = false;
            }
            state.client_initialize = Some(message.clone());
        }
        // A later page of a listing is the server's to give: the listing
        // holds the first.
        let paged = message.pointer("/params/cursor").is_some();
        if state.running.is_none() && state.answering && !paged {
            if let Some(answer) = state.listing.answer(method, &id) {
                drop(state);
                return self.to_client(&answer);
            }
            match method {
                "ping" => {
                    drop(state);
                    return self.to_client(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                }
                "logging/setLevel" => {
                    state.log_level = message.get("params").cloned();
                    drop(state);
                    return self.to_client(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                }
                _ => {}
            }
        }
        let start = state.running.is_none();
        let replay = start && method != "initialize" && state.client_initialize.is_some();
        if start {
            state.last_client_id = 0;
        }
        let Some(wire_id) = state.last_client_id.checked_add(1) else {
            drop(state);
            return self.to_client(&refusal(&id, &self.name, "ran out of request ids"));
        };
        state.last_client_id = wire_id;
        // The request is out from here, before any start: a server that is
        // slow to answer its start is not taken for one left unused, and one
        // that ends meanwhile answers it.
        if REMEMBERED.contains(&method) {
            if !paged {
                state.learning.insert(wire_id, method.to_string());
            }
        } else if method != "ping" {
            state.used = true;
        }
        state.pending.insert(wire_id, id.clone());
        drop(state);
        if start && let Err(why) = self.start(replay) {
            if let Some(listings) = &self.listings {
                listings.forget(&self.key);
            }
            let mut state = self.lock();
            state.answering = false;
            let still_waiting = state.pending.remove(&wire_id).is_some();
            state.learning.remove(&wire_id);
            drop(state);
            // The server's end may have answered it already.
            return !still_waiting || self.to_client(&refusal(&id, &self.name, &why));
        }
        message["id"] = wire_id.into();
        let input = self.lock().input();
        if input.is_some_and(|input| write_to(&input, &message)) {
            return true;
        }
        let mut state = self.lock();
        let still_waiting = state.pending.remove(&wire_id).is_some();
        state.learning.remove(&wire_id);
        drop(state);
        // The server's end may have answered it already.
        !still_waiting
            || self.to_client(&refusal(
                &id,
                &self.name,
                "ended before it was handed the request",
            ))
    }

    /// Start the real server. With `replay` the client was answered from the
    /// listing, so the server is first sent the client's own `initialize`.
    fn start(self: &Arc<Self>, replay: bool) -> Result<(), String> {
        let mut child = Command::new(&self.real)
            .args(&self.args)
            .env_remove(MARKER)
            .env_remove(LISTINGS_ENV)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not be started ({})", error.kind()))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("could not be started (no pipe)".to_string());
        };
        let (told, answered) = mpsc::channel();
        let generation = {
            let mut state = self.lock();
            state.generation += 1;
            state.handshake = replay.then_some(told);
            state.running = Some(Running {
                child,
                stdin: Arc::new(Mutex::new(stdin)),
            });
            state.generation
        };
        let reader = Arc::clone(self);
        std::thread::spawn(move || reader.from_server(generation, stdout));
        self.stop_when_unused(generation);
        if !replay {
            return Ok(());
        }
        let (initialize, input) = {
            let state = self.lock();
            (state.client_initialize.clone(), state.input())
        };
        let Some(mut initialize) = initialize else {
            return Ok(());
        };
        initialize["id"] = OWN_INITIALIZE.into();
        let sent = input.is_some_and(|input| write_to(&input, &initialize));
        let answer = if sent {
            answered.recv_timeout(START_LIMIT)
        } else {
            Err(mpsc::RecvTimeoutError::Disconnected)
        };
        let answer = match answer {
            Ok(answer) if answer.get("result").is_some() => answer,
            Ok(_) => {
                self.stop(generation, None);
                return Err("answered its start with an error".to_string());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.stop(generation, None);
                return Err(format!(
                    "did not answer within {} s of its start",
                    START_LIMIT.as_secs()
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.stop(generation, None);
                return Err("ended before it answered".to_string());
            }
        };
        let (level, input) = {
            let mut state = self.lock();
            state.learn("initialize", &answer);
            (state.log_level.take(), state.input())
        };
        if let Some(input) = input {
            write_to(
                &input,
                &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            );
            if let Some(params) = level {
                write_to(
                    &input,
                    &json!({"jsonrpc": "2.0", "id": OWN_LOG_LEVEL, "method": "logging/setLevel", "params": params}),
                );
            }
            // What the server offers now, kept for the next session when it
            // differs from the listing the client was answered from.
            write_to(
                &input,
                &json!({"jsonrpc": "2.0", "id": OWN_RELIST, "method": "tools/list", "params": {}}),
            );
        }
        self.save();
        Ok(())
    }

    /// Read the server started as `generation` until it ends.
    fn from_server(self: Arc<Self>, generation: u64, stdout: impl Read) {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            let Ok(mut message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let mut state = self.lock();
            if state.generation != generation || state.running.is_none() {
                return;
            }
            if message.get("method").is_none()
                && let Some(id) = message.get("id").filter(|id| !id.is_null())
            {
                let Some(wire_id) = id.as_i64().and_then(|id| i32::try_from(id).ok()) else {
                    continue;
                };
                match wire_id {
                    OWN_INITIALIZE => {
                        if let Some(told) = state.handshake.take() {
                            let _ = told.send(message);
                        }
                        continue;
                    }
                    OWN_RELIST => {
                        if message.get("result").is_some()
                            && state.listing.answers.get("tools/list") != Some(&kept(&message))
                        {
                            state.learn("tools/list", &message);
                            drop(state);
                            self.save();
                        }
                        continue;
                    }
                    OWN_LOG_LEVEL => continue,
                    _ => {}
                }
                // The call that triggered replay is pending, but we have not
                // sent it to the server until the replayed initialize answers.
                if state.handshake.is_some() {
                    continue;
                }
                let Some(client_id) = state.pending.remove(&wire_id) else {
                    continue;
                };
                message["id"] = client_id;
                if let Some(method) = state.learning.remove(&wire_id) {
                    state.learn(&method, &message);
                    drop(state);
                    self.save();
                } else {
                    drop(state);
                }
            } else {
                drop(state);
            }
            if !self.to_client(&message) {
                return;
            }
        }
        // The server ended: whatever it was handed and did not answer is
        // answered here, so no call waits on a server that is gone.
        let mut state = self.lock();
        if state.generation != generation {
            return;
        }
        state.handshake = None;
        state.learning.clear();
        let waiting: Vec<Value> = state.pending.drain().map(|(_, id)| id).collect();
        let running = state.running.take();
        drop(state);
        if let Some(running) = running {
            end_child(running, None);
        }
        for id in waiting {
            self.to_client(&refusal(&id, &self.name, "ended before it answered"));
        }
    }

    /// Stop the server started as `generation` after the idle time, when the
    /// session has asked it nothing but what it offers — a server it has
    /// called may hold what a restart would lose (an open page, a login), and
    /// runs on for the session.
    fn stop_when_unused(self: &Arc<Self>, generation: u64) {
        let stand_in = Arc::clone(self);
        let idle = self.idle_after_learning;
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(idle);
                let mut state = stand_in.lock();
                if state.generation != generation || state.running.is_none() || state.used {
                    return;
                }
                // Not yet when a request is out, the server's start is still
                // being answered, or the listing is not whole — the session
                // may still be opening; looked at again later. Looked at and
                // taken under one lock, so a call that comes meanwhile finds
                // the server stopped and starts it again.
                if state.pending.is_empty()
                    && state.handshake.is_none()
                    && state.client_initialize.is_some()
                    && state.listing.complete()
                {
                    let running = state.take_server();
                    state.answering = true;
                    drop(state);
                    if let Some(running) = running {
                        end_child(running, None);
                    }
                    return;
                }
            }
        });
    }

    /// Stop the server started as `generation`: its input closed, `signal`
    /// sent where the platform has them, then ended after [`STOP_GRACE`].
    fn stop(&self, generation: u64, signal: Option<i32>) {
        let running = {
            let mut state = self.lock();
            if state.generation != generation {
                return;
            }
            state.take_server()
        };
        if let Some(running) = running {
            end_child(running, signal);
        }
    }

    /// The client hung up, or the stand-in was told to go.
    pub fn end(&self, signal: Option<i32>) {
        let generation = self.lock().generation;
        self.stop(generation, signal);
    }

    /// Keep the listing, once it can open a session.
    fn save(&self) {
        let listing = {
            let state = self.lock();
            if !state.listing.complete() {
                return;
            }
            state.listing.clone()
        };
        if let Some(listings) = &self.listings {
            let _ = listings.remember(&self.key, &listing);
        }
    }

    fn to_client(&self, message: &Value) -> bool {
        let mut out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        write_line(&mut *out, message)
    }

    fn to_server(&self, message: &Value) {
        let input = self.lock().input();
        if let Some(input) = input {
            write_to(&input, message);
        }
    }

    fn to_server_text(&self, line: &str) {
        let input = self.lock().input();
        if let Some(input) = input {
            let mut input = input.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = writeln!(input, "{line}").and_then(|()| input.flush());
        }
    }
}

impl State {
    /// Take the running server out to be stopped: what it was handed for the
    /// listing is not waited on any more, and its late words are not passed on.
    fn take_server(&mut self) -> Option<Running> {
        self.handshake = None;
        self.learning.clear();
        self.generation += 1;
        self.running.take()
    }

    /// The running server's input.
    fn input(&self) -> Option<Arc<Mutex<ChildStdin>>> {
        self.running
            .as_ref()
            .map(|running| Arc::clone(&running.stdin))
    }

    /// Keep what the server answered to `method`.
    fn learn(&mut self, method: &str, answer: &Value) {
        if method == "initialize" {
            if answer.get("result").is_none() {
                return;
            }
            self.listing.protocol = self
                .client_initialize
                .as_ref()
                .and_then(|asked| asked.pointer("/params/protocolVersion"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        self.listing
            .answers
            .insert(method.to_string(), kept(answer));
        self.listing.listed_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
    }
}

/// An answer's `result` or `error`, the rest left out.
fn kept(answer: &Value) -> Value {
    let mut kept = serde_json::Map::new();
    for field in ["result", "error"] {
        if let Some(value) = answer.get(field) {
            kept.insert(field.into(), value.clone());
        }
    }
    Value::Object(kept)
}

fn write_to(input: &Mutex<ChildStdin>, message: &Value) -> bool {
    let mut input = input.lock().unwrap_or_else(PoisonError::into_inner);
    write_line(&mut *input, message)
}

fn write_line(to: &mut dyn Write, message: &Value) -> bool {
    let Ok(text) = serde_json::to_string(message) else {
        return false;
    };
    writeln!(to, "{text}").and_then(|()| to.flush()).is_ok()
}

/// Close the server's input, send `signal`, and end it if it is still there
/// after [`STOP_GRACE`].
fn end_child(running: Running, signal: Option<i32>) {
    let Running { mut child, stdin } = running;
    drop(stdin);
    #[cfg(unix)]
    if let (Some(signal), Ok(pid)) = (signal, i32::try_from(child.id())) {
        // SAFETY: `kill` takes a pid and a signal number and touches no memory;
        // the child is not reaped yet, so the pid is still its own.
        unsafe { libc::kill(pid, signal) };
    }
    #[cfg(not(unix))]
    let _ = signal;
    let waited_out = |child: &mut Child, limit: Duration| {
        let until = Instant::now() + limit;
        while Instant::now() < until {
            if !matches!(child.try_wait(), Ok(None)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };
    if waited_out(&mut child, STOP_GRACE) {
        return;
    }
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        if waited_out(&mut child, STOP_GRACE) {
            return;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The signals that end a session (Claude Code sends its server `SIGINT`; a
/// closed pane, `SIGHUP`): the server is stopped with the same signal before
/// the stand-in goes.
///
/// The handler writes one byte into a pipe a thread waits on: a stand-in
/// sleeps until a signal comes (a pane holds one per server, a busy machine
/// about a hundred), never waking to look.
#[cfg(unix)]
mod signals {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::StandIn;

    static RECEIVED: AtomicI32 = AtomicI32::new(0);
    static WAKE: AtomicI32 = AtomicI32::new(-1);

    extern "C" fn received(number: libc::c_int) {
        RECEIVED.store(number, Ordering::SeqCst);
        let wake = WAKE.load(Ordering::SeqCst);
        if wake >= 0 {
            let byte = 1_u8;
            // SAFETY: write(2) is async-signal-safe; it reads one byte from
            // this frame and touches nothing else.
            unsafe { libc::write(wake, (&raw const byte).cast(), 1) };
        }
    }

    pub(super) fn watch(stand_in: Arc<StandIn>) {
        let mut ends: [libc::c_int; 2] = [0; 2];
        // SAFETY: pipe(2) fills the two descriptors of the array it is handed.
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            // No pipe: the signals keep their default and end the stand-in;
            // its server then reads the end of its input.
            return;
        }
        for end in ends {
            // SAFETY: fcntl(2) on a descriptor this process just opened; the
            // server the stand-in starts is not handed either end.
            unsafe { libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        WAKE.store(ends[1], Ordering::SeqCst);
        for number in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let handler = received as extern "C" fn(libc::c_int);
            // SAFETY: the handler only stores into an atomic and writes to a
            // pipe, both async-signal-safe; a handler (not an ignore) is reset
            // to the default in the server the stand-in starts.
            unsafe { libc::signal(number, handler as libc::sighandler_t) };
        }
        std::thread::spawn(move || {
            let mut byte = 0_u8;
            loop {
                // SAFETY: read(2) of one byte into this frame.
                let read = unsafe { libc::read(ends[0], (&raw mut byte).cast(), 1) };
                let number = RECEIVED.load(Ordering::SeqCst);
                if number != 0 {
                    stand_in.end(Some(number));
                    std::process::exit(0);
                }
                if read == 0
                    || (read < 0
                        && std::io::Error::last_os_error().kind()
                            != std::io::ErrorKind::Interrupted)
                {
                    return;
                }
            }
        });
    }
}

#[cfg(all(test, unix))]
mod tests;
