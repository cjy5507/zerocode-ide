//! The seat that WRITES: which small model a browser walk asks for the one
//! line it has to type, what it is asked, and what counts as an answer
//! (t-4700).
//!
//! Every other judgment this product makes picks from a set somebody else laid
//! out — a numbered control (`crate::screen_action`), a room
//! (`crate::jev::PLACEMENT`), an order over notes. A `type` step is the one
//! place with nothing to pick FROM: the flow stopped in front of an empty box
//! and the next thing that has to exist is a string. Jev answers a closed
//! choice and cannot write one, so a model has to, and the only thing that
//! settles WHICH model is the wait it costs — the walk is already inside a
//! stopped step's budget, and a value that arrives after the budget is a value
//! nobody typed.
//!
//! So the choice is a measurement, and the measurement is the table. Both files
//! beside this module are read by two programs: `tools/type_value_latency.py`
//! calls the real providers with them, and this module hands the product the
//! same words and the same row. Neither can drift from the other, because
//! neither holds a second copy.
//!
//! - `fixtures/type-value/question.json` — the words, the state's keys and
//!   their labels, the cap on a value, and the field shapes the probe rotates
//!   through. [`render`] builds from it the exact bytes the probe sends.
//! - `fixtures/type-value/models.json` — one row per candidate: its model id,
//!   the road that reaches it, and what that road was measured at. The seat
//!   names a row by id ([`chosen`]); nothing else in this product spells a
//!   model for this seat.
//!
//! WHAT WAS MEASURED, and what it settled (2026-09-18, this machine; four
//! readings, nine candidates, `tools/type_value_latency.py`). Every number is
//! in the table, kept there rather than here so a re-reading replaces data
//! instead of prose. Three things the numbers said:
//!
//! 1. **The bar was not met.** The target was a 400 ms median for one line.
//!    The best median any row a person of this product can reach ever held is
//!    617 ms ([`chosen`]). It is named anyway, because a seat with no row asks
//!    nobody and a walk with nobody to ask stops for a person.
//! 2. **One reading would have chosen the wrong row.** The pilot reading had
//!    `diffusiongemma` at 393 ms — under the bar — against `haiku` at 624. The
//!    three readings after it had `diffusiongemma` at 1,363, 1,562 and 3,715
//!    against 641, 671 and 617, and its worst came at the QUIETEST load of the
//!    four. A seat is held to the worst median it can be given, which is what
//!    [`Measured::p50_worst_ms`] answers and what the choice is argued from.
//! 3. **The wait is the distance, not the model.** A 4B model one network hop
//!    away (`lan-qwen-4b`) answered in 101–129 ms and was right every time,
//!    six times faster than the best row across the internet, and a model ten
//!    times its size over the same hop cost 7 ms more. That is why [`Road`]
//!    keeps `openai-compat` open: a person with their own endpoint, on their
//!    own machine or their own network, is a row away from a walk that types
//!    as fast as it looks.
//!
//! Nothing here touches the network or the clock. A road is a row's data; the
//! socket belongs to whoever spends it.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The question, as both readers hold it.
const QUESTION_JSON: &str = include_str!("../fixtures/type-value/question.json");
/// The candidates and what each was measured at.
const MODELS_JSON: &str = include_str!("../fixtures/type-value/models.json");

/// One key of the state a field's question carries, and the words that
/// introduce it on the wire.
#[derive(Debug, Clone, Deserialize)]
pub struct StateKey {
    pub key: String,
    pub label: String,
}

/// One field shape the probe asks every candidate about. They are the
/// measurement's cases, not the product's — the product renders a real field —
/// but they live beside the words so a re-reading asks what the last one did.
#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub id: String,
    pub goal: String,
    pub field: Field,
    /// The values this field could take. A candidate that answers quickly and
    /// wrongly is not a candidate.
    pub accepts: Vec<String>,
}

