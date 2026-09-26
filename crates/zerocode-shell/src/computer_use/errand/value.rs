//! The value a walk types (t-6720): written by the value seat's chosen row
//! (`zerocode_core::type_value`) only once the walk's judgment chose to type
//! into a field, typed again from memory when a retry or a replay asks for
//! the very same write, and handed to the world's typing road alone — never
//! to a row, a log, or an argv a log keeps.
//!
//! Jev chooses; it does not write. So the one string a goal walk needs is
//! asked of a small model, and which model, down which road, is the seat's
//! table ([`zerocode_core::type_value::chosen_on`]) — nothing here spells
//! one. Which road is the person's choice in the Computer Use pane
//! (`computer_generator_road`, t-10372, [`Setup`]):
//!
//! - **a login** — the person's Claude Code or Codex sign-in, spent the one
//!   way this product spends one: the vendor's own CLI, run once headless
//!   under the account the window's panes run as ([`crate::scm_runtime::run_once`]).
//!   This product never reads, holds or sends the login, and speaks for no
//!   client it is not (the coordinator's decision m-9526).
//! - **a person's own API key**, put in the window's key store for the row
//!   ([`ValueRow::credential_key`], `dev.zerocode.key.<name>`, or the item a
//!   router row names), asked only when they chose it.
//!
//! `auto`, the road a person who never chose is on, asks Claude's login and
//! then Codex's. Every answer says which road gave it and why each road
//! before it could not ([`Answered`]) — on the walk's row, on the autopilot's
//! plan row and on the pane's card ([`last_answered`]) — so a road passed
//! over is never a quiet one. When no road can answer, the refusal names
//! every road's reason, and the walk hands the field back.
//!
//! What makes two writes the same write is the core's
//! ([`zerocode_core::type_value::identity`]): the question's version, the
//! model, the goal, the words around the box, what the box holds, the
//! document and the box itself. [`Values`] keeps what was written under that
//! identity, for as many writes as the longest walk the verb allows can make
//! ([`zerocode_core::computer_use::WALK_STEPS_MAX`]); anything else asks
//! afresh.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use zerocode_core::type_value::{ANTHROPIC_WIRE, FieldLook, GeneratorRoad, Road, ValueRow};
use zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX;

use crate::api_routers::{Keychain, RouterKeys};
use crate::quota_wall::StallCause;
use crate::scm_runtime::{Once, OnceFailure};
use crate::systemone::{SCHEMA, TIMEOUT, TRANSPORT, token_for};

/// Why a writer wrote nothing, beside the wire's own words
/// (`crate::systemone`) and a value the seat's rules refused
/// ([`zerocode_core::type_value::ValueRefusal::token`]): the table names no
/// row; the person put no key in the window's key store for it; its road is
/// one this product does not take (it would have to speak as another client,
/// or it is not built).
pub const NO_ROW: &str = "value_no_row";
pub const NO_KEY: &str = "value_no_key";
pub const ROAD_UNSUPPORTED: &str = "value_road_unsupported";

/// Why a login road wrote nothing (t-10372): the vendor's CLI is not on this
/// machine; it said its login is what stopped it; it stood at its quota
/// wall; it refused in words nobody measured. And the person chose no road.
pub const NO_CLI: &str = "value_no_cli";
pub const NO_LOGIN: &str = "value_no_login";
pub const QUOTA_WALL: &str = "quota_wall";
pub const CLI_REFUSED: &str = "value_cli_refused";
pub const GENERATOR_OFF: &str = "value_generator_off";

/// What a window's writer is built from (t-10372): the road the person chose
/// in the Computer Use pane (`computer_generator_road`), and where the window
/// keeps the accounts its panes run as — read when a road is asked, so an
/// account switched between two walks is the next walk's account.
#[derive(Debug, Clone)]
pub struct Setup {
    pub road: GeneratorRoad,
    pub config_root: PathBuf,
    pub local_data_root: PathBuf,
    /// The program each login road runs instead of the one this machine's
    /// catalogue detects — a test's script. Empty in the window.
    pub programs: Vec<(Road, String)>,
}

