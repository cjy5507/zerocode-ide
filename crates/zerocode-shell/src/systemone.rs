//! The window's System One wire: the one socket every question the window
//! asks Jev goes through — which control a walk presses next, on a page or on
//! the desktop (`computer_use::errand::live`), and a quiet worker's cause
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
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use zerocode_core::jev::door::{
    self, JevSettings, Memo, Memoed, Passed, REDACTED_LINES_KEY, REQUESTS_KEY, Refused,
};
use zerocode_core::jev::summary::MODEL;
use zerocode_core::jev::{JevUse, count, memo};

use crate::api_routers::{Keychain, RouterKeys};

/// The public endpoint, its route and the model the SDKs call by default —
/// the same three words zo's client holds, because they are the vendor's.
/// The model is the pin's default (`smart.jevModel`, t-6187): a body is
/// built with it, and the door writes the person's pin over it
/// ([`door::may_send`]).
pub const SYSTEMONE_BASE_URL: &str = "https://api.typesafe.ai";
pub const SYSTEMONE_PATH: &str = "/v1/systemone";
pub const SYSTEMONE_MODEL: &str = zerocode_core::jev::DEFAULT_MODEL;

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

/// How long a warm-up connect may take before it is abandoned — the cold
/// first ask the bench timed, rounded up; a warm-up that outlives a walk's
/// first question was no help and is not worth a socket.
const ACTION_WARM_TIMEOUT: Duration = Duration::from_millis(2_000);

/// How long the pool keeps a socket nobody is using — reqwest's own default,
/// but named and set on the client below rather than inherited, so the window
/// a door skips a warm-up inside and the window the socket actually survives
/// are one number and cannot drift apart.
const POOL_IDLE: Duration = Duration::from_secs(90);

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
        (door::WIRE_MODEL_KEY): SYSTEMONE_MODEL,
        "questions": questions,
    })
}

/// The version an answer's body says answered it — its own `model`
/// (`jev-1.13.0` for a request that asked `jev-latest`) — or `None` for a
/// body that is not an answer or names none. Read by the wire and by the
/// memo's rows alike, so a remembered answer names the version that gave it.
#[must_use]
pub fn answered_by(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get(door::WIRE_MODEL_KEY)?
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

/// Where one use's rows live: under zo's config home, in the folder the Jev
/// door counts the day in — beside the settings that switch them on.
///
/// One producer for every use the WINDOW asks, so the folder a reader looks
/// in and the folder the door counts in cannot drift apart per seat.
#[must_use]
pub fn ledger_of(wire: &Wire, row: &JevUse) -> Option<PathBuf> {
    Some(
        wire.config_home()?
            .join(count::REQUESTS_DIR)
            .join(row.ledger),
    )
}

/// Where the judgment memo lives: one file beside the seats' ledgers and the
/// day's count, so a reader finds every Jev artifact in one folder and a
/// memo written by one walk is read by the next, restart or not
/// ([`zerocode_core::jev::memo`], t-6132).
#[must_use]
pub fn memo_path(wire: &Wire) -> Option<PathBuf> {
    Some(
        wire.config_home()?
            .join(count::REQUESTS_DIR)
            .join(memo::MEMO_FILE),
    )
}

/// Append `rows` to one use's ledger, and judge the seat on them when a
/// judgment is due (§4): every twenty requests, or at once for an acting seat
/// that has fallen back three times running. A rise or a fall is written
/// beside the rows it was decided on, so the seat's standing and the rows it
/// was earned on cannot be found apart — the same road zo's routing seat
/// walks (`decision_shadow::judge_ledger`).
///
/// This is the one road a window seat's rows take: the writer that appended
/// is the one that judges, so a seat that only ever recorded could not have
/// been left at `auto` with nothing deciding (2026-09-20, "전부 자동 기록하며
/// 실제 적용되어야").
pub fn record_rows(seat: &JevUse, ledger: &Path, rows: &[Value], now_ms: i64) {
    append_rows(ledger, rows);
    if !seat.promotes {
        return;
    }
    let held = read_rows(ledger);
    if !zerocode_core::jev::promote::judgment_due(seat, &held) {
        return;
    }
    let Some(judged) = zerocode_core::jev::promote::judge_seat(seat, &held) else {
        return;
    };
    if let Some(row) =
        zerocode_core::jev::promote::transition_row(now_ms, judged.verdict, &judged.window)
    {
        append_rows(ledger, std::slice::from_ref(&row));
    }
}

/// Every row of one use's ledger, newest last — the same reader zo's counter
/// uses, so a judgment here and a number on the screen read one file one way.
#[must_use]
pub fn read_rows(ledger: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(ledger) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Whether a seat acts right now: a person's `on`, or `auto` raised by the
/// judge its own ledger recorded (§4). Read where the seat is about to act,
/// so the answer and the standing come from the same file.
#[must_use]
pub fn applies(wire: &Wire, seat: &JevUse) -> bool {
    let mode = seat.mode_in(&wire.settings_root());
    let raised = ledger_of(wire, seat)
        .map(|ledger| {
            zerocode_core::jev::promote::stand_from(&read_rows(&ledger))
                == zerocode_core::jev::promote::Stand::Applying
        })
        .unwrap_or(false);
    mode.applies_with(raised)
}

/// Append `rows` to one use's ledger.
///
/// A ledger that will not take them is said once on stderr and never raised:
/// a record of a decision is not worth failing the decision. Which record is
/// named by the ledger file's own stem, so the sentence cannot name one use
/// while the bytes go to another.
pub fn append_rows(ledger: &Path, rows: &[Value]) {
    if rows.is_empty() {
        return;
    }
    let mut said = String::new();
    for row in rows {
        said.push_str(&row.to_string());
        said.push('\n');
    }
    let written = ledger
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(ledger)
        })
        .and_then(|mut file| std::io::Write::write_all(&mut file, said.as_bytes()));
    if let Err(why) = written {
        let what = ledger.file_stem().map_or_else(
            || ledger.to_string_lossy(),
            std::ffi::OsStr::to_string_lossy,
        );
        eprintln!("orchestration: the {what} record was not written: {why}");
    }
}