/// What a look saw around one box.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Field {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub placeholder: String,
    /// The page's own words beside the box. Never its value: a field a person
    /// already typed into carries what they typed.
    #[serde(default)]
    pub near: String,
}

/// The one question this seat asks.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    /// The version of the words. A judgment read under one wording is not
    /// evidence about another, and `the_version_is_pinned_to_the_words` holds
    /// this to [`crate::jev::rubric_fingerprint`].
    pub rubric_version: u32,
    pub instructions: String,
    pub state: Vec<StateKey>,
    /// The line that ends the state and asks for the answer.
    pub value_line: String,
    /// Characters of a value this seat will take. A field's value is a line;
    /// an answer longer than this is an answer to another question.
    pub value_char_cap: usize,
    pub cases: Vec<Case>,
}

/// How a row is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Road {
    /// Anthropic's Messages API, asked with the API key a person put in the
    /// window's key store for this row ([`ValueRow::credential_key`]) — never
    /// with a subscription login, which is the person's to spend in the
    /// vendor's own clients alone.
    Anthropic,
    /// Google's Code Assist backend, under the Antigravity identity.
    CodeAssist,
    /// Anything that speaks `/chat/completions` — a gateway preset, or the
    /// person's own endpoint saved as a provider row.
    OpenaiCompat,
    /// Claude Code's own CLI, asked once headless (`claude -p`) under the
    /// account the window's panes run as (t-10372). The login is the CLI's:
    /// this product never reads, holds or sends it, and speaks for no client
    /// — the vendor's own client is the one asking. A row of this road names
    /// no endpoint and no key.
    ClaudeCli,
    /// Codex's own CLI, asked once headless (`codex exec`) under the Codex
    /// home the window's panes run with — the same terms as [`Self::ClaudeCli`].
    CodexCli,
}

/// Which road the value seat's writer takes, as a person chose it in the
/// Computer Use pane (`computer_generator_road`, t-10372).
///
/// `auto` is what a person who never chose gets: the logins the window
/// already runs its panes with, Claude's first and Codex's when Claude's
/// cannot answer — and every answer says which road gave it and why the one
/// before it did not, so a road passed over is never a quiet one. A person's
/// own API key is asked only when they choose it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratorRoad {
    /// [`Self::ClaudeLogin`], then [`Self::CodexLogin`].
    #[default]
    Auto,
    /// The Claude Code login ([`Table::claude_login`], down
    /// [`Road::ClaudeCli`]), spent from the person's subscription.
    ClaudeLogin,
    /// The Codex login ([`Table::codex_login`], down [`Road::CodexCli`]).
    CodexLogin,
    /// A person's own API key ([`Table::chosen`]).
    ApiKey,
    /// Nobody writes: a walk leaves an empty field to a person, and a reflex
    /// autopilot does not start.
    Off,
}

impl GeneratorRoad {
    /// Every road, in the order the pane offers them.
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::ClaudeLogin,
        Self::CodexLogin,
        Self::ApiKey,
        Self::Off,
    ];

    /// The word the settings document, the pane and every answered row name
    /// this road by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ClaudeLogin => "claude_login",
            Self::CodexLogin => "codex_login",
            Self::ApiKey => "api_key",
            Self::Off => "off",
        }
    }

    /// The roads asked, in order, when a person chose this one: `auto` is
    /// the two logins, `off` is none, and every other road is itself.
    #[must_use]
    pub const fn tries(self) -> &'static [Self] {
        match self {
            Self::Auto => &[Self::ClaudeLogin, Self::CodexLogin],
            Self::ClaudeLogin => &[Self::ClaudeLogin],
            Self::CodexLogin => &[Self::CodexLogin],
            Self::ApiKey => &[Self::ApiKey],
            Self::Off => &[],
        }
    }
}