impl Setup {
    #[must_use]
    pub const fn new(road: GeneratorRoad, config_root: PathBuf, local_data_root: PathBuf) -> Self {
        Self {
            road,
            config_root,
            local_data_root,
            programs: Vec::new(),
        }
    }

    /// The CLI a login road runs: a test's, else the one the catalogue found.
    fn program(&self, road: Road) -> Option<String> {
        if let Some((_, program)) = self.programs.iter().find(|(held, _)| *held == road) {
            return Some(program.clone());
        }
        found_program(road)
    }

    /// The account a login road's CLI runs as — the env the window's panes
    /// get for that agent, from the functions every launch reads it from.
    fn account_env(&self, road: Road) -> Result<Vec<(String, String)>, String> {
        match road {
            Road::ClaudeCli => crate::scm_runtime::claude_reading_env(&self.config_root),
            Road::CodexCli => {
                let mut env = crate::usage_runtime::account_env_for(&self.config_root, "codex")?;
                env.extend(crate::hooks::agent_launch_env(
                    &self.local_data_root,
                    "codex",
                ));
                Ok(env)
            }
            Road::Anthropic | Road::CodeAssist | Road::OpenaiCompat => Ok(Vec::new()),
        }
    }
}

/// The directory a login road's CLI runs in: an empty one of this
/// product's own. A CLI reads the directory it starts in — its project's
/// instructions, its remembered notes — and none of that is the question:
/// one value asked from a checkout carried 17,194 input tokens, the same
/// value from an empty directory 501 (2026-09-26, the Claude login).
fn one_shot_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("zerocode-one-shot");
        std::fs::create_dir_all(&dir).ok().map(|()| dir)
    })
    .clone()
}

/// The CLI this machine's catalogue found for a login road's row.
fn found_program(road: Road) -> Option<String> {
    match road {
        Road::ClaudeCli => crate::usage_runtime::claude_program(),
        Road::CodexCli => crate::usage_runtime::agent_program("codex"),
        Road::Anthropic | Road::CodeAssist | Road::OpenaiCompat => None,
    }
}

/// Whether the CLI a login road asks is on this machine — what the pane says
/// beside the road.
#[must_use]
pub fn cli_found(road: GeneratorRoad) -> bool {
    zerocode_core::type_value::chosen_on(road).is_some_and(|row| found_program(row.road).is_some())
}

/// Which road gave an answer, the model that wrote it, and why each road
/// asked before it gave none — `road=reason`, in the order they were asked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answered {
    /// [`GeneratorRoad::word`] of the road that answered; empty for none.
    pub road: &'static str,
    pub model: String,
    pub passed: Vec<String>,
}

impl Answered {
    /// The fields a row carries it under: `road`, and `passedOver` when a
    /// road was passed over.
    #[must_use]
    pub fn note(&self) -> Value {
        let mut note = json!({ "road": self.road });
        if !self.passed.is_empty()
            && let Some(fields) = note.as_object_mut()
        {
            fields.insert("passedOver".to_string(), json!(self.passed));
        }
        note
    }
}

/// What the last question any writer in this window asked came to — the
/// pane's 「마지막으로 답한 길」: the road that answered (empty when none did),
/// every road passed over and why, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastAnswer {
    #[serde(flatten)]
    pub answered: Answered,
    pub at_ms: i64,
}

fn last() -> &'static Mutex<Option<LastAnswer>> {
    static LAST: Mutex<Option<LastAnswer>> = Mutex::new(None);
    &LAST
}