/// The origin to ask, honouring the test override.
fn base_url() -> String {
    std::env::var(SYSTEMONE_BASE_URL_ENV)
        .ok()
        .map(|given| given.trim().to_string())
        .filter(|given| !given.is_empty())
        .unwrap_or_else(|| SYSTEMONE_BASE_URL.to_string())
}

/// What asking came to at the Jev door: the requests it sent, the lines the
/// door withheld from them, and the version that answered — what every Jev
/// ledger row the window writes carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Spent {
    pub requests: u32,
    pub redacted_lines: usize,
    /// The version the answer named ([`answered_by`], t-6187); `None` when
    /// nothing answered — a refusal at the door, a failure, a wall.
    pub model: Option<String>,
}

impl Spent {
    /// Write this ask's account onto `row`: the door's two counts and, when
    /// an answer named one, the version that answered, under the key table's
    /// spelling ([`MODEL`]).
    ///
    /// The one writer of those keys for every seat the window asks — each
    /// seat writes its own words and hands its row here — so a seat cannot
    /// forget the version, and a row nothing answered carries none.
    pub fn stamp(&self, row: &mut Value) {
        let Some(fields) = row.as_object_mut() else {
            return;
        };
        fields.insert(REQUESTS_KEY.to_string(), json!(self.requests));
        fields.insert(REDACTED_LINES_KEY.to_string(), json!(self.redacted_lines));
        if let Some(model) = self.model.as_deref() {
            fields.insert(MODEL.canonical.to_string(), json!(model));
        }
    }