/// ONE run of one candidate. Milliseconds to the WHOLE one-line value, which
/// is what a walk waits for; a first byte that is a model clearing its throat
/// is not a value.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// The probe's name for the run — the note its ledger rows carry.
    pub note: String,
    /// Counted calls that answered.
    pub n: u32,
    pub p10_ms: u32,
    pub p50_ms: u32,
    pub p90_ms: u32,
    /// Of `n`, how many answers this seat would have taken. A candidate that
    /// answers quickly and wrongly is not a candidate.
    pub accepted: u32,
    /// Of `n`, how many spent their first byte narrating rather than
    /// answering — the mark of a model whose shape is wrong for this seat
    /// however fast its first byte is.
    #[serde(default)]
    pub thought_first: u32,
    /// The 1-minute load average's range across the run. A number read under
    /// a running gate is not the same number, and a candidate is only
    /// comparable with the ones read beside it.
    pub load1_min: f64,
    pub load1_max: f64,
}

/// Every run of one candidate.
///
/// A list rather than one number on purpose: the first reading of this table
/// had `diffusiongemma` at a median of 393 ms and `haiku` at 624, and the next
/// two readings had them at 1,363 and 1,562 against 641 and 671. One reading
/// would have chosen the wrong row. What a seat needs is not the best median
/// but the worst one it can be held to, so the table keeps them all and
/// [`Measured::p50_worst_ms`] is what a choice is argued from.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measured {
    /// The day it was read, `YYYY-MM-DD`.
    pub at: String,
    pub runs: Vec<Run>,
    /// What the reading is not, in one sentence.
    #[serde(default)]
    pub caveat: String,
}

impl Measured {
    /// The worst median across the runs — what this row can be held to.
    #[must_use]
    pub fn p50_worst_ms(&self) -> Option<u32> {
        self.runs.iter().map(|run| run.p50_ms).max()
    }

    /// The best p10 across the runs — what the model can do when nothing is
    /// in front of it, which is a fact about the model rather than a promise.
    #[must_use]
    pub fn p10_best_ms(&self) -> Option<u32> {
        self.runs.iter().map(|run| run.p10_ms).min()
    }

    /// Counted calls across every run, and how many of them this seat would
    /// have taken.
    #[must_use]
    pub fn asked_and_accepted(&self) -> (u32, u32) {
        self.runs.iter().fold((0, 0), |(asked, took), run| {
            (asked + run.n, took + run.accepted)
        })
    }
}

/// One candidate.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueRow {
    /// The word the seat, a ledger row and a refusal use.
    pub id: String,
    /// The model id the road is asked for. THE one place this product spells
    /// a model for this seat.
    pub model: String,
    pub road: Road,
    /// Who can reach it at all — the sentence that decides whether a row may
    /// ever be the chosen one, however fast it reads.
    pub reach: String,
    /// `openai-compat`: the endpoint.
    #[serde(default)]
    pub base_url: Option<String>,
    /// The key's name in the window's key store (`dev.zerocode.key.<name>`),
    /// or the keychain item a router row keeps it under. Never a key — and
    /// the only place this seat's key is found: a row whose key a person has
    /// not put there is a row this seat does not ask.
    #[serde(default)]
    pub credential_key: Option<String>,
    #[serde(default)]
    pub keychain_service: Option<String>,
    /// A gateway that refuses an unknown wire image names the client it wants.
    #[serde(default)]
    pub client_fingerprint: Option<String>,
    /// `code-assist`: the host, and the thinking rung the request asks for —
    /// the one field that separates `gemini-3.8-flash` the NAME from what the
    /// backend serves under it (t-4701).
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub thinking_level: Option<String>,
    #[serde(default)]
    pub measured: Option<Measured>,
}