/// The last answer, if this window asked anything yet.
#[must_use]
pub fn last_answered() -> Option<LastAnswer> {
    last()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn remember(answered: &Answered) {
    *last().lock().unwrap_or_else(PoisonError::into_inner) = Some(LastAnswer {
        answered: answered.clone(),
        at_ms: crate::project_runtime::now_epoch_ms(),
    });
}

/// `road=reason`, the way every row names a road passed over.
fn passed_word(road: GeneratorRoad, why: &str) -> String {
    format!("{}={why}", road.word())
}

/// What a writer says when no road answered: the one road's reason when the
/// person chose one road, every road's when `auto` asked two.
fn nobody(passed: &[(GeneratorRoad, String)]) -> String {
    match passed {
        [(_, why)] => why.clone(),
        _ => passed
            .iter()
            .map(|(road, why)| passed_word(*road, why))
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// Whether a road that gave no answer is one `auto` passes over: every
/// reason a road cannot answer — no CLI, no login, the quota wall, the wall
/// of time, a wire or a CLI that refused. An answer that came back and was
/// not what was asked is not the road's to be passed over for.
fn passes_over(why: &str) -> bool {
    why != SCHEMA
}

/// A value, written. Its `Debug` never prints the value: a written value is
/// what a person's field will hold, and a debug line is a log line.
pub struct Written {
    pub value: String,
    /// The model that wrote it, as the row names it.
    pub model: String,
    /// How long the write took, whole.
    pub ms: u64,
    /// The road that wrote it, and every road passed over on the way.
    pub answered: Answered,
}

impl std::fmt::Debug for Written {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Written")
            .field("chars", &self.value.chars().count())
            .field("model", &self.model)
            .field("ms", &self.ms)
            .field("answered", &self.answered)
            .finish()
    }
}

/// Writing the value one field should hold — the seam the small model sits
/// behind and a test replaces.
pub trait ValueWriter {
    /// The row this writer asks: half of a written value's identity, so a
    /// value another model wrote is never typed as this one's.
    fn row(&self) -> Option<&'static ValueRow>;

    /// Whether a person set this writer up: its row's road is one this
    /// product takes, and its key is where the row says a person puts it.
    /// `false`, no question offers an entry and nothing is ever asked.
    fn ready(&self) -> bool;

    /// The value `look` asks for, written within `left` — or the token of
    /// why there is none.
    ///
    /// # Errors
    ///
    /// The token a walk's row names the refusal by.
    fn write(&mut self, look: &FieldLook<'_>, left: Duration) -> Result<Written, String>;

    /// Open the road's connection ahead of the first write, off the
    /// caller's thread — while the judgment that may ask for a value is
    /// still in flight. Nothing is sent but a bare request of the endpoint's
    /// origin, no key with it. A writer with no road to warm does nothing.
    fn warm(&self) {}

    /// The most one value may take: the seat's own wall, unless the road is
    /// a CLI's, whose start alone is longer than that wall.
    fn wall(&self) -> Duration {
        Duration::from_millis(zerocode_core::type_value::seat().deadline_ms)
    }
}

/// Values written before, by identity — what a retry or a replay types again
/// instead of asking. Oldest out first, at most `cap` held.
#[derive(Debug)]
pub struct Values {
    held: VecDeque<(String, String)>,
    cap: usize,
}

impl Values {
    #[must_use]
    pub const fn new(cap: usize) -> Self {
        Self {
            held: VecDeque::new(),
            cap,
        }
    }

    /// How many values it holds at most.
    #[cfg(test)]
    #[must_use]
    pub const fn cap(&self) -> usize {
        self.cap
    }

    /// The value written under `identity`, if one was.
    #[must_use]
    pub fn recall(&self, identity: &str) -> Option<String> {
        self.held
            .iter()
            .find(|(kept, _)| kept == identity)
            .map(|(_, value)| value.clone())
    }

    /// Keep `value` under `identity`, in place of whatever it held; the
    /// oldest goes when the memory is full.
    pub fn keep(&mut self, identity: String, value: String) {
        self.held.retain(|(kept, _)| *kept != identity);
        self.held.push_back((identity, value));
        while self.held.len() > self.cap {
            self.held.pop_front();
        }
    }
}

/// The window's values: one memory every walk the window runs types from.
#[must_use]
pub fn window_values() -> Arc<Mutex<Values>> {
    static VALUES: OnceLock<Arc<Mutex<Values>>> = OnceLock::new();
    Arc::clone(VALUES.get_or_init(|| {
        Arc::new(Mutex::new(Values::new(
            zerocode_core::computer_use::WALK_STEPS_MAX,
        )))
    }))
}

/// The memory, surviving a panicking holder: a value either was written or
/// was not, and the map is whole at every step.
pub fn held(values: &Mutex<Values>) -> std::sync::MutexGuard<'_, Values> {
    values.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a row's key lives: the item a router row names, else the window's
/// key store under the row's key name. `None` for a row that names neither.
#[must_use]
pub fn key_service(row: &ValueRow) -> Option<String> {
    row.keychain_service.clone().or_else(|| {
        row.credential_key
            .as_ref()
            .map(|name| format!("{SERVICE_KEYCHAIN_SERVICE_PREFIX}{name}"))
    })
}

/// The endpoint a row's road asks, when it is one this product takes: the
/// Messages API for [`Road::Anthropic`], a row's own `/chat/completions` for
/// [`Road::OpenaiCompat`] — unless the row would have the request speak as
/// another client ([`ValueRow::client_fingerprint`]), which this product
/// never does. Code Assist is not built here. The Computer Use pane offers a
/// key only for a row this answers (`crate::type_value_keys`, t-9537): a key
/// for a road never taken is a key nothing asks with.
pub(crate) fn endpoint_of(row: &ValueRow) -> Option<String> {
    if row.client_fingerprint.is_some() {
        return None;
    }
    match row.road {
        Road::Anthropic => Some(ANTHROPIC_WIRE.url.to_string()),
        Road::OpenaiCompat => row
            .base_url
            .as_deref()
            .map(|base| format!("{}/chat/completions", base.trim_end_matches('/'))),
        // Code Assist is not built; a login road asks its vendor's CLI.
        Road::CodeAssist | Road::ClaudeCli | Road::CodexCli => None,
    }
}

/// What one question down a row's road came back with: the text the answer
/// wrote, and what asking cost — the bytes each way and, where the endpoint
/// said, the tokens it billed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub text: String,
    pub bytes_out: usize,
    pub bytes_in: usize,
    pub tokens: Option<Tokens>,
    /// The road that answered, and every road passed over on the way.
    pub answered: Answered,
}

/// The tokens one answer billed, as its endpoint counted them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
}

/// One question down `row`'s road, bounded whole by `deadline`: `system` as
/// the instructions and `user` as the one user message, at most `max_tokens`
/// of answer — the text the answer wrote, or the wire's word for why there is
/// none. The one envelope both askers of a generator send (t-10223): the
/// value a walk types and the plan a reflex run acts on; each road's two
/// shapes are spelled here and nowhere else.
pub(crate) async fn ask_road(
    row: &ValueRow,
    endpoint: &str,
    key: &str,
    system: &str,
    user: &str,
    max_tokens: usize,
    deadline: Instant,
) -> Result<Said, String> {
    let client = client().ok_or_else(|| TRANSPORT.to_string())?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| TIMEOUT.to_string())?;
    let request = client
        .post(endpoint)
        .timeout(remaining)
        .header("Content-Type", "application/json");
    let (request, body) = match row.road {
        Road::Anthropic => (
            request
                .header(ANTHROPIC_WIRE.key_header, key)
                .header("anthropic-version", ANTHROPIC_WIRE.version),
            json!({
                "model": row.model,
                "max_tokens": max_tokens,
                "system": system,
                "messages": [{ "role": "user", "content": user }],
            }),
        ),
        Road::ClaudeCli | Road::CodexCli => return Err(ROAD_UNSUPPORTED.to_string()),
        Road::OpenaiCompat | Road::CodeAssist => (
            request.bearer_auth(key),
            json!({
                "model": row.model,
                "max_tokens": max_tokens,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user },
                ],
            }),
        ),
    };
    let body = body.to_string();
    let bytes_out = body.len();
    let answer = request
        .body(body)
        .send()
        .await
        .map_err(|err| failure(&err))?;
    let status = answer.status();
    if !status.is_success() {
        return Err(token_for(status.as_u16()));
    }
    let text = answer.text().await.map_err(|err| failure(&err))?;
    let parsed: Value = serde_json::from_str(&text).map_err(|_| SCHEMA.to_string())?;
    let (written, input, output) = match row.road {
        Road::Anthropic => (
            parsed
                .get("content")
                .and_then(Value::as_array)
                .and_then(|blocks| {
                    blocks
                        .iter()
                        .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                        .and_then(|block| block.get("text").and_then(Value::as_str))
                }),
            "/usage/input_tokens",
            "/usage/output_tokens",
        ),
        Road::OpenaiCompat | Road::CodeAssist | Road::ClaudeCli | Road::CodexCli => (
            parsed
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            "/usage/prompt_tokens",
            "/usage/completion_tokens",
        ),
    };
    let tokens = parsed
        .pointer(input)
        .and_then(Value::as_u64)
        .zip(parsed.pointer(output).and_then(Value::as_u64))
        .map(|(input, output)| Tokens { input, output });
    Ok(Said {
        text: written.ok_or_else(|| SCHEMA.to_string())?.to_string(),
        bytes_out,
        bytes_in: text.len(),
        tokens,
        answered: Answered::default(),
    })
}

