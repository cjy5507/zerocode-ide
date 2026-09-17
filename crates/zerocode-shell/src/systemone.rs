//! The window's System One wire: the one socket every question the window
//! asks Jev goes through — a stopped browser walk's recovery
//! (`computer_use::recover::live`) and a quiet worker's cause
//! (`orchestration::stall_cause`).
//!
//! zo has a System One client of its own (`zo-ide/crates/api/src/systemone.rs`)
//! and this is deliberately NOT that one. The two Cargo workspaces depend one
//! way — zo-ide reads this repository's crates, never the reverse — so the
//! window cannot call it, and moving it down here would put a second `reqwest`
//! major in zo-ide's lock (docs/design/jev-browser-action-20260917.md §1.4).
//! What must not fork is the CONTRACT, and it does not: each question, its
//! answer space and every validation rule come from `zerocode_core`, which
//! both sides read. This file holds a socket, a deadline and a table of
//! failure words, and nothing else.
//!
//! The key is the one the settings pane already keeps
//! (`crate::typesafe_settings::typesafe_keychain_service`). Without it nothing
//! is sent: no key is an answer (`no_key`), not an error.
//!
//! Every request passes the Jev door zo's requests pass
//! (`zerocode_core::jev::door`) — a key, the switch, the person's consent for
//! the workspace the words come from, the day's budget — and what is POSTed is
//! the door's bytes: the words with every line that may carry a credential
//! withheld, cut to the use's caps. The door's facts are read from zo's own
//! settings file, the one the settings pane writes, whose folder also keeps
//! the day's count.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use zerocode_core::jev::door::{self, Cleared, JevSettings, Refused};
use zerocode_core::jev::{JevUse, count};

use crate::api_routers::{Keychain, RouterKeys};

/// The public endpoint, its route and the model the SDKs call by default —
/// the same three words zo's client holds, because they are the vendor's.
pub const SYSTEMONE_BASE_URL: &str = "https://api.typesafe.ai";
pub const SYSTEMONE_PATH: &str = "/v1/systemone";
pub const SYSTEMONE_MODEL: &str = "jev-latest";

/// Test and diagnostic origin override, for the same reason zo's client has
/// one: only a call that crosses a real socket catches a wire-shaped failure.
/// The key is still required.
pub const SYSTEMONE_BASE_URL_ENV: &str = "ZO_SYSTEMONE_BASE_URL";

/// The wire's failure words, the whole closed table. `http` grows its status
/// only in a ledger row (`http_503`), never in a counter. A request that never
/// reached the wire — no key among them — is the Jev door's to name
/// (`door::Refused`).
pub const UNAUTHORIZED: &str = "unauthorized";
pub const INVALID_REQUEST: &str = "invalid_request";
pub const RATE_LIMITED: &str = "rate_limited";
pub const OVERLOADED: &str = "overloaded";
pub const TRANSPORT: &str = "transport";
pub const TIMEOUT: &str = "timeout";
pub const SCHEMA: &str = "schema";

/// The contract's "overloaded" status; it is not in the HTTP registry.
const OVERLOADED_STATUS: u16 = 529;

/// The word a status is refused with. Unlike zo's client this wire does not
/// retry: a browser recovery is already inside a stopped walk's budget, and a
/// quiet worker's question is asked once per silence and recorded either way.
#[must_use]
pub fn token_for(status: u16) -> String {
    match status {
        401 => UNAUTHORIZED.to_string(),
        422 => INVALID_REQUEST.to_string(),
        429 => RATE_LIMITED.to_string(),
        OVERLOADED_STATUS => OVERLOADED.to_string(),
        other => format!("http_{other}"),
    }
}

/// A question's request body, as the endpoint takes it.
#[must_use]
pub fn request_body(state: &Value, questions: &Value) -> Value {
    json!({
        "state": state,
        "model": SYSTEMONE_MODEL,
        "questions": questions,
    })
}

/// The origin to ask, honouring the test override.
fn base_url() -> String {
    std::env::var(SYSTEMONE_BASE_URL_ENV)
        .ok()
        .map(|given| given.trim().to_string())
        .filter(|given| !given.is_empty())
        .unwrap_or_else(|| SYSTEMONE_BASE_URL.to_string())
}

/// What asking cost at the Jev door: the requests it sent, and the lines the
/// door withheld from them — the two numbers every Jev ledger row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spent {
    pub requests: u32,
    pub redacted_lines: usize,
}

/// What one question came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The endpoint's body, or the token of why there is none — a door's
    /// refusal or one of this wire's failure words.
    pub answer: Result<String, String>,
    pub spent: Spent,
    /// The bytes the request carried, `0` when the door refused it.
    pub request_bytes: usize,
}

/// Where the key is read from.
#[derive(Clone)]
enum KeySource {
    /// Read already, when the wire was made.
    Read(Option<String>),
    /// This machine's keychain, read only when a question is asked — a beat
    /// that builds a wire and asks nothing never touches the keychain.
    Keychain,
}

/// The wire: where the key comes from, the origin, and the person's settings
/// file the door reads.
#[derive(Clone)]
pub struct Wire {
    keys: KeySource,
    base: String,
    /// zo's settings file — the person's own, which the settings pane writes.
    /// Its folder keeps the day's count. `None` when no home resolves, which
    /// consents to nothing.
    settings: Option<PathBuf>,
}