/// The seat's table.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub seat: String,
    /// The row a person's own API key is asked with
    /// ([`GeneratorRoad::ApiKey`]), by [`ValueRow::id`], or `None` while no
    /// candidate has cleared the bar.
    pub chosen: Option<String>,
    /// The row the Claude Code login is asked with
    /// ([`GeneratorRoad::ClaudeLogin`]), by [`ValueRow::id`].
    #[serde(default)]
    pub claude_login: Option<String>,
    /// The row the Codex login is asked with ([`GeneratorRoad::CodexLogin`]).
    #[serde(default)]
    pub codex_login: Option<String>,
    /// The whole wait a walk gives this seat before it gives up and stops for
    /// a person. Past it the value is no longer worth having.
    pub deadline_ms: u64,
    pub rows: Vec<ValueRow>,
}

fn question() -> &'static Question {
    static PARSED: OnceLock<Question> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(QUESTION_JSON).expect("question.json"))
}

fn table() -> &'static Table {
    static PARSED: OnceLock<Table> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(MODELS_JSON).expect("models.json"))
}

/// The one question, whole.
#[must_use]
pub fn asked() -> &'static Question {
    question()
}

/// The seat's table, whole.
#[must_use]
pub fn seat() -> &'static Table {
    table()
}

/// Every candidate, in the order the table lists them.
#[must_use]
pub fn rows() -> &'static [ValueRow] {
    &table().rows
}

/// The row named `id`.
#[must_use]
pub fn row(id: &str) -> Option<&'static ValueRow> {
    rows().iter().find(|row| row.id == id)
}

/// The row a person's own key is asked with, or `None` while none has cleared
/// the bar — which is an answer too: a walk with no row to ask does not guess
/// a value, it stops for a person exactly as it would have without a model at
/// all.
#[must_use]
pub fn chosen() -> Option<&'static ValueRow> {
    table().chosen.as_deref().and_then(row)
}

/// The row one road asks — the one function a writer learns its row from, so
/// the road a person chose and the row that answers cannot part. `off` asks
/// nobody, and `auto` is no one road: its rows are its
/// [`GeneratorRoad::tries`]'.
#[must_use]
pub fn chosen_on(road: GeneratorRoad) -> Option<&'static ValueRow> {
    match road {
        GeneratorRoad::ClaudeLogin => table().claude_login.as_deref().and_then(row),
        GeneratorRoad::CodexLogin => table().codex_login.as_deref().and_then(row),
        GeneratorRoad::ApiKey => chosen(),
        GeneratorRoad::Auto | GeneratorRoad::Off => None,
    }
}

/// What the look around one box says, as the question reads it.
#[derive(Debug, Clone, Copy)]
pub struct FieldLook<'a> {
    /// What the flow is for.
    pub goal: &'a str,
    pub label: &'a str,
    pub placeholder: &'a str,
    /// The page's own words beside the box.
    pub near: &'a str,
}

/// The bytes one question sends — the state in the table's key order, each
/// under the table's own label, ending with the table's asking line.
///
/// This is the whole request as far as this crate is concerned: which envelope
/// carries it is the road's business, and the words are the same down every
/// road, which is the only reason two roads' readings can be compared.
#[must_use]
pub fn render(look: &FieldLook<'_>) -> String {
    let question = question();
    let mut said = String::new();
    for key in &question.state {
        let value = match key.key.as_str() {
            "goal" => look.goal,
            "label" => look.label,
            "placeholder" => look.placeholder,
            "near" => look.near,
            _ => "",
        };
        said.push_str(&key.label);
        said.push_str(": ");
        said.push_str(value);
        said.push('\n');
    }
    said.push_str(&question.value_line);
    said
}

/// Why an answer is not a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueRefusal {
    /// Nothing, or nothing but space.
    Empty,
    /// More than one line. A model that explained itself answered a different
    /// question, and the first line of an explanation is not the value.
    NotOneLine,
    /// Longer than [`Question::value_char_cap`].
    TooLong,
    /// Holds a character a keyboard road cannot type — a control character
    /// that would land as a key rather than as text.
    Untypable,
}