/// The word a one-shot's own refusal is said by: the quota wall or the login
/// wall where the CLI's words are this window's measured ones
/// ([`crate::quota_wall::one_shot_cause`]), the CLI's refusal otherwise.
fn cli_word(agent: &str, said: &str) -> String {
    match crate::quota_wall::one_shot_cause(agent, said) {
        Some(StallCause::QuotaWall) => QUOTA_WALL,
        Some(StallCause::LoginWall) => NO_LOGIN,
        _ => CLI_REFUSED,
    }
    .to_string()
}

/// The word for a run that never finished.
fn once_word(failure: OnceFailure) -> String {
    match failure {
        OnceFailure::Spawn(_) => TRANSPORT,
        OnceFailure::TimedOut => TIMEOUT,
    }
    .to_string()
}

/// What Claude Code's one JSON result says (`--output-format json`): the
/// text of `result`, and what the request carried and wrote — or, when the
/// result is an error, the word for why.
fn read_claude(once: &Once, bytes_out: usize) -> Result<Said, String> {
    let Ok(parsed) = serde_json::from_str::<Value>(once.stdout.trim()) else {
        return Err(if once.success {
            SCHEMA.to_string()
        } else {
            cli_word("claude", &once.stderr_tail)
        });
    };
    let result = parsed
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !once.success || parsed.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(cli_word("claude", result));
    }
    let count = |key: &str| {
        parsed
            .pointer(&format!("/usage/{key}"))
            .and_then(Value::as_u64)
    };
    let tokens = count("input_tokens")
        .zip(count("output_tokens"))
        .map(|(input, output)| Tokens {
            input: input
                + count("cache_creation_input_tokens").unwrap_or(0)
                + count("cache_read_input_tokens").unwrap_or(0),
            output,
        });
    Ok(Said {
        text: result.to_string(),
        bytes_out,
        bytes_in: once.stdout.len(),
        tokens,
        answered: Answered::default(),
    })
}