    /// What several asks of one judgment came to together — a sharded
    /// question's requests side by side: their requests and withheld lines
    /// added, and the version the first answer among them named.
    #[must_use]
    pub fn together<'spent>(asks: impl IntoIterator<Item = &'spent Self>) -> Self {
        asks.into_iter().fold(Self::default(), |mut all, one| {
            all.requests += one.requests;
            all.redacted_lines += one.redacted_lines;
            if all.model.is_none() {
                all.model.clone_from(&one.model);
            }
            all
        })
    }
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
    /// What the memo said, when one was asked ([`Wire::ask_remembering`]):
    /// `None` for a question asked without one, or one the door refused
    /// before the memo was reached. When `memo.answered`, `answer` is the
    /// memo's and `spent.requests` is `0`: nothing left the machine.
    pub memo: Option<Memoed>,
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
        memo: Option<Memo<'_>>,
    ) -> Result<Passed, Refused> {
        let settings = JevSettings::from_root(&self.settings_root()).resolved();
        let workspace = workspace.map(door::resolved_path);
        let requests = self
            .config_home()
            .map(|home| count::requests_path(home, &today()))
            .unwrap_or_default();
        door::pass_remembering(
            |asking| door::may_send(row, asking, body),
            key,
            &settings,
            workspace.as_deref(),
            &requests,
            memo,
        )
    }

    /// Open the endpoint's connection ahead of the first question, off the
    /// caller's thread: a walk's first look takes longer than a handshake,
    /// and a first ask that pays DNS, TCP and TLS on top of the answer ran
    /// past its 1,500 ms deadline on the bench (round 1: 1,504 ms refused,
    /// rounds 2–10: p50 280 ms — 2026-09-21, §2 of
    /// docs/design/jev-seats-accuracy-wave-20260921.md). No key is sent: the
    /// request is a bare GET of the base, whose answer is thrown away; what
    /// it leaves behind is a pooled socket the POST reuses. Without a key
    /// nothing is sent, as nothing would be asked.
    ///
    /// Called from every door a walk may start behind ([`warm_for_walks`]),
    /// so most calls find the socket an earlier door left and send nothing:
    /// the pool's own idle window decides (`warm_due`). The key is read
    /// before that window is touched, so a keyless window — which sends
    /// nothing — also remembers nothing, and the first warm-up after a key
    /// is finally typed is not skipped as though it had already run.
    pub fn warm(&self) {
        if self.key().is_none() {
            return;
        }
        let Some(client) = client() else {
            return;
        };
        let base = self.base.trim_end_matches('/').to_string();
        if !warm_due(&base) {
            return;
        }
        tauri::async_runtime::spawn(async move {
            let _ = client.get(&base).timeout(ACTION_WARM_TIMEOUT).send().await;
        });
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
        self.ask_remembering(row, workspace, body, deadline, None)
    }

    /// [`Self::ask`], with a memo between the door and the socket
    /// ([`door::pass_remembering`], t-6132): the door's four questions, then
    /// the lookup of the cleared bytes, then — for a hit under a seat that
    /// may answer from it — the memo's answer with no request sent and none
    /// counted; otherwise one POST as ever, with what the memo held riding
    /// beside the answer so the caller can compare the two and remember the
    /// fresh one ([`memo::remember`]).
    #[must_use]
    pub fn ask_remembering(
        &self,
        row: &JevUse,
        workspace: Option<&Path>,
        body: Value,
        deadline: Duration,
        memo: Option<Memo<'_>>,
    ) -> Asked {
        let deadline = Instant::now() + deadline;
        let refused = |refusal: Refused| Asked {
            answer: Err(refusal.token().to_string()),
            spent: Spent::default(),
            request_bytes: 0,
            memo: None,
        };
        let key = self.key();
        let passed = match self.pass(row, key.is_some(), workspace, body, memo) {
            Ok(passed) => passed,
            Err(refusal) => return refused(refusal),
        };
        // The door refuses a keyless request before anything else it asks.
        let Some(key) = key else {
            return refused(Refused::NoKey);
        };
        let Passed { cleared, memo } = passed;
        let request_bytes = cleared.bytes().len();
        let redacted_lines = cleared.withheld_lines();
        if let Some(remembered) = memo
            .as_ref()
            .filter(|memoed| memoed.answered)
            .and_then(|memoed| memoed.recalled.as_ref())
        {
            return Asked {
                answer: Ok(remembered.answer.clone()),
                spent: Spent {
                    requests: 0,
                    redacted_lines,
                    // The version that gave the remembered answer, read off
                    // the body the memo kept whole.
                    model: answered_by(&remembered.answer),
                },
                request_bytes,
                memo,
            };
        }
        // Every caller is sync — a walk drives sync roads, a question asked
        // off the beat has a thread of its own — and blocks on the window's
        // runtime the same way.
        let answer = tauri::async_runtime::block_on(self.ask_once(&key, cleared, deadline));
        let spent = Spent {
            requests: 1,
            redacted_lines,
            model: answer.as_deref().ok().and_then(answered_by),
        };
        Asked {
            answer,
            spent,
            request_bytes,
            memo,
        }
    }

    /// One call carrying the door's bytes, bounded whole by `deadline`.
    async fn ask_once(
        &self,
        key: &str,
        cleared: door::Cleared,
        deadline: Instant,
    ) -> Result<String, String> {
        if Instant::now() >= deadline {
            return Err(TIMEOUT.to_string());
        }
        let client = client().ok_or_else(|| TRANSPORT.to_string())?;
        let url = format!("{}{SYSTEMONE_PATH}", self.base.trim_end_matches('/'));
        // Key/consent checks, runtime startup and client construction spend
        // this call's budget too; the socket never starts a fresh deadline.
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| TIMEOUT.to_string())?;
        let answer = client
            .post(&url)
            .timeout(remaining)
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