impl ValueRefusal {
    /// The word a walk's row names this refusal by: `value_` and the rule,
    /// so a row tells a value this seat refused from a wire that never
    /// answered, and carries not one character of the answer.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Empty => "value_empty",
            Self::NotOneLine => "value_not_one_line",
            Self::TooLong => "value_too_long",
            Self::Untypable => "value_untypable",
        }
    }
}

/// Everything one written value depends on (t-6720): what makes a second
/// write the same write, so a retry or a replay types the value it wrote
/// before instead of asking again, and anything else asks afresh.
#[derive(Debug, Clone, Copy)]
pub struct ValueInput<'a> {
    /// What the question renders — the goal and the words around the box.
    pub look: FieldLook<'a>,
    /// What the field holds right now, as the look read it. Never rendered
    /// and never sent: it is part of the identity only, because a field that
    /// changed under the walk is a different question even with the same
    /// words around it.
    pub now: &'a str,
    /// The document the look read the field in — its epoch, as the look
    /// carried it. Empty when the look carried none, and then nothing is
    /// reused: a value cannot be vouched for across documents nobody told
    /// apart.
    pub epoch: &'a str,
    /// The field itself in that document, as the look named it — its own
    /// selector, tag and role, never a name composed here.
    pub target: &'a str,
}

/// The identity of one value's input asked of `row`: the seat's name, its
/// question's version, the row's model and every part of the input, each
/// behind its length ([`crate::jev::digest_of`], the digest a Jev row's
/// receipt is) — so a changed word of the question, another model, another
/// goal, another field, another value in it or another document is another
/// identity. `None` when the look named no document ([`ValueInput::epoch`]):
/// such a value is written fresh every time.
#[must_use]
pub fn identity(input: &ValueInput<'_>, row: &ValueRow) -> Option<String> {
    if input.epoch.trim().is_empty() {
        return None;
    }
    let parts = serde_json::json!([
        input.look.goal,
        input.look.label,
        input.look.placeholder,
        input.look.near,
        input.now,
        input.epoch,
        input.target,
    ]);
    Some(crate::jev::digest_of(
        &seat().seat,
        asked().rubric_version,
        &row.model,
        parts.to_string().as_bytes(),
    ))
}

/// What the [`Road::Anthropic`] road speaks: the Messages endpoint, the API
/// version the request is written against, and the header a person's own API
/// key goes in. Nothing else: no client is spoken for and no subscription is
/// admitted down this road (t-6720, the coordinator's decision m-9526).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnthropicWire {
    /// Where a value is asked for.
    pub url: &'static str,
    /// The API version the request is written against.
    pub version: &'static str,
    /// The header the person's key rides.
    pub key_header: &'static str,
}

/// [`AnthropicWire`], as this product sends it.
pub const ANTHROPIC_WIRE: AnthropicWire = AnthropicWire {
    url: "https://api.anthropic.com/v1/messages",
    version: "2023-06-01",
    key_header: "x-api-key",
};

/// What the [`Road::ClaudeCli`] road runs after the program's name (t-10372):
/// Claude Code once, headless, on the row's model, with `system` as the whole
/// system prompt and the question on stdin — never on argv, where its words
/// would sit in every `ps` and meet the command line's length. The run is
/// nobody's session and nobody's workspace: no session is saved, no tool is
/// offered, no settings file is read (so no hook of a pane's is loaded), no
/// MCP server and no skill is listed, and the answer comes back as the CLI's
/// one JSON result.
///
/// The CLI takes no cap on an answer's tokens: the seat's reader
/// ([`read`]) and the asker's wall bound what it may write.
#[must_use]
pub fn claude_cli_argv(row: &ValueRow, system: &str) -> Vec<String> {
    [
        "-p",
        "--model",
        row.model.as_str(),
        "--output-format",
        "json",
        "--no-session-persistence",
        "--tools",
        "",
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--system-prompt",
        system,
    ]
    .iter()
    .map(|word| (*word).to_string())
    .collect()
}