/// What Codex's JSON events say (`exec --json`): the last agent message, and
/// the turn's usage — or, when the turn failed, the word for why.
fn read_codex(once: &Once, bytes_out: usize) -> Result<Said, String> {
    let mut text = None;
    let mut tokens = None;
    let mut failed: Option<String> = None;
    for event in once
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        match event.get("type").and_then(Value::as_str) {
            Some("item.completed")
                if event.pointer("/item/type").and_then(Value::as_str) == Some("agent_message") =>
            {
                text = event
                    .pointer("/item/text")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("turn.completed") => {
                let count = |key: &str| {
                    event
                        .pointer(&format!("/usage/{key}"))
                        .and_then(Value::as_u64)
                };
                tokens = count("input_tokens")
                    .zip(count("output_tokens"))
                    .map(|(input, output)| Tokens { input, output });
            }
            Some("turn.failed") => {
                failed = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("error") if failed.is_none() => {
                failed = event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    match (text, failed) {
        (Some(text), _) if once.success => Ok(Said {
            text,
            bytes_out,
            bytes_in: once.stdout.len(),
            tokens,
            answered: Answered::default(),
        }),
        (_, Some(said)) => Err(cli_word("codex", &said)),
        (None, None) if once.success => Err(SCHEMA.to_string()),
        _ => Err(cli_word("codex", &once.stderr_tail)),
    }
}

/// The writer that actually asks: the road a person chose, down each road it
/// tries, with the login the window's panes run as or the key a person put
/// where the row says.
pub struct LiveWriter {
    setup: Setup,
    keys: Box<dyn RouterKeys>,
    /// Where an API-key request goes instead of the row's own endpoint — a
    /// test's loopback.
    endpoint: Option<String>,
    /// The key, read once and only when first needed: a walk that never
    /// meets a field never touches the key store.
    key: OnceLock<Option<String>>,
    /// The road that answered last, and the roads set aside for this
    /// writer's later questions with why ([`Self::set_aside`]).
    roads: Mutex<Roads>,
}

/// Which road a writer asked last, and which it no longer asks.
#[derive(Debug, Default)]
struct Roads {
    last: Option<GeneratorRoad>,
    aside: Vec<(GeneratorRoad, String)>,
}

impl LiveWriter {
    /// The window's writer: the road the person chose, the accounts the
    /// window's panes run as, and its key store (`Keychain`) — read nothing
    /// until a walk's look has a field to type into.
    #[must_use]
    pub fn window(setup: Setup) -> Self {
        Self {
            setup,
            keys: Box::new(Keychain::of_this_machine()),
            endpoint: None,
            key: OnceLock::new(),
            roads: Mutex::default(),
        }
    }

    /// A writer on a person's own key over `keys`, asking `endpoint` — how a
    /// test crosses a real socket with a key store of its own.
    #[cfg(test)]
    #[must_use]
    pub fn at(endpoint: &str, keys: Box<dyn RouterKeys>) -> Self {
        Self {
            setup: Setup::new(GeneratorRoad::ApiKey, PathBuf::new(), PathBuf::new()),
            keys,
            endpoint: Some(endpoint.to_string()),
            key: OnceLock::new(),
            roads: Mutex::default(),
        }
    }

    /// A writer on `setup`'s road with no key store — how a test runs the
    /// login roads over scripts of its own.
    #[cfg(test)]
    #[must_use]
    pub fn over(setup: Setup) -> Self {
        Self {
            setup,
            keys: Box::new(crate::api_routers::HeldKeys::default()),
            endpoint: None,
            key: OnceLock::new(),
            roads: Mutex::default(),
        }
    }

    /// The key-road row's key, read once from where the row says a person
    /// puts it.
    fn key(&self) -> Option<&str> {
        self.key
            .get_or_init(|| {
                let row = zerocode_core::type_value::chosen_on(GeneratorRoad::ApiKey)?;
                let service = key_service(row)?;
                self.keys
                    .read(&service)
                    .ok()
                    .flatten()
                    .map(|key| key.trim().to_string())
                    .filter(|key| !key.is_empty())
            })
            .as_deref()
    }

    /// Where `row`'s API request goes.
    fn endpoint(&self, row: &ValueRow) -> Option<String> {
        endpoint_of(row)?;
        self.endpoint.clone().or_else(|| endpoint_of(row))
    }

    /// Why `road` cannot be asked now, found without asking it: no row, a
    /// road this product does not take, no key a person set, no CLI on this
    /// machine. A login is not looked at — this product never reads one — so
    /// a missing login is said when the CLI says it.
    fn road_unready(&self, road: GeneratorRoad) -> Option<&'static str> {
        let Some(row) = zerocode_core::type_value::chosen_on(road) else {
            return Some(NO_ROW);
        };
        match row.road {
            Road::ClaudeCli | Road::CodexCli => {
                self.setup.program(row.road).is_none().then_some(NO_CLI)
            }
            Road::Anthropic | Road::CodeAssist | Road::OpenaiCompat => {
                if endpoint_of(row).is_none() {
                    Some(ROAD_UNSUPPORTED)
                } else {
                    self.key().is_none().then_some(NO_KEY)
                }
            }
        }
    }

    fn roads(&self) -> std::sync::MutexGuard<'_, Roads> {
        self.roads.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Set the road that answered last aside for this writer's later
    /// questions, for `why` — a road that answered, but whose answers could
    /// not be used, such as plans the contract refused after every retry —
    /// when a road after it is left to ask. Whether one is.
    pub fn set_aside(&self, why: &str) -> bool {
        let tries = self.setup.road.tries();
        let mut roads = self.roads();
        let Some(last) = roads.last else {
            return false;
        };
        let after = tries
            .iter()
            .skip_while(|road| **road != last)
            .skip(1)
            .any(|road| !roads.aside.iter().any(|(aside, _)| aside == road));
        if after {
            roads.aside.push((last, why.to_string()));
            roads.last = None;
        }
        after
    }

    /// Why nobody can answer this writer, as the word a row names it by —
    /// the person chose no road, or every road it tries is unready, each
    /// with its reason — or `None` for a writer one of whose roads can ask.
    #[must_use]
    pub fn unready(&self) -> Option<String> {
        let tries = self.setup.road.tries();
        if tries.is_empty() {
            return Some(GENERATOR_OFF.to_string());
        }
        let mut passed = Vec::new();
        for road in tries {
            match self.road_unready(*road) {
                None => return None,
                Some(why) => passed.push((*road, why.to_string())),
            }
        }
        Some(nobody(&passed))
    }

    /// One question of the first road that answers, each road asked within
    /// what `left` has left ([`ask_road`] for a key, the vendor's CLI once
    /// for a login): the text the answer wrote, what it cost and which road
    /// gave it — or every road's word for why there is none.
    ///
    /// # Errors
    ///
    /// The token a row names the failure by.
    pub fn ask_text(
        &self,
        system: &str,
        user: &str,
        max_tokens: usize,
        left: Duration,
    ) -> Result<Said, String> {
        let tries = self.setup.road.tries();
        if tries.is_empty() {
            return Err(GENERATOR_OFF.to_string());
        }
        let deadline = Instant::now() + left;
        let aside = self.roads().aside.clone();
        let mut passed: Vec<(GeneratorRoad, String)> = aside.clone();
        for road in tries {
            if aside.iter().any(|(set, _)| set == road) {
                continue;
            }
            let asked = match (
                self.road_unready(*road),
                zerocode_core::type_value::chosen_on(*road),
            ) {
                (None, Some(row)) => self
                    .ask_on(row, system, user, max_tokens, deadline)
                    .map(|said| (said, row)),
                (why, _) => Err(why.unwrap_or(NO_ROW).to_string()),
            };
            match asked {
                Ok((mut said, row)) => {
                    said.answered = Answered {
                        road: road.word(),
                        model: row.model.clone(),
                        passed: passed
                            .iter()
                            .map(|(road, why)| passed_word(*road, why))
                            .collect(),
                    };
                    remember(&said.answered);
                    self.roads().last = Some(*road);
                    return Ok(said);
                }
                Err(why) if passes_over(&why) => passed.push((*road, why)),
                Err(why) => return Err(why),
            }
        }
        remember(&Answered {
            road: "",
            model: String::new(),
            passed: passed
                .iter()
                .map(|(road, why)| passed_word(*road, why))
                .collect(),
        });
        Err(nobody(&passed))
    }

    /// One question down `row`'s own road, by `deadline`.
    fn ask_on(
        &self,
        row: &'static ValueRow,
        system: &str,
        user: &str,
        max_tokens: usize,
        deadline: Instant,
    ) -> Result<Said, String> {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| TIMEOUT.to_string())?;
        match row.road {
            Road::ClaudeCli => {
                let argv = zerocode_core::type_value::claude_cli_argv(row, system);
                let env = zerocode_core::type_value::claude_cli_env(row);
                let once = self.run(row.road, &argv, &env, user, left)?;
                read_claude(&once, user.len())
            }
            Road::CodexCli => {
                let argv = zerocode_core::type_value::codex_cli_argv(
                    row,
                    &zerocode_core::launch::compat_launch_args("codex"),
                );
                // Codex takes no system prompt of a caller's: the
                // instructions go first on stdin, the question after them.
                let asked = format!("{system}\n\n{user}");
                let once = self.run(row.road, &argv, &[], &asked, left)?;
                read_codex(&once, asked.len())
            }
            Road::Anthropic | Road::CodeAssist | Road::OpenaiCompat => {
                let endpoint = self
                    .endpoint(row)
                    .ok_or_else(|| ROAD_UNSUPPORTED.to_string())?;
                let key = self.key().ok_or_else(|| NO_KEY.to_string())?.to_string();
                // Every asker drives sync roads from a thread of its own, and
                // blocks on the window's runtime as its judge's wire does.
                tauri::async_runtime::block_on(ask_road(
                    row, &endpoint, &key, system, user, max_tokens, deadline,
                ))
            }
        }
    }

    /// A login road's CLI, once, under the account the window's panes run
    /// as with the row's `extra` on top, the question on stdin — in a
    /// directory of its own ([`one_shot_dir`]).
    fn run(
        &self,
        road: Road,
        argv: &[String],
        extra: &[(String, String)],
        stdin: &str,
        left: Duration,
    ) -> Result<Once, String> {
        let program = self.setup.program(road).ok_or_else(|| NO_CLI.to_string())?;
        let mut env = self
            .setup
            .account_env(road)
            .map_err(|_| NO_LOGIN.to_string())?;
        env.extend(extra.iter().cloned());
        let cwd = one_shot_dir();
        crate::scm_runtime::run_once(&program, cwd.as_deref(), argv, &env, stdin, left)
            .map_err(once_word)
    }
}

impl ValueWriter for LiveWriter {
    /// The row of the first road this writer can ask — what a value it
    /// writes is remembered under.
    fn row(&self) -> Option<&'static ValueRow> {
        self.setup
            .road
            .tries()
            .iter()
            .find(|road| self.road_unready(**road).is_none())
            .or_else(|| self.setup.road.tries().first())
            .and_then(|road| zerocode_core::type_value::chosen_on(*road))
    }

    fn ready(&self) -> bool {
        self.unready().is_none()
    }

    /// A first write in a process paid for the client and the handshake:
    /// 895 ms against 657 and 672 for the two behind it on the same endpoint
    /// (2026-09-26). A warm-up that outlives the seat's own wall was no help,
    /// so that wall bounds it. Only a writer a person set up on a key warms
    /// anything: a CLI has no connection of ours to open.
    fn warm(&self) {
        let Some(row) = self.row() else {
            return;
        };
        let (true, Some(client), Some(endpoint)) = (self.ready(), client(), self.endpoint(row))
        else {
            return;
        };
        let Ok(url) = url::Url::parse(&endpoint) else {
            return;
        };
        let origin = url.origin().ascii_serialization();
        let wall = Duration::from_millis(zerocode_core::type_value::seat().deadline_ms);
        tauri::async_runtime::spawn(async move {
            let _ = client.get(&origin).timeout(wall).send().await;
        });
    }

    /// A login road's CLI starts, reads its home and asks in seconds, not the
    /// seat's milliseconds: its wall is the one a headless frontier turn is
    /// held to ([`super::team::TEAM_DEADLINE`]).
    fn wall(&self) -> Duration {
        let seat = Duration::from_millis(zerocode_core::type_value::seat().deadline_ms);
        let by_cli = self.setup.road.tries().iter().any(|road| {
            zerocode_core::type_value::chosen_on(*road)
                .is_some_and(|row| matches!(row.road, Road::ClaudeCli | Road::CodexCli))
        });
        if by_cli {
            super::team::TEAM_DEADLINE.max(seat)
        } else {
            seat
        }
    }

    fn write(&mut self, look: &FieldLook<'_>, left: Duration) -> Result<Written, String> {
        let began = Instant::now();
        // The table's own question: its instructions as the system, the
        // rendered field as the one user line. A value is a line of at most
        // the question's cap in characters, so no more tokens than that are
        // ever worth waiting for.
        let question = zerocode_core::type_value::asked();
        let said = self.ask_text(
            &question.instructions,
            &zerocode_core::type_value::render(look),
            question.value_char_cap,
            left,
        )?;
        let value = zerocode_core::type_value::read(&said.text)
            .map_err(|refusal| refusal.token().to_string())?;
        Ok(Written {
            value,
            model: said.answered.model.clone(),
            ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
            answered: said.answered,
        })
    }
}

/// The one HTTP client every written value goes through: its pool keeps the
/// endpoint's TLS session between a walk's entries, so the second value
/// rides the first one's socket.
fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| reqwest::Client::builder().build().ok())
        .as_ref()
}

/// The wire's word for a request that never answered: its wall, or anything
/// else on the way (`crate::systemone`'s own two words).
fn failure(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        TIMEOUT.to_string()
    } else {
        TRANSPORT.to_string()
    }
}

#[cfg(test)]
pub(in crate::computer_use) mod tests;