/// Open the socket a walk's first question will ride, from a door the walk
/// has not reached yet: the window's boot, a browser pane, a device stream.
///
/// A walk's own judge warms the wire too (`errand::live::LiveJudge::new`),
/// and after a restart that is too late: the first look of the first walk is
/// 100–300 ms and the handshake it would have to hide is longer, so on the
/// installed 1.1.9 that first question cost 554 ms against the 242–290 ms of
/// the ones behind it (§2.2 of
/// docs/design/jev-seats-accuracy-wave-20260921.md). The doors below open
/// long before a walk is asked for, and what they call is the judge's own
/// warm-up through the judge's own wire — one warm-up in the window, not a
/// second kind of one.
///
/// On a thread of its own, because the key comes from the keychain through
/// the `security` command (`accounts::read_keychain_service`) — a process
/// spawn — and a door that waits on one has turned a warm-up into a delay.
pub fn warm_for_walks() {
    warm_off_thread(Wire::of_this_machine());
}

/// The body every door walks, with the wire handed in rather than read from
/// this machine, so a test can point the same road at a loopback endpoint.
fn warm_off_thread(wire: Wire) {
    std::thread::spawn(move || wire.warm());
}

/// Whether a warm-up to `origin` would buy anything, and a record that it is
/// about to if it would.
///
/// The pool keeps an idle socket for [`POOL_IDLE`]; a second warm-up inside
/// that window pays a request for a connection the client already holds, and
/// a window whose boot, browser pane and device stream all open within a
/// minute would pay it three times. Keyed by origin because the pool is
/// keyed by origin — a socket opened to one endpoint is no help to another,
/// which is what the test override points the wire at. A row older than the
/// window names a socket the pool has already dropped and is dropped with
/// it, so this holds one row per origin warmed in the last [`POOL_IDLE`].
fn warm_due(origin: &str) -> bool {
    static WARMED: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
    let mut warmed = WARMED.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    warmed.retain(|(_, at)| now.duration_since(*at) < POOL_IDLE);
    if warmed.iter().any(|(seen, _)| seen == origin) {
        return false;
    }
    warmed.push((origin.to_string(), now));
    true
}

/// The one HTTP client every question the window asks goes through: its
/// connection pool keeps the endpoint's TLS session alive between asks, so
/// a walk's second question rides the first's socket instead of opening its
/// own. A client per ask was a handshake per ask — see §2 of
/// docs/design/jev-seats-accuracy-wave-20260921.md for the bench that timed
/// both on the same look. Its idle window is [`POOL_IDLE`], named rather
/// than inherited so a warm-up can be skipped on the same number the socket
/// lives by. `None` only when the client cannot be built at all, which the
/// ask refuses as `transport`.
fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .pool_idle_timeout(POOL_IDLE)
                .build()
                .ok()
        })
        .as_ref()
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