/// What the [`Road::CodexCli`] road runs after the program's name (t-10372):
/// Codex once, headless, on the row's model at the row's reasoning rung
/// ([`ValueRow::thinking_level`]), reading its instructions and question from
/// stdin (`-`). Nothing of it is kept or offered: no session is recorded
/// (`--ephemeral`), the person's `config.toml` and rules are not read (so no
/// hook, MCP server or pinned provider of theirs rides along — the login
/// still comes from the Codex home), the sandbox is read-only, and the
/// answer is the CLI's JSON events. `compat` is the words that keep Codex
/// starting inside this window (`crate::launch::compat_launch_args`).
#[must_use]
pub fn codex_cli_argv(row: &ValueRow, compat: &[String]) -> Vec<String> {
    let mut argv: Vec<String> = [
        "exec",
        "--json",
        "--ephemeral",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--ignore-user-config",
        "--ignore-rules",
        "--model",
        row.model.as_str(),
    ]
    .iter()
    .map(|word| (*word).to_string())
    .collect();
    if let Some(rung) = &row.thinking_level {
        argv.push("-c".to_string());
        argv.push(format!("model_reasoning_effort={rung}"));
    }
    argv.extend(compat.iter().cloned());
    argv.push("-".to_string());
    argv
}

/// The environment the [`Road::ClaudeCli`] road's run adds: a row whose rung
/// ([`ValueRow::thinking_level`]) is `off` asks with no thinking budget
/// (`MAX_THINKING_TOKENS=0`, Claude Code's own variable). Measured on this
/// machine 2026-09-26, one party-size value on the Claude login: 108 output
/// tokens, 101 of them thinking, in 1,627 ms — and 5 tokens in 667 ms with
/// the budget at nothing, the same value.
#[must_use]
pub fn claude_cli_env(row: &ValueRow) -> Vec<(String, String)> {
    match row.thinking_level.as_deref() {
        Some("off") => vec![("MAX_THINKING_TOKENS".to_string(), "0".to_string())],
        _ => Vec::new(),
    }
}

/// The value an answer names, or why it names none.
///
/// The rules are the seat's, not a model's: one line, inside the cap, typable,
/// and stripped of the quotes a model wraps a value in however plainly it was
/// asked not to. A broken rule refuses the answer WHOLE — a walk that types
/// half of a misunderstood answer has done something nobody asked for.
///
/// # Errors
///
/// [`ValueRefusal`] names the rule the answer broke.
pub fn read(said: &str) -> Result<String, ValueRefusal> {
    let trimmed = said.trim();
    if trimmed.is_empty() {
        return Err(ValueRefusal::Empty);
    }
    if trimmed.lines().count() > 1 {
        return Err(ValueRefusal::NotOneLine);
    }
    let value = unquoted(trimmed);
    if value.is_empty() {
        return Err(ValueRefusal::Empty);
    }
    if value.chars().count() > question().value_char_cap {
        return Err(ValueRefusal::TooLong);
    }
    if value.chars().any(|ch| ch.is_control()) {
        return Err(ValueRefusal::Untypable);
    }
    Ok(value.to_string())
}

/// A value with the matching pair of quotes a model wrapped it in taken off.
/// One pair only: a value that IS a quoted string keeps its inner quotes.
fn unquoted(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.trim();
        }
    }
    value
}

/// The words that define the question, as one string — what the version is
/// pinned to, so a word changed without a version bump is a red test rather
/// than a quiet drift.
#[must_use]
pub fn rubric_words() -> String {
    let question = question();
    let mut words = String::new();
    words.push_str(&question.instructions);
    words.push('\n');
    for key in &question.state {
        words.push_str(&key.key);
        words.push('=');
        words.push_str(&key.label);
        words.push(',');
    }
    words.push('\n');
    words.push_str(&question.value_line);
    words
}

#[cfg(test)]
mod tests;