/// A key as the wire keeps it: trimmed, and empty is none.
fn trimmed(key: Option<String>) -> Option<String> {
    key.map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

impl Wire {
    /// A wire holding the key `keys` has now. An unreadable keychain and an
    /// empty item are both "no key": nothing is sent either way.
    #[must_use]
    pub fn new(keys: &dyn RouterKeys) -> Self {
        Self {
            keys: KeySource::Read(trimmed(
                keys.read(&crate::typesafe_settings::typesafe_keychain_service())
                    .ok()
                    .flatten(),
            )),
            base: base_url(),
            settings: crate::api_routers::zo_settings_path(),
        }
    }

    /// This machine's wire, reading nothing yet: the keychain when a question
    /// is asked, the settings file when the door or a switch is read.
    #[must_use]
    pub fn of_this_machine() -> Self {
        Self {
            keys: KeySource::Keychain,
            base: base_url(),
            settings: crate::api_routers::zo_settings_path(),
        }
    }

    /// A wire pointed at one origin with one key and one settings file — how a
    /// test crosses a real socket, with the origin and the settings passed
    /// rather than read from the environment so two tests never race over it.
    #[cfg(test)]
    #[must_use]
    pub fn at(base: &str, key: &str, settings: Option<PathBuf>) -> Self {
        Self {
            keys: KeySource::Read(trimmed(Some(key.to_string()))),
            base: base.trim().to_string(),
            settings,
        }
    }

    /// The key, read now.
    #[must_use]
    pub fn key(&self) -> Option<String> {
        match &self.keys {
            KeySource::Read(key) => key.clone(),
            KeySource::Keychain => trimmed(
                Keychain::of_this_machine()
                    .read(&crate::typesafe_settings::typesafe_keychain_service())
                    .ok()
                    .flatten(),
            ),
        }
    }

    /// Whether this wire can ask at all.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.key().is_some()
    }

    /// The person's settings document, read now — `Null` when there is no
    /// file or it does not read, which switches every use off and consents
    /// to nothing.
    #[must_use]
    pub fn settings_root(&self) -> Value {
        self.settings
            .as_deref()
            .and_then(|path| crate::api_routers::read_zo_settings_root(path).ok())
            .map_or(Value::Null, Value::Object)
    }

    /// zo's config home: the folder the settings file sits in, where the door
    /// counts the day and a use the window asks keeps its ledger.
    #[must_use]
    pub fn config_home(&self) -> Option<&Path> {
        self.settings.as_deref().and_then(Path::parent)
    }

    /// Ask the door about one request of `row`'s, its facts read now: the
    /// person's settings, the workspace, the day's count.
    fn pass(
        &self,
        row: &JevUse,
        key: bool,
        workspace: Option<&Path>,
        body: Value,
    ) -> Result<Cleared, Refused> {
        let settings = JevSettings::from_root(&self.settings_root()).resolved();
        let workspace = workspace.map(door::resolved_path);
        let requests = self
            .config_home()
            .map(|home| count::requests_path(home, &today()))
            .unwrap_or_default();
        door::pass(
            |asking| door::may_send(row, asking, body),
            key,
            &settings,
            workspace.as_deref(),
            &requests,
        )
    }

    /// One question of `row`'s about words from `workspace`, whole: the door,
    /// then one POST of the door's bytes bounded by `deadline`. Blocks — a
    /// caller that must not wait asks from a thread of its own.
    #[must_use]
    pub fn ask(
        &self,
        row: &JevUse,
        workspace: Option<&Path>,
        body: Value,
        deadline: Duration,
    ) -> Asked {
        let refused = |refusal: Refused| Asked {
            answer: Err(refusal.token().to_string()),
            spent: Spent {
                requests: 0,
                redacted_lines: 0,
            },
            request_bytes: 0,
        };
        let key = self.key();
        let cleared = match self.pass(row, key.is_some(), workspace, body) {
            Ok(cleared) => cleared,
            Err(refusal) => return refused(refusal),
        };
        // The door refuses a keyless request before anything else it asks.
        let Some(key) = key else {
            return refused(Refused::NoKey);
        };
        let spent = Spent {
            requests: 1,
            redacted_lines: cleared.withheld_lines(),
        };
        let request_bytes = cleared.bytes().len();
        // Every caller is sync — a walk drives sync roads, a question asked
        // off the beat has a thread of its own — and blocks on the window's
        // runtime the same way.
        let answer = tauri::async_runtime::block_on(self.ask_once(&key, cleared, deadline));
        Asked {
            answer,
            spent,
            request_bytes,
        }
    }

    /// One call carrying the door's bytes, bounded whole by `deadline`.
    async fn ask_once(
        &self,
        key: &str,
        cleared: Cleared,
        deadline: Duration,
    ) -> Result<String, String> {
        let client = reqwest::Client::builder()
            .timeout(deadline)
            .build()
            .map_err(|_| TRANSPORT.to_string())?;
        let url = format!("{}{SYSTEMONE_PATH}", self.base.trim_end_matches('/'));
        let answer = client
            .post(&url)
            .header("Authorization", format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .body(cleared.into_bytes())
            .send()
            .await
            .map_err(|err| failure(&err))?;
        let status = answer.status();
        if !status.is_success() {
            return Err(token_for(status.as_u16()));
        }
        // The body is read inside the same deadline the request was given.
        answer.text().await.map_err(|err| failure(&err))
    }
}

/// The word a request that never answered is refused with.
fn failure(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        TIMEOUT.to_string()
    } else {
        TRANSPORT.to_string()
    }
}

/// The local day, on this machine's clock, a request is counted in.
fn today() -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        });
    let offset_minutes =
        i32::try_from(crate::automation_runtime::local_offset_secs() / 60).unwrap_or(0);
    count::day_of(now_ms, offset_minutes)
}

#[cfg(test)]
pub(crate) mod tests;
