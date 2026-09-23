//! Wire sessions — a CLI driven without its screen.
//!
//! Codex's `app-server` and Gemini CLI's `--acp` speak JSON-RPC over stdio,
//! Claude Code's `--input-format stream-json --output-format stream-json`
//! speaks its own event lines: the person's words go down as a request, the
//! agent's turns come back as notifications, and every question the agent
//! has is a REQUEST this side answers — so the conversation view is the whole
//! surface and nothing ever has to fall back to a screen
//! (docs/design/agent-wire-sessions-20260915.md).
//!
//! One session per child process. A reader thread turns each stdout line into
//! a message and hands it to the protocol's adapter, a pure function over
//! [`WireState`] (tested here without a child), which appends the turns the
//! window already knows how to draw (`TranscriptTurn`, unit 2), keeps the
//! words still being said as the live text ([`WireLive`] — what streams on
//! the page the way Claude Code's own panel streams), and files the open
//! questions ([`WireAsk`]). The window polls `wire_log` the way it polls a
//! pane's transcript — and at once when the session says something changed
//! (`wire:update`) — sends with `wire_send`, and answers with `wire_answer`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use zerocode_core::transcript::{TranscriptEdit, TranscriptTool, TranscriptTurn};

pub(crate) type WireId = u32;

/// How long the handshake may take: Codex answers `initialize` in under a
/// second; Gemini CLI's bundle boots for several, and a first `session/new`
/// may go out to authenticate.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(90);
/// A request the child never answers is a dead child, not a slow one.
const REPLY_WAIT: Duration = Duration::from_secs(30);
/// Turns held per session — the same ceiling the page keeps
/// (`HELPER_TURN_CAP`); older turns are dropped and the log says so.
const TURN_CAP: usize = 400;
/// How often at most the live text is announced to the page — a repaint
/// ceiling of about thirty a second. A settled change (a turn, a question, a
/// status) is announced at once, and the close that follows the last delta is
/// one, so no delta is ever held back for longer than the gap.
const LIVE_NOTIFY_GAP: Duration = Duration::from_millis(33);
/// How long a CLI gets to answer `--help` / `--version` at a wire's start.
const PROBE_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Protocol {
    /// Codex `app-server` (its own JSON-RPC protocol).
    AppServer,
    /// The Agent Client Protocol (Gemini CLI `--acp`).
    Acp,
    /// Claude Code `-p --input-format stream-json --output-format
    /// stream-json`: one JSON line per event — the partial messages its own
    /// panel streams from (`--include-partial-messages`), the transcript's
    /// `assistant`/`user` lines when a message closes, and every permission
    /// question as a `control_request` answered on stdin
    /// (`--permission-prompt-tool stdio`; 2.1.272 read 2026-09-16).
    ClaudeStream,
}

/// The protocols by the name the catalog's wire road gives them
/// (`zerocode_core::agent::WireRoad::protocol`).
const PROTOCOL_NAMES: [(&str, Protocol); 3] = [
    ("app-server", Protocol::AppServer),
    ("acp", Protocol::Acp),
    ("claude-stream", Protocol::ClaudeStream),
];

impl Protocol {
    fn named(protocol: &str) -> Option<Self> {
        PROTOCOL_NAMES
            .iter()
            .find(|(name, _)| *name == protocol)
            .map(|(_, protocol)| *protocol)
    }

    fn name(self) -> &'static str {
        PROTOCOL_NAMES
            .iter()
            .find(|(_, protocol)| *protocol == self)
            .map_or("", |(name, _)| name)
    }

    /// JSON-RPC 2.0 on the wire (Codex, ACP); Claude Code's lines are its
    /// own event shapes.
    fn json_rpc(self) -> bool {
        !matches!(self, Self::ClaudeStream)
    }
}

/// One turn with the number the page keys its row by.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SeqTurn {
    pub(crate) seq: u64,
    #[serde(flatten)]
    pub(crate) turn: TranscriptTurn,
}

/// One choice the wire offers on a question — the agent's own words when it
/// gave them (ACP `options[].name`), the decision's kind otherwise, which the
/// window puts its own word to.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireOption {
    pub(crate) id: String,
    /// `allow_once` · `allow_always` · `reject_once` · `reject_always`.
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
}

/// One question of a `request_user_input`, in the ask card's shape.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireQuestion {
    pub(crate) id: String,
    pub(crate) header: String,
    pub(crate) question: String,
    pub(crate) options: Vec<WireQuestionOption>,
    pub(crate) other: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireQuestionOption {
    pub(crate) label: String,
    pub(crate) description: String,
}

/// A question the agent asked over the wire, waiting on the person.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireAsk {
    /// The JSON-RPC id of the agent's request — the answer wears it.
    pub(crate) id: serde_json::Value,
    /// `approval` (pick one option) or `question` (answer each question).
    pub(crate) kind: &'static str,
    pub(crate) method: String,
    pub(crate) tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    /// The plan this permission asks approval for, when the tool asking is
    /// the agent's plan tool (`ask::plan_in`) — the card draws it as the
    /// extension's plan review rather than as a summary line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) plan: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) edits: Vec<TranscriptEdit>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) options: Vec<WireOption>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) questions: Vec<WireQuestion>,
    /// What a granted answer carries back (Codex `item/permissions`: the
    /// requested profile), kept so the reply can repeat it.
    #[serde(skip)]
    pub(crate) grant: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireModel {
    pub(crate) id: String,
    pub(crate) display_name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireMode {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireCommand {
    pub(crate) name: String,
    pub(crate) about: String,
}

/// A tool item the agent is still running: what it is, its output so far.
#[derive(Debug, Clone, Default)]
struct OpenItem {
    kind: String,
    output: String,
    /// The edits a file change described when it started, for the approval
    /// that asks about it and the result that closes it.
    edits: Vec<TranscriptEdit>,
    paths: Vec<String>,
}

/// Everything the page reads about a session, and what the adapter needs
/// between messages.
#[derive(Debug, Default)]
pub(crate) struct WireState {
    turns: Vec<SeqTurn>,
    next_seq: u64,
    /// `starting` · `working` · `asking` · `idle` · `failed` · `ended`.
    pub(crate) status: &'static str,
    pub(crate) asks: Vec<WireAsk>,
    /// The tool THIS agent asks plan approval with, taken from the catalog
    /// when the session started (`AgentVoice::plan_tool`). Kept beside the
    /// state because the adapter reads messages and knows no agent; the
    /// catalog is still the one place the fact is written.
    pub(crate) plan_tool: Option<&'static str>,
    pub(crate) model: Option<String>,
    pub(crate) models: Vec<WireModel>,
    pub(crate) mode: Option<String>,
    pub(crate) modes: Vec<WireMode>,
    pub(crate) commands: Vec<WireCommand>,
    pub(crate) version: Option<String>,
    /// The thread (Codex) or session (ACP) the wire opened.
    pub(crate) thread: Option<String>,
    /// The turn out right now (Codex names them; interrupt needs the id).
    pub(crate) turn: Option<String>,
    /// The id of the `session/prompt` request whose answer ends the turn (ACP).
    prompt_request: Option<serde_json::Value>,
    open: HashMap<String, OpenItem>,
    /// What the agent is saying right now, delta by delta, before the
    /// message closes into a turn — the words the page streams. Thought and
    /// answer stand apart, in the order they came.
    pub(crate) live: Vec<WireLive>,
    /// ACP: the ways the agent can be authenticated, from `initialize`.
    pub(crate) auth_methods: Vec<String>,
    /// How full the session's context is, when the CLI says — the composer's
    /// meter. `None` for a wire that never reports one (ACP says nothing
    /// about tokens), and the meter then does not stand.
    pub(crate) usage: Option<WireUsage>,
    /// The helpers the session runs, while they run — the rows at the foot
    /// of its page (t-6323 A6).
    pub(crate) tasks: Vec<WireTask>,
    /// How many times the helpers moved: the page's mark for them.
    task_moves: u64,
    /// The images the session's tools handed back, newest last, each under
    /// its own key — held here because the stream is the only place they
    /// were ever written, and bounded ([`WIRE_IMAGE_CAP`]) because a page
    /// shows the last few and a long session sends many (t-6323 A8).
    images: std::collections::VecDeque<(u64, String)>,
    image_seq: u64,
    /// The transcript of the pane whose conversation this session continues
    /// (`wire_start`'s `from_pane`): the page opens with that pane's turns,
    /// and the pictures they name stand in that file (t-6323 A8).
    history: Option<std::path::PathBuf>,
}

/// Where a picture the page asks a session for stands: kept by the session
/// (`wire:<n>`, what its tools handed back), or in the transcript of the pane
/// it continues (`"<offset>:<len>"`, what that pane's history named).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WireImage {
    Held(String),
    InFile(std::path::PathBuf),
}

/// How many images a session keeps for its page to fetch.
const WIRE_IMAGE_CAP: usize = 24;

/// A helper Claude Code runs for the session — a local agent, from its
/// `task_started` frame to its end — in the words its frames give it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WireTask {
    pub(crate) task: String,
    /// The Agent call that spawned it (`tool_use_id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) call: Option<String>,
    pub(crate) description: String,
    /// Its own account of how far it got, once it gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    /// Its latest step: the progress frame's own words, or the tool it
    /// last used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) step: Option<String>,
    pub(crate) tokens: u64,
    pub(crate) tools: u64,
    /// When the window heard it start, in ms since the epoch — its row's
    /// clock.
    pub(crate) started_ms: u64,
    /// Whether it runs in the background: a roll call that stops listing a
    /// background helper ends it. Provenance, kept off the wire.
    #[serde(skip)]
    backgrounded: Option<bool>,
}

/// What a session says about the context it carries: the tokens the last
/// request stood on, and the window they stand in.
///
/// `window` is `0` where the CLI names no window — the chip then says the
/// count and draws no ring, because a ring over a made-up denominator cannot
/// tell "nearly full" from "just started". The same two numbers come off a
/// transcript (`zerocode_core::transcript::TranscriptUsage`) and off zo's
/// channel, so one chip reads all three roads without asking which it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct WireUsage {
    pub(crate) tokens: u64,
    pub(crate) window: u64,
}

/// One voice the agent is streaming in — `thinking` or `assistant` — and its
/// words so far.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WireLive {
    pub(crate) role: &'static str,
    pub(crate) text: String,
    /// Whether the voice finished these words: a wire closes its live text
    /// into a turn itself (`false` here, always), while a pane's channel says
    /// `done` on the last delta and its transcript carries the turn later —
    /// the page keeps a done piece standing until that turn arrives.
    pub(crate) done: bool,
}

/// What the page draws, as one comparable value: the settled part (turns,
/// status, questions, the head's words) and the live text's length.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireMark {
    turns: u64,
    status: &'static str,
    asks: usize,
    model: Option<String>,
    mode: Option<String>,
    commands: usize,
    /// The context reading the page's meter draws: a settled fact, so a new
    /// one is news even in a turn that said nothing else.
    usage: Option<WireUsage>,
    /// The helpers' moves: a helper that started, stepped or ended is news.
    tasks: u64,
    live: usize,
}

impl WireMark {
    /// Whether everything but the live text is the same.
    fn settled_as(&self, other: &Self) -> bool {
        self.turns == other.turns
            && self.status == other.status
            && self.asks == other.asks
            && self.model == other.model
            && self.mode == other.mode
            && self.commands == other.commands
            && self.usage == other.usage
            && self.tasks == other.tasks
    }
}

/// What the adapter wants sent back down the wire after a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outgoing {
    Reply {
        id: serde_json::Value,
        result: serde_json::Value,
    },
    Refuse {
        id: serde_json::Value,
        message: String,
    },
}

impl WireState {
    fn push(&mut self, turn: TranscriptTurn) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.turns.push(SeqTurn { seq, turn });
        if self.turns.len() > TURN_CAP {
            let drop = self.turns.len() - TURN_CAP;
            self.turns.drain(..drop);
        }
    }

    fn say(&mut self, role: &str, text: impl Into<String>) {
        let text: String = text.into();
        if text.trim().is_empty() {
            return;
        }
        self.push(TranscriptTurn {
            role: role.to_string(),
            text,
            at_ms: None,
            tool: None,
            images: Vec::new(),
        });
    }

    fn call(
        &mut self,
        call_id: &str,
        name: &str,
        target: &str,
        input: String,
        edits: Vec<TranscriptEdit>,
    ) {
        let text = if target.is_empty() {
            name.to_string()
        } else {
            format!("{name} · {}", zerocode_core::transcript::clamp(target))
        };
        // The file the call names, read off its input the way a transcript's
        // call is (`file_in`): Codex's and ACP's calls say it in fields too.
        let file = zerocode_core::transcript::file_in(
            name,
            Some(&serde_json::Value::String(input.clone())),
        );
        self.push(TranscriptTurn {
            role: "tool".to_string(),
            text,
            at_ms: None,
            tool: Some(TranscriptTool {
                call_id: call_id.to_string(),
                name: name.to_string(),
                input,
                is_error: false,
                edits,
                file,
            }),
            images: Vec::new(),
        });
    }

    fn result(
        &mut self,
        call_id: &str,
        name: &str,
        text: String,
        is_error: bool,
        edits: Vec<TranscriptEdit>,
    ) {
        self.push(TranscriptTurn {
            role: "tool_result".to_string(),
            text,
            at_ms: None,
            tool: Some(TranscriptTool {
                call_id: call_id.to_string(),
                name: name.to_string(),
                input: String::new(),
                is_error,
                edits,
                file: None,
            }),
            images: Vec::new(),
        });
    }

    /// The images `turn` carries, taken out of `line` (where the reader set
    /// them aside) into the session's own keeping, each re-keyed `wire:<n>`.
    fn hold_images(&mut self, line: &str, turn: &mut TranscriptTurn) {
        for image in &mut turn.images {
            let Some((offset, len)) = image.at.split_once(':').and_then(|(offset, len)| {
                Some((offset.parse::<usize>().ok()?, len.parse::<usize>().ok()?))
            }) else {
                continue;
            };
            let Some(payload) = line.get(offset..offset + len) else {
                continue;
            };
            self.image_seq += 1;
            self.images.push_back((self.image_seq, payload.to_string()));
            if self.images.len() > WIRE_IMAGE_CAP {
                self.images.pop_front();
            }
            image.at = format!("wire:{}", self.image_seq);
        }
    }

    /// Where the picture at `at` stands, or why there is none: a `wire:<n>`
    /// key the session no longer keeps (pushed out by newer ones), or a file
    /// place when the session continues no pane.
    fn image(&self, at: &str) -> Result<WireImage, String> {
        let Some(key) = at.strip_prefix("wire:") else {
            return self
                .history
                .clone()
                .map(WireImage::InFile)
                .ok_or_else(|| format!("이미지 자리가 아닙니다: {at}"));
        };
        let key: u64 = key
            .parse()
            .map_err(|_| format!("이미지 자리가 아닙니다: {at}"))?;
        self.images
            .iter()
            .find(|(held, _)| *held == key)
            .map(|(_, payload)| WireImage::Held(payload.clone()))
            .ok_or_else(|| "이 이미지는 더 이상 없습니다".to_string())
    }

    /// A delta of what the agent is saying, onto the live text: the same
    /// voice grows, another voice starts a new piece after it.
    fn stream(&mut self, role: &'static str, delta: &str) {
        if delta.is_empty() {
            return;
        }
        match self.live.last_mut() {
            Some(last) if last.role == role => last.text.push_str(delta),
            _ => self.live.push(WireLive {
                role,
                text: delta.to_string(),
                done: false,
            }),
        }
    }

    /// The live text of one voice, taken out — what a closing item said when
    /// the close itself carries no text (Codex).
    fn take_live(&mut self, role: &str) -> String {
        let mut taken = String::new();
        self.live.retain(|piece| {
            if piece.role == role {
                taken.push_str(&piece.text);
                false
            } else {
                true
            }
        });
        taken
    }

    /// Everything live, closed into turns in the order it was said (ACP
    /// streams with no item boundary; the boundary is the next event).
    fn flush_live(&mut self) {
        for piece in std::mem::take(&mut self.live) {
            self.say(piece.role, piece.text);
        }
    }

    fn mark(&self) -> WireMark {
        WireMark {
            turns: self.next_seq,
            status: self.status,
            asks: self.asks.len(),
            model: self.model.clone(),
            mode: self.mode.clone(),
            commands: self.commands.len(),
            usage: self.usage,
            tasks: self.task_moves,
            live: self.live.len()
                + self
                    .live
                    .iter()
                    .map(|piece| piece.text.len())
                    .sum::<usize>(),
        }
    }

    /// The turns from `after` on, for the page.
    fn log_from(&self, after: u64) -> (Vec<TranscriptTurn>, u64, bool) {
        let first = self.turns.first().map_or(self.next_seq, |turn| turn.seq);
        let skipped = after < first;
        let turns: Vec<TranscriptTurn> = self
            .turns
            .iter()
            .filter(|turn| turn.seq >= after)
            .map(|turn| turn.turn.clone())
            .collect();
        (turns, self.next_seq, skipped)
    }
}

fn text_of(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

/// The prose in an ACP content block or a Codex user input.
fn block_text(block: &serde_json::Value) -> String {
    match block.get("type").and_then(serde_json::Value::as_str) {
        Some("text") => text_of(block.get("text")),
        Some("content") => block_text(block.get("content").unwrap_or(&serde_json::Value::Null)),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- Codex

/// The decisions Codex's approval requests take, in the extension's order —
/// yes, yes for the session, no — and the kinds the window words them by.
const CODEX_DECISIONS: [(&str, &str); 3] = [
    ("accept", "allow_once"),
    ("acceptForSession", "allow_always"),
    ("decline", "reject_once"),
];

fn codex_options() -> Vec<WireOption> {
    CODEX_DECISIONS
        .iter()
        .map(|(id, kind)| WireOption {
            id: (*id).to_string(),
            kind: (*kind).to_string(),
            label: None,
        })
        .collect()
}

/// A Codex thread item as the page's turns: what it is called, its target,
/// its detail and the edits it carries.
fn codex_item_words(
    item: &serde_json::Value,
) -> Option<(String, String, String, Vec<TranscriptEdit>)> {
    let kind = item.get("type").and_then(serde_json::Value::as_str)?;
    Some(match kind {
        "commandExecution" => {
            let command = text_of(item.get("command"));
            (
                "shell".to_string(),
                first_line(&command).to_string(),
                command,
                Vec::new(),
            )
        }
        "fileChange" => {
            let changes = item.get("changes").and_then(serde_json::Value::as_array);
            let mut edits = Vec::new();
            let mut paths = Vec::new();
            for change in changes.into_iter().flatten() {
                let path = text_of(change.get("path"));
                let diff = text_of(change.get("diff"));
                let lines = crate::scm_runtime::parse_unified_diff(&diff);
                edits.push(TranscriptEdit::at(&path, lines));
                paths.push(path);
            }
            (
                "apply_patch".to_string(),
                paths.join(", "),
                paths.join("\n"),
                edits,
            )
        }
        "mcpToolCall" => {
            let name = format!(
                "{}.{}",
                text_of(item.get("server")),
                text_of(item.get("tool"))
            );
            let arguments = item
                .get("arguments")
                .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
                .unwrap_or_default();
            (
                name,
                first_line(&arguments).to_string(),
                arguments,
                Vec::new(),
            )
        }
        "dynamicToolCall" => {
            let name = text_of(item.get("tool"));
            let arguments = item
                .get("arguments")
                .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
                .unwrap_or_default();
            (
                name,
                first_line(&arguments).to_string(),
                arguments,
                Vec::new(),
            )
        }
        _ => return None,
    })
}

fn codex_item_failed(item: &serde_json::Value) -> bool {
    let status = item
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    status == "failed"
        || status == "declined"
        || item
            .get("exitCode")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|code| code != 0)
}

fn codex_item_output(item: &serde_json::Value, open: Option<&OpenItem>) -> String {
    let said = text_of(item.get("aggregatedOutput"));
    if !said.is_empty() {
        return said;
    }
    if let Some(result) = item.get("result").filter(|value| !value.is_null()) {
        return match result {
            serde_json::Value::String(text) => text.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        };
    }
    if let Some(error) = item.get("error").filter(|value| !value.is_null()) {
        return text_of(error.get("message"));
    }
    if item.get("type").and_then(serde_json::Value::as_str) == Some("fileChange") {
        let status = text_of(item.get("status"));
        return format!("{} · {status}", open.map_or(0, |open| open.paths.len()));
    }
    open.map(|open| open.output.clone()).unwrap_or_default()
}

fn codex_take(state: &mut WireState, message: &serde_json::Value) -> Vec<Outgoing> {
    let method = message.get("method").and_then(serde_json::Value::as_str);
    let params = message.get("params").unwrap_or(&serde_json::Value::Null);
    let mut out = Vec::new();
    if let (Some(method), Some(id)) = (method, message.get("id")) {
        // The agent asks; the person answers, or this side does when it can.
        match method {
            "item/commandExecution/requestApproval" => {
                let command = text_of(params.get("command"));
                let reason = text_of(params.get("reason"));
                state.asks.push(WireAsk {
                    id: id.clone(),
                    kind: "approval",
                    method: method.to_string(),
                    tool: "shell".to_string(),
                    summary: Some(if command.is_empty() { reason } else { command }),
                    // Only Claude Code names a plan tool today; the other
                    // adapters ask ordinary approvals.
                    plan: None,
                    edits: Vec::new(),
                    options: codex_options(),
                    questions: Vec::new(),
                    grant: None,
                });
                state.status = "asking";
            }
            "item/fileChange/requestApproval" => {
                let item = text_of(params.get("itemId"));
                let open = state.open.get(&item);
                state.asks.push(WireAsk {
                    id: id.clone(),
                    kind: "approval",
                    method: method.to_string(),
                    plan: None,
                    tool: "apply_patch".to_string(),
                    summary: Some(open.map_or_else(
                        || text_of(params.get("reason")),
                        |open| open.paths.join(", "),
                    )),
                    edits: open.map(|open| open.edits.clone()).unwrap_or_default(),
                    options: codex_options(),
                    questions: Vec::new(),
                    grant: None,
                });
                state.status = "asking";
            }
            "item/permissions/requestApproval" => {
                let reason = text_of(params.get("reason"));
                let permissions = params.get("permissions").cloned();
                state.asks.push(WireAsk {
                    id: id.clone(),
                    kind: "approval",
                    method: method.to_string(),
                    plan: None,
                    tool: "permissions".to_string(),
                    summary: Some(if reason.is_empty() {
                        permissions
                            .as_ref()
                            .map(|value| serde_json::to_string(value).unwrap_or_default())
                            .unwrap_or_default()
                    } else {
                        reason
                    }),
                    edits: Vec::new(),
                    options: vec![
                        WireOption {
                            id: "turn".into(),
                            kind: "allow_once".into(),
                            label: None,
                        },
                        WireOption {
                            id: "session".into(),
                            kind: "allow_always".into(),
                            label: None,
                        },
                        WireOption {
                            id: "decline".into(),
                            kind: "reject_once".into(),
                            label: None,
                        },
                    ],
                    questions: Vec::new(),
                    grant: permissions,
                });
                state.status = "asking";
            }
            "item/tool/requestUserInput" => {
                let questions = params
                    .get("questions")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|question| WireQuestion {
                        id: text_of(question.get("id")),
                        header: text_of(question.get("header")),
                        question: text_of(question.get("question")),
                        options: question
                            .get("options")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                            .map(|option| WireQuestionOption {
                                label: text_of(option.get("label")),
                                description: text_of(option.get("description")),
                            })
                            .collect(),
                        other: question
                            .get("isOther")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    })
                    .collect();
                state.asks.push(WireAsk {
                    id: id.clone(),
                    kind: "question",
                    method: method.to_string(),
                    tool: "request_user_input".to_string(),
                    summary: None,
                    plan: None,
                    edits: Vec::new(),
                    options: Vec::new(),
                    questions,
                    grant: None,
                });
                state.status = "asking";
            }
            "mcpServer/elicitation/request" => out.push(Outgoing::Reply {
                id: id.clone(),
                result: serde_json::json!({ "action": "decline" }),
            }),
            other => out.push(Outgoing::Refuse {
                id: id.clone(),
                message: format!("{other} is not answered by this window"),
            }),
        }
        return out;
    }
    let Some(method) = method else {
        return out;
    };
    match method {
        "thread/started" => {
            let thread = params.get("thread").unwrap_or(&serde_json::Value::Null);
            state.thread = thread
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            if let Some(model) = thread.get("model").and_then(serde_json::Value::as_str) {
                state.model = Some(model.to_string());
            }
            if let Some(version) = thread.get("cliVersion").and_then(serde_json::Value::as_str) {
                state.version = Some(version.to_string());
            }
        }
        "turn/started" => {
            state.turn = params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            state.status = "working";
        }
        "turn/completed" => {
            let turn = params.get("turn").unwrap_or(&serde_json::Value::Null);
            let failed = turn.get("status").and_then(serde_json::Value::as_str) == Some("failed");
            if let Some(message) = turn
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
            {
                state.say("system", message);
            }
            state.turn = None;
            state.live.clear();
            state.status = if failed { "failed" } else { "idle" };
        }
        "thread/tokenUsage/updated" => {
            // Codex's app-server announces the thread's token usage on its
            // own notification (`ThreadTokenUsageUpdatedNotification` →
            // `ThreadTokenUsage`, read off `codex app-server
            // generate-json-schema` for 0.155.1 on 2026-09-21). `last` is the
            // request that just finished — what the next one will carry, so
            // the context; `total` is the thread's whole spend, which is not
            // a context and would read as a meter that only ever fills.
            // `modelContextWindow` is null where the model's window is not
            // known, and 0 then says so.
            let told = params.get("tokenUsage").unwrap_or(&serde_json::Value::Null);
            let tokens = told
                .get("last")
                .and_then(|last| last.get("totalTokens"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if tokens > 0 {
                state.usage = Some(WireUsage {
                    tokens,
                    window: told
                        .get("modelContextWindow")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                });
            }
        }
        "thread/status/changed" => {
            let status = params.get("status").unwrap_or(&serde_json::Value::Null);
            let flags = status
                .get("activeFlags")
                .and_then(serde_json::Value::as_array);
            state.status = match status.get("type").and_then(serde_json::Value::as_str) {
                Some("active") if flags.is_some_and(|flags| !flags.is_empty()) => "asking",
                Some("active") => "working",
                Some("systemError") => "failed",
                Some("idle") if state.turn.is_none() => "idle",
                _ => state.status,
            };
        }
        "error" => {
            let error = params.get("error").unwrap_or(&serde_json::Value::Null);
            state.say("system", text_of(error.get("message")));
            if params.get("willRetry").and_then(serde_json::Value::as_bool) != Some(true) {
                state.status = "failed";
            }
        }
        "item/started" => {
            let item = params.get("item").unwrap_or(&serde_json::Value::Null);
            let id = text_of(item.get("id"));
            let kind = text_of(item.get("type"));
            let mut open = OpenItem {
                kind: kind.clone(),
                ..OpenItem::default()
            };
            if let Some((name, target, input, edits)) = codex_item_words(item) {
                if kind == "fileChange" {
                    open.paths = input.lines().map(str::to_string).collect();
                    open.edits.clone_from(&edits);
                }
                state.call(&id, &name, &target, input, edits);
            }
            state.open.insert(id, open);
        }
        "item/agentMessage/delta" => {
            state.stream("assistant", &text_of(params.get("delta")));
        }
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            state.stream("thinking", &text_of(params.get("delta")));
        }
        "item/commandExecution/outputDelta" => {
            let id = text_of(params.get("itemId"));
            let delta = text_of(params.get("delta"));
            state.open.entry(id).or_default().output.push_str(&delta);
        }
        "item/completed" => {
            let item = params.get("item").unwrap_or(&serde_json::Value::Null);
            let id = text_of(item.get("id"));
            let open = state.open.remove(&id);
            match item.get("type").and_then(serde_json::Value::as_str) {
                Some("agentMessage") | Some("plan") => {
                    let streamed = state.take_live("assistant");
                    let text = text_of(item.get("text"));
                    let text = if text.is_empty() { streamed } else { text };
                    state.say("assistant", text);
                }
                Some("reasoning") => {
                    let summary: Vec<String> = item
                        .get("summary")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect();
                    let streamed = state.take_live("thinking");
                    let text = if summary.is_empty() {
                        streamed
                    } else {
                        summary.join("\n\n")
                    };
                    state.say("thinking", text);
                }
                Some("commandExecution")
                | Some("fileChange")
                | Some("mcpToolCall")
                | Some("dynamicToolCall") => {
                    let name = codex_item_words(item)
                        .map(|(name, ..)| name)
                        .unwrap_or_default();
                    let output = codex_item_output(item, open.as_ref());
                    let edits = open.map(|open| open.edits).unwrap_or_default();
                    state.result(&id, &name, output, codex_item_failed(item), edits);
                }
                _ => {}
            }
        }
        _ => {}
    }
    out
}

/// The reply to one of Codex's questions, from what the person chose.
fn codex_answer(ask: &WireAsk, option: Option<&str>, answers: &[Vec<String>]) -> serde_json::Value {
    match ask.method.as_str() {
        "item/tool/requestUserInput" => {
            let mut map = serde_json::Map::new();
            for (question, given) in ask.questions.iter().zip(answers.iter()) {
                map.insert(question.id.clone(), serde_json::json!({ "answers": given }));
            }
            serde_json::json!({ "answers": map })
        }
        "item/permissions/requestApproval" => match option {
            Some("turn") | Some("session") => serde_json::json!({
                "permissions": ask.grant.clone().unwrap_or_else(|| serde_json::json!({})),
                "scope": option.unwrap_or("turn"),
            }),
            _ => serde_json::json!({ "permissions": {}, "scope": "turn" }),
        },
        _ => serde_json::json!({ "decision": option.unwrap_or("decline") }),
    }
}

// ---------------------------------------------------------------- ACP

fn acp_edits(content: Option<&serde_json::Value>) -> Vec<TranscriptEdit> {
    content
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("diff"))
        .filter_map(|block| {
            let path = text_of(block.get("path"));
            let old = block
                .get("oldText")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            TranscriptEdit::replaced(&path, old, &text_of(block.get("newText")))
        })
        .collect()
}

fn acp_content_text(content: Option<&serde_json::Value>) -> String {
    content
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(block_text)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool call's name and target from what ACP gives: the kind the agent
/// filed it under (`execute`, `read`, `edit`…) and its own title.
fn acp_call_words(call: &serde_json::Value) -> (String, String) {
    let raw_name = call
        .get("rawInput")
        .and_then(|input| input.get("name"))
        .and_then(serde_json::Value::as_str);
    let name = raw_name
        .map(str::to_string)
        .unwrap_or_else(|| text_of(call.get("kind")));
    let name = if name.is_empty() {
        "tool".to_string()
    } else {
        name
    };
    (name, text_of(call.get("title")))
}

fn acp_take(state: &mut WireState, message: &serde_json::Value) -> Vec<Outgoing> {
    let method = message.get("method").and_then(serde_json::Value::as_str);
    let params = message.get("params").unwrap_or(&serde_json::Value::Null);
    let mut out = Vec::new();
    if let (Some(method), Some(id)) = (method, message.get("id")) {
        match method {
            "session/request_permission" => {
                let call = params.get("toolCall").unwrap_or(&serde_json::Value::Null);
                let (tool, title) = acp_call_words(call);
                state.flush_live();
                state.asks.push(WireAsk {
                    id: id.clone(),
                    kind: "approval",
                    method: method.to_string(),
                    tool,
                    summary: (!title.is_empty()).then_some(title),
                    plan: None,
                    edits: acp_edits(call.get("content")),
                    options: params
                        .get("options")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|option| WireOption {
                            id: text_of(option.get("optionId")),
                            kind: text_of(option.get("kind")),
                            label: Some(text_of(option.get("name")))
                                .filter(|name| !name.is_empty()),
                        })
                        .collect(),
                    questions: Vec::new(),
                    grant: None,
                });
                state.status = "asking";
            }
            other => out.push(Outgoing::Refuse {
                id: id.clone(),
                message: format!("{other} is not offered by this client"),
            }),
        }
        return out;
    }
    if method == Some("session/update") {
        let update = params.get("update").unwrap_or(&serde_json::Value::Null);
        match update
            .get("sessionUpdate")
            .and_then(serde_json::Value::as_str)
        {
            Some("agent_message_chunk") => {
                state.stream(
                    "assistant",
                    &block_text(update.get("content").unwrap_or(&serde_json::Value::Null)),
                );
            }
            Some("agent_thought_chunk") => {
                state.stream(
                    "thinking",
                    &block_text(update.get("content").unwrap_or(&serde_json::Value::Null)),
                );
            }
            Some("tool_call") => {
                state.flush_live();
                let id = text_of(update.get("toolCallId"));
                let (name, title) = acp_call_words(update);
                let input = update
                    .get("rawInput")
                    .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
                    .unwrap_or_default();
                let edits = acp_edits(update.get("content"));
                state.open.insert(
                    id.clone(),
                    OpenItem {
                        kind: name.clone(),
                        ..OpenItem::default()
                    },
                );
                state.call(&id, &name, &title, input, edits);
                if matches!(
                    update.get("status").and_then(serde_json::Value::as_str),
                    Some("completed") | Some("failed")
                ) {
                    let failed =
                        update.get("status").and_then(serde_json::Value::as_str) == Some("failed");
                    let text = acp_content_text(update.get("content"));
                    state.open.remove(&id);
                    state.result(&id, &name, text, failed, Vec::new());
                }
            }
            Some("tool_call_update") => {
                let id = text_of(update.get("toolCallId"));
                let status = update.get("status").and_then(serde_json::Value::as_str);
                if matches!(status, Some("completed") | Some("failed")) {
                    let name = state
                        .open
                        .remove(&id)
                        .map(|open| open.kind)
                        .unwrap_or_default();
                    let mut text = acp_content_text(update.get("content"));
                    if text.is_empty()
                        && let Some(raw) = update.get("rawOutput")
                    {
                        text = match raw {
                            serde_json::Value::String(text) => text.clone(),
                            other => serde_json::to_string_pretty(other).unwrap_or_default(),
                        };
                    }
                    state.result(
                        &id,
                        &name,
                        text,
                        status == Some("failed"),
                        acp_edits(update.get("content")),
                    );
                }
            }
            Some("available_commands_update") => {
                state.commands = update
                    .get("availableCommands")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|command| WireCommand {
                        name: format!("/{}", text_of(command.get("name")).trim_start_matches('/')),
                        about: text_of(command.get("description")),
                    })
                    .collect();
            }
            Some("current_mode_update") => {
                state.mode = update
                    .get("currentModeId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    out
}

/// ACP's models and modes as `session/new` handed them back.
fn acp_session_opened(state: &mut WireState, result: &serde_json::Value) {
    state.thread = result
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    if let Some(models) = result.get("models") {
        state.model = models
            .get("currentModelId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        state.models = models
            .get("availableModels")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|model| WireModel {
                id: text_of(model.get("modelId")),
                display_name: text_of(model.get("name")),
            })
            .collect();
    }
    if let Some(modes) = result.get("modes") {
        state.mode = modes
            .get("currentModeId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        state.modes = modes
            .get("availableModes")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|mode| WireMode {
                id: text_of(mode.get("id")),
                name: text_of(mode.get("name")),
            })
            .collect();
    }
}

/// The reply to an ACP permission request.
fn acp_answer(option: Option<&str>) -> serde_json::Value {
    match option {
        Some(option) => {
            serde_json::json!({ "outcome": { "outcome": "selected", "optionId": option } })
        }
        None => serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

// ---------------------------------------------------------------- Claude Code

/// The decisions a Claude Code permission question takes — allow this call,
/// allow it and every call like it for the session (the rules the CLI itself
/// proposed in `permission_suggestions`), deny — and the kinds the window
/// words them by.
const CLAUDE_DECISIONS: [(&str, &str); 3] = [
    ("allow", "allow_once"),
    ("allow_always", "allow_always"),
    ("deny", "reject_once"),
];

fn claude_options(with_suggestions: bool) -> Vec<WireOption> {
    CLAUDE_DECISIONS
        .iter()
        .filter(|(id, _)| with_suggestions || *id != "allow_always")
        .map(|(id, kind)| WireOption {
            id: (*id).to_string(),
            kind: (*kind).to_string(),
            label: None,
        })
        .collect()
}

/// A Claude Code `can_use_tool` request as the card's question: an approval
/// described by the one reader the pane view's cards use
/// (`ask::approval_in_payload` — its summary and its diff), or the questions
/// of an `AskUserQuestion` (`ask::questions_shape`), which the CLI also asks
/// through permission.
fn claude_ask(
    id: serde_json::Value,
    request: &serde_json::Value,
    plan_tool: Option<&str>,
) -> WireAsk {
    let tool = text_of(request.get("tool_name"));
    let input = request
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let suggestions = request
        .get("permission_suggestions")
        .cloned()
        .filter(|value| value.as_array().is_some_and(|rows| !rows.is_empty()));
    let grant = Some(serde_json::json!({ "input": input, "suggestions": suggestions }));
    if let Some(prompt) = zerocode_core::ask::questions_shape(&input) {
        return WireAsk {
            id,
            kind: "question",
            method: "can_use_tool".to_string(),
            tool,
            summary: None,
            plan: None,
            edits: Vec::new(),
            options: Vec::new(),
            questions: prompt
                .questions
                .into_iter()
                .map(|question| WireQuestion {
                    id: question.question.clone(),
                    header: question.header.unwrap_or_default(),
                    question: question.question,
                    options: question
                        .options
                        .into_iter()
                        .map(|option| WireQuestionOption {
                            label: option.label,
                            description: option.description.unwrap_or_default(),
                        })
                        .collect(),
                    other: true,
                })
                .collect(),
            grant,
        };
    }
    let described = zerocode_core::ask::approval_in_payload(
        &serde_json::json!({ "tool_name": tool, "tool_input": input }).to_string(),
    );
    let tool = described
        .as_ref()
        .map_or(tool, |prompt| prompt.tool.clone());
    WireAsk {
        id,
        // The plan the CLI asks approval for, when this IS its plan tool:
        // the catalog's fact, applied by the caller as it is on the hook
        // road (`ask::plan_in`).
        plan: zerocode_core::ask::plan_in(Some(&input), &tool, plan_tool),
        kind: "approval",
        method: "can_use_tool".to_string(),
        tool,
        summary: described
            .as_ref()
            .and_then(|prompt| prompt.summary.clone())
            .or_else(|| {
                Some(text_of(request.get("description"))).filter(|words| !words.is_empty())
            }),
        edits: described.map(|prompt| prompt.edits).unwrap_or_default(),
        options: claude_options(suggestions.is_some()),
        questions: Vec::new(),
        grant,
    }
}

/// Claude Code's stream-json line, taken into the state: the lines its
/// transcript is made of (`assistant`, `user`) become turns through the one
/// transcript reader the pane view uses; `stream_event` deltas are the live
/// text; `control_request can_use_tool` is the question its permission
/// dialog would have asked; `result` ends the turn.
fn claude_take(state: &mut WireState, message: &serde_json::Value) -> Vec<Outgoing> {
    let mut out = Vec::new();
    // A subagent's stream (Task) is not this conversation: the page shows
    // the call that started it as running until its result, as the CLI does.
    if message
        .get("parent_tool_use_id")
        .is_some_and(|parent| !parent.is_null())
    {
        return out;
    }
    let kind = message.get("type").and_then(serde_json::Value::as_str);
    match kind {
        Some("system") => match message.get("subtype").and_then(serde_json::Value::as_str) {
            Some("init") => {
                state.thread = message
                    .get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                if let Some(model) = message.get("model").and_then(serde_json::Value::as_str) {
                    state.model = Some(model.to_string());
                }
                if let Some(mode) = message
                    .get("permissionMode")
                    .and_then(serde_json::Value::as_str)
                {
                    state.mode = Some(mode.to_string());
                }
                if let Some(version) = message
                    .get("claude_code_version")
                    .and_then(serde_json::Value::as_str)
                {
                    state.version = Some(version.to_string());
                }
                state.commands = message
                    .get("slash_commands")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(|name| WireCommand {
                        name: format!("/{}", name.trim_start_matches('/')),
                        about: String::new(),
                    })
                    .collect();
            }
            Some("status") => {
                if let Some(mode) = message
                    .get("permissionMode")
                    .and_then(serde_json::Value::as_str)
                {
                    state.mode = Some(mode.to_string());
                }
            }
            Some(subtype)
                if subtype.starts_with("task_") || subtype == "background_tasks_changed" =>
            {
                claude_task(state, subtype, message);
            }
            _ => {}
        },
        Some("stream_event") => {
            let event = message.get("event").unwrap_or(&serde_json::Value::Null);
            match event.get("type").and_then(serde_json::Value::as_str) {
                Some("message_start") => {
                    if let Some(model) = event
                        .get("message")
                        .and_then(|message| message.get("model"))
                        .and_then(serde_json::Value::as_str)
                    {
                        state.model = Some(model.to_string());
                    }
                    state.status = "working";
                }
                Some("content_block_delta") => {
                    let delta = event.get("delta").unwrap_or(&serde_json::Value::Null);
                    match delta.get("type").and_then(serde_json::Value::as_str) {
                        Some("text_delta") => {
                            state.stream("assistant", &text_of(delta.get("text")));
                        }
                        Some("thinking_delta") => {
                            state.stream("thinking", &text_of(delta.get("thinking")));
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Some("assistant") | Some("user") => {
            // A transcript line; the transcript reader makes its turns
            // (thought, answer, tool call, tool result). The person's own
            // words were said when they were sent. Its payloads step aside
            // first, as a file's do, and the wire keeps the images (A8).
            let line = message.to_string();
            let read = zerocode_core::transcript::elide_payloads(line.as_bytes(), 0);
            for mut turn in zerocode_core::transcript::turns_in(&String::from_utf8_lossy(&read)) {
                if turn.role != "user" {
                    state.hold_images(&line, &mut turn);
                    state.push(turn);
                }
            }
            if kind == Some("assistant") {
                state.live.clear();
            }
        }
        Some("control_request") => {
            let id = message
                .get("request_id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let request = message.get("request").unwrap_or(&serde_json::Value::Null);
            match request.get("subtype").and_then(serde_json::Value::as_str) {
                Some("can_use_tool") => {
                    state.flush_live();
                    state.asks.push(claude_ask(id, request, state.plan_tool));
                    state.status = "asking";
                }
                Some(other) => out.push(Outgoing::Refuse {
                    id,
                    message: format!("{other} is not answered by this window"),
                }),
                None => {}
            }
        }
        Some("result") => {
            state.live.clear();
            if let Some(usage) = claude_result_usage(state, message) {
                state.usage = Some(usage);
            }
            let failed = message.get("is_error").and_then(serde_json::Value::as_bool) == Some(true);
            if failed {
                let words = message
                    .get("result")
                    .and_then(serde_json::Value::as_str)
                    .filter(|words| !words.is_empty())
                    .map_or_else(|| text_of(message.get("subtype")), str::to_string);
                state.say("system", words);
            }
            state.status = if failed { "failed" } else { "idle" };
        }
        _ => {}
    }
    out
}

/// Claude Code's helper frames (2.1.280, read off a live `-p` stream on
/// 2026-09-23): `task_started` names the Agent call that spawned a local
/// agent (`tool_use_id`), `task_progress` its tokens, tools and latest step,
/// `task_updated` an end in its patch, `task_notification` the end itself,
/// and `background_tasks_changed` the helpers still running in the
/// background. Kept as the extension keeps them (2.1.280 `handleTask*`):
/// local agents only; the latest step is the progress frame's own words
/// where they differ from the description, else the tool it last used, and
/// a frame with a summary moves the summary instead.
fn claude_task(state: &mut WireState, subtype: &str, message: &serde_json::Value) {
    let id = text_of(message.get("task_id"));
    if id.is_empty() && subtype != "background_tasks_changed" {
        return;
    }
    let words = |key: &str| Some(text_of(message.get(key))).filter(|said| !said.is_empty());
    match subtype {
        "task_started" => {
            if message.get("task_type").and_then(serde_json::Value::as_str) != Some("local_agent") {
                return;
            }
            state.tasks.retain(|task| task.task != id);
            state.tasks.push(WireTask {
                task: id,
                call: words("tool_use_id"),
                description: text_of(message.get("description")),
                summary: None,
                step: None,
                tokens: 0,
                tools: 0,
                started_ms: epoch_ms(),
                backgrounded: message
                    .get("is_backgrounded")
                    .and_then(serde_json::Value::as_bool),
            });
        }
        "task_progress" => {
            let Some(task) = state.tasks.iter_mut().find(|task| task.task == id) else {
                return;
            };
            let usage = message.get("usage");
            let count = |key: &str| {
                usage
                    .and_then(|usage| usage.get(key))
                    .and_then(serde_json::Value::as_u64)
            };
            task.tokens = count("total_tokens").unwrap_or(task.tokens);
            task.tools = count("tool_uses").unwrap_or(task.tools);
            match words("summary") {
                Some(summary) => task.summary = Some(summary),
                None => {
                    let step = words("description")
                        .filter(|said| *said != task.description)
                        .or_else(|| words("last_tool_name"));
                    if step.is_some() {
                        task.step = step;
                    }
                }
            }
        }
        "task_updated" => {
            let patch = message.get("patch");
            let status = patch
                .and_then(|patch| patch.get("status"))
                .and_then(serde_json::Value::as_str);
            let background = patch
                .and_then(|patch| patch.get("is_backgrounded"))
                .and_then(serde_json::Value::as_bool);
            if matches!(status, Some("completed" | "failed" | "killed")) {
                state.tasks.retain(|task| task.task != id);
            } else if let (Some(background), Some(task)) = (
                background,
                state.tasks.iter_mut().find(|task| task.task == id),
            ) {
                task.backgrounded = Some(background);
            } else {
                return;
            }
        }
        "task_notification" => state.tasks.retain(|task| task.task != id),
        "background_tasks_changed" => {
            let listed: Vec<&str> = message
                .get("tasks")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|task| task.get("task_id").and_then(serde_json::Value::as_str))
                .collect();
            state.tasks.retain(|task| {
                task.backgrounded != Some(true) || listed.contains(&task.task.as_str())
            });
        }
        _ => return,
    }
    state.task_moves += 1;
}

/// Now, in ms since the epoch — a helper's clock starts when its frame
/// arrives, as the extension's does (`startTime: Date.now()`).
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The context a Claude Code `result` line reports.
///
/// Measured 2026-09-21 against `claude -p --input-format stream-json
/// --output-format stream-json --verbose` (2.1.x on this machine): the result
/// frame carries `usage` and `modelUsage` at its TOP level, not inside a
/// `message`. The context is the last request's whole prompt —
/// `input_tokens` plus both cache counts, the three the token measurement
/// note adds (`zo-ide/docs/analysis/cc-token-measure-r16.md`); the output is
/// not context yet. The window is that model's own
/// (`modelUsage[<model>].contextWindow`, measured 1000000 for
/// `claude-fable-5-1`), and a result with no row for it — a local command
/// that never reached a model, where `modelUsage` came back `{}` — keeps the
/// window at 0 rather than inventing a denominator.
///
/// The extension subtracts `maxOutputTokens` and a constant of its own from
/// the window before drawing its pie. Neither number is ours to reuse: one
/// is the extension's arithmetic and the other is its literal, so this
/// reports the two numbers the CLI gave and lets the chip say what they are.
fn claude_result_usage(state: &WireState, message: &serde_json::Value) -> Option<WireUsage> {
    let usage = message.get("usage")?;
    let count = |name: &str| {
        usage
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let tokens = count("input_tokens")
        + count("cache_creation_input_tokens")
        + count("cache_read_input_tokens");
    if tokens == 0 {
        return None;
    }
    let window = state
        .model
        .as_deref()
        .and_then(|model| message.get("modelUsage")?.get(model))
        .and_then(|row| row.get("contextWindow"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Some(WireUsage { tokens, window })
}

/// The reply to a Claude Code permission question: an allow repeats the input
/// as the CLI proposed it, an allow for the session adds the CLI's own rule
/// suggestions, a question's answers ride the input, and a denial is a
/// sentence the model reads.
///
/// `message` is the person's own words on that refusal — the plan card's
/// feedback field (the extension's "Send feedback", 2.1.275). Given none, the
/// window's own sentence stands, which is what every other refusal sends.
fn claude_answer(
    ask: &WireAsk,
    option: Option<&str>,
    answers: &[Vec<String>],
    message: Option<&str>,
) -> serde_json::Value {
    let grant = ask.grant.clone().unwrap_or(serde_json::Value::Null);
    let mut input = grant
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if ask.kind == "question" {
        let mut given = serde_json::Map::new();
        for (question, answer) in ask.questions.iter().zip(answers.iter()) {
            given.insert(
                question.question.clone(),
                serde_json::Value::String(answer.join(", ")),
            );
        }
        if given.is_empty() {
            return serde_json::json!({
                "behavior": "deny",
                "message": "The person dismissed the question in the ZeroCode window.",
            });
        }
        if let Some(object) = input.as_object_mut() {
            object.insert("answers".into(), serde_json::Value::Object(given));
        }
        return serde_json::json!({ "behavior": "allow", "updatedInput": input });
    }
    match option {
        Some("allow") => serde_json::json!({ "behavior": "allow", "updatedInput": input }),
        Some("allow_always") => serde_json::json!({
            "behavior": "allow",
            "updatedInput": input,
            "updatedPermissions": grant
                .get("suggestions")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
        }),
        _ => serde_json::json!({
            "behavior": "deny",
            "message": message
                .filter(|words| !words.trim().is_empty())
                .unwrap_or("The person declined this tool call in the ZeroCode window."),
        }),
    }
}

/// The values a CLI's `--help` lists for `flag` — clap's `(choices: "a",
/// "b")`, wrapped over lines as the terminal width had it.
fn help_choices(lines: &[String], flag: &str) -> Vec<String> {
    let Some(start) = lines.iter().position(|line| line.contains(flag)) else {
        return Vec::new();
    };
    let text = lines[start..]
        .iter()
        .take(8)
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let Some(from) = text.find("(choices:") else {
        return Vec::new();
    };
    let listed = &text[from + "(choices:".len()..];
    listed
        .split(')')
        .next()
        .unwrap_or_default()
        .split(',')
        .map(|word| word.trim().trim_matches('"').to_string())
        .filter(|word| !word.is_empty())
        .collect()
}

/// One message from the wire, taken into the state. Pure: what to send back
/// comes out as [`Outgoing`].
pub(crate) fn take(
    protocol: Protocol,
    state: &mut WireState,
    message: &serde_json::Value,
) -> Vec<Outgoing> {
    match protocol {
        Protocol::AppServer => codex_take(state, message),
        Protocol::Acp => acp_take(state, message),
        Protocol::ClaudeStream => claude_take(state, message),
    }
}

/// A request of ours on the wire: JSON-RPC's, or Claude Code's
/// `control_request` whose `subtype` is the method and whose fields are the
/// params; the id travels as a string there, ours are numbers spelled out.
fn request_line(
    protocol: Protocol,
    id: i64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    if protocol.json_rpc() {
        return serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    }
    let mut request = match params {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    request.insert(
        "subtype".into(),
        serde_json::Value::String(method.to_string()),
    );
    serde_json::json!({ "type": "control_request", "request_id": id.to_string(), "request": request })
}

/// The answer to one of the agent's requests, in its protocol's shape.
fn reply_line(
    protocol: Protocol,
    id: &serde_json::Value,
    result: serde_json::Value,
) -> serde_json::Value {
    if protocol.json_rpc() {
        return serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
    }
    serde_json::json!({ "type": "control_response", "response": { "subtype": "success", "request_id": id, "response": result } })
}

fn refuse_line(protocol: Protocol, id: &serde_json::Value, message: &str) -> serde_json::Value {
    if protocol.json_rpc() {
        return serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": message } });
    }
    serde_json::json!({ "type": "control_response", "response": { "subtype": "error", "request_id": id, "error": message } })
}

/// A line that answers a request of ours: its id, and the answer in
/// JSON-RPC's shape (`result`, or `error.message`) whichever protocol
/// carried it.
fn response_of(
    protocol: Protocol,
    message: &serde_json::Value,
) -> Option<(i64, serde_json::Value)> {
    if protocol.json_rpc() {
        if message.get("method").is_some() {
            return None;
        }
        return message
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .map(|id| (id, message.clone()));
    }
    if message.get("type").and_then(serde_json::Value::as_str) != Some("control_response") {
        return None;
    }
    let response = message.get("response")?;
    let id = response.get("request_id")?.as_str()?.parse::<i64>().ok()?;
    let answer = if response.get("subtype").and_then(serde_json::Value::as_str) == Some("error") {
        serde_json::json!({ "error": { "message": text_of(response.get("error")) } })
    } else {
        serde_json::json!({ "result": response.get("response").cloned().unwrap_or(serde_json::Value::Null) })
    };
    Some((id, answer))
}

/// The turn ending on an ACP prompt's response, or a Codex request's error.
fn take_response(protocol: Protocol, state: &mut WireState, message: &serde_json::Value) {
    if protocol == Protocol::Acp
        && let Some(prompt) = state.prompt_request.as_ref()
        && message.get("id") == Some(prompt)
    {
        state.prompt_request = None;
        state.flush_live();
        state.open.clear();
        state.status = match message.get("error") {
            Some(error) if !error.is_null() => {
                let words = text_of(error.get("message"));
                state.say("system", words);
                "failed"
            }
            _ => "idle",
        };
    }
}

// ---------------------------------------------------------------- session

pub(crate) struct WireSession {
    pub(crate) id: WireId,
    pub(crate) agent: String,
    pub(crate) protocol: Protocol,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next_request: AtomicI64,
    pub(crate) state: Mutex<WireState>,
    replies: Mutex<HashMap<i64, mpsc::SyncSender<serde_json::Value>>>,
    /// Told each time the state changed in a way the page draws, so the page
    /// polls at once instead of on its next tick.
    changed: Box<dyn Fn(WireId) + Send + Sync>,
    /// When the live text was last announced (`LIVE_NOTIFY_GAP`).
    last_live_notice: Mutex<Instant>,
}

impl WireSession {
    /// A settled change: the page hears now.
    fn settled(&self) {
        if let Ok(mut at) = self.last_live_notice.lock() {
            *at = Instant::now();
        }
        (self.changed)(self.id);
    }

    /// Tell the page what a line changed: a settled change at once, a live
    /// one no more often than the gap.
    fn announce(&self, before: Option<WireMark>) {
        let Some(before) = before else {
            return;
        };
        let Some(now) = self.state.lock().ok().map(|state| state.mark()) else {
            return;
        };
        if !now.settled_as(&before) {
            self.settled();
            return;
        }
        if now.live == before.live {
            return;
        }
        let due = self.last_live_notice.lock().ok().is_some_and(|mut at| {
            if at.elapsed() >= LIVE_NOTIFY_GAP {
                *at = Instant::now();
                true
            } else {
                false
            }
        });
        if due {
            (self.changed)(self.id);
        }
    }

    fn write(&self, message: &serde_json::Value) -> Result<(), String> {
        let mut line = serde_json::to_string(message).map_err(|error| error.to_string())?;
        line.push('\n');
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| "wire stdin lock".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .map_err(|error| error.to_string())?;
        stdin.flush().map_err(|error| error.to_string())
    }

    fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), String> {
        self.write(&serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Send a request and hand back its id; the answer arrives on the reader
    /// thread and is either waited for ([`Self::request`]) or taken by the
    /// adapter (`take_response`).
    fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
        wait: Option<mpsc::SyncSender<serde_json::Value>>,
    ) -> Result<i64, String> {
        let id = self.next_request.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = wait {
            self.replies
                .lock()
                .map_err(|_| "wire replies lock".to_string())?
                .insert(id, sender);
        }
        self.write(&request_line(self.protocol, id, method, params))?;
        Ok(id)
    }

    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        wait: Duration,
    ) -> Result<serde_json::Value, String> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let id = self.send_request(method, params, Some(sender))?;
        let answer = receiver.recv_timeout(wait).map_err(|_| {
            let _ = self.replies.lock().map(|mut replies| replies.remove(&id));
            format!("{method}: no answer in {} s", wait.as_secs())
        })?;
        if let Some(error) = answer.get("error").filter(|error| !error.is_null()) {
            return Err(format!("{method}: {}", text_of(error.get("message"))));
        }
        Ok(answer
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    fn reply(&self, id: &serde_json::Value, result: serde_json::Value) -> Result<(), String> {
        self.write(&reply_line(self.protocol, id, result))
    }

    fn refuse(&self, id: &serde_json::Value, message: &str) -> Result<(), String> {
        self.write(&refuse_line(self.protocol, id, message))
    }

    fn take_line(self: &Arc<Self>, line: &str) {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        let before = self.state.lock().ok().map(|state| state.mark());
        if let Some((id, answer)) = response_of(self.protocol, &message) {
            let waiting = self
                .replies
                .lock()
                .ok()
                .and_then(|mut replies| replies.remove(&id));
            if let Some(sender) = waiting {
                let _ = sender.try_send(answer);
            } else if let Ok(mut state) = self.state.lock() {
                take_response(self.protocol, &mut state, &message);
            }
        } else {
            let outgoing = match self.state.lock() {
                Ok(mut state) => take(self.protocol, &mut state, &message),
                Err(_) => Vec::new(),
            };
            for item in outgoing {
                let _ = match item {
                    Outgoing::Reply { id, result } => self.reply(&id, result),
                    Outgoing::Refuse { id, message } => self.refuse(&id, &message),
                };
            }
        }
        self.announce(before);
    }

    pub(crate) fn send(&self, text: &str) -> Result<(), String> {
        let (thread, protocol) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "wire state lock".to_string())?;
            (state.thread.clone(), self.protocol)
        };
        // Claude Code has no thread to name until its first turn; the other
        // two address the thread the handshake opened.
        let thread = if protocol.json_rpc() {
            thread.ok_or("the wire has no thread yet")?
        } else {
            thread.unwrap_or_default()
        };
        match protocol {
            Protocol::AppServer => {
                let mut params = serde_json::json!({ "threadId": thread, "input": [{ "type": "text", "text": text }] });
                if let Some(model) = self.state.lock().ok().and_then(|state| state.model.clone()) {
                    params["model"] = serde_json::Value::String(model);
                }
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| "wire state lock".to_string())?;
                state.say("user", text);
                state.status = "working";
                drop(state);
                self.settled();
                let turn = self.request("turn/start", params, REPLY_WAIT)?;
                if let Ok(mut state) = self.state.lock() {
                    state.turn = turn
                        .get("turn")
                        .and_then(|turn| turn.get("id"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                }
                Ok(())
            }
            Protocol::Acp => {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| "wire state lock".to_string())?;
                state.say("user", text);
                state.status = "working";
                drop(state);
                self.settled();
                let id = self.send_request(
                    "session/prompt",
                    serde_json::json!({ "sessionId": thread, "prompt": [{ "type": "text", "text": text }] }),
                    None,
                )?;
                if let Ok(mut state) = self.state.lock() {
                    state.prompt_request = Some(serde_json::Value::from(id));
                }
                Ok(())
            }
            Protocol::ClaudeStream => {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| "wire state lock".to_string())?;
                state.say("user", text);
                state.status = "working";
                state.live.clear();
                drop(state);
                self.settled();
                self.write(&serde_json::json!({ "type": "user", "message": { "role": "user", "content": text } }))
            }
        }
    }

    /// Answer one open question. `message` is the words a refusal carries
    /// where the protocol has a place for them — Claude Code's denial IS a
    /// sentence the model reads, so the plan card's feedback travels there.
    /// Codex and ACP answer with an option alone and drop it.
    pub(crate) fn answer(
        &self,
        ask_id: &serde_json::Value,
        option: Option<&str>,
        answers: &[Vec<String>],
        message: Option<&str>,
    ) -> Result<(), String> {
        let ask = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "wire state lock".to_string())?;
            let at = state
                .asks
                .iter()
                .position(|ask| &ask.id == ask_id)
                .ok_or("that question is no longer open")?;
            let ask = state.asks.remove(at);
            if state.asks.is_empty() && state.status == "asking" {
                state.status = "working";
            }
            ask
        };
        let result = match self.protocol {
            Protocol::AppServer => codex_answer(&ask, option, answers),
            Protocol::Acp => acp_answer(option),
            Protocol::ClaudeStream => claude_answer(&ask, option, answers, message),
        };
        let written = self.reply(&ask.id, result);
        self.settled();
        written
    }

    pub(crate) fn interrupt(&self) -> Result<(), String> {
        let (thread, turn) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "wire state lock".to_string())?;
            (state.thread.clone(), state.turn.clone())
        };
        match self.protocol {
            Protocol::AppServer => {
                let (Some(thread), Some(turn)) = (thread, turn) else {
                    return Ok(());
                };
                self.request(
                    "turn/interrupt",
                    serde_json::json!({ "threadId": thread, "turnId": turn }),
                    REPLY_WAIT,
                )
                .map(|_| ())
            }
            Protocol::Acp => {
                let Some(thread) = thread else {
                    return Ok(());
                };
                self.notify("session/cancel", serde_json::json!({ "sessionId": thread }))
            }
            Protocol::ClaudeStream => self
                .request("interrupt", serde_json::json!({}), REPLY_WAIT)
                .map(|_| ()),
        }
    }

    pub(crate) fn set_model(&self, model: &str) -> Result<(), String> {
        match self.protocol {
            Protocol::AppServer => {
                // The model rides the next `turn/start`; the thread keeps it.
                if let Ok(mut state) = self.state.lock() {
                    state.model = Some(model.to_string());
                }
                Ok(())
            }
            Protocol::Acp => {
                let thread = self
                    .state
                    .lock()
                    .ok()
                    .and_then(|state| state.thread.clone())
                    .ok_or("no session")?;
                self.request(
                    "session/set_model",
                    serde_json::json!({ "sessionId": thread, "modelId": model }),
                    REPLY_WAIT,
                )?;
                if let Ok(mut state) = self.state.lock() {
                    state.model = Some(model.to_string());
                }
                self.settled();
                Ok(())
            }
            Protocol::ClaudeStream => {
                self.request(
                    "set_model",
                    serde_json::json!({ "model": model }),
                    REPLY_WAIT,
                )?;
                if let Ok(mut state) = self.state.lock() {
                    state.model = Some(model.to_string());
                }
                self.settled();
                Ok(())
            }
        }
    }

    pub(crate) fn set_mode(&self, mode: &str) -> Result<(), String> {
        match self.protocol {
            Protocol::AppServer => {
                if let Ok(mut state) = self.state.lock() {
                    state.mode = Some(mode.to_string());
                }
                Ok(())
            }
            Protocol::Acp => {
                let thread = self
                    .state
                    .lock()
                    .ok()
                    .and_then(|state| state.thread.clone())
                    .ok_or("no session")?;
                self.request(
                    "session/set_mode",
                    serde_json::json!({ "sessionId": thread, "modeId": mode }),
                    REPLY_WAIT,
                )?;
                if let Ok(mut state) = self.state.lock() {
                    state.mode = Some(mode.to_string());
                }
                self.settled();
                Ok(())
            }
            Protocol::ClaudeStream => {
                self.request(
                    "set_permission_mode",
                    serde_json::json!({ "mode": mode }),
                    REPLY_WAIT,
                )?;
                if let Ok(mut state) = self.state.lock() {
                    state.mode = Some(mode.to_string());
                }
                self.settled();
                Ok(())
            }
        }
    }

    /// The wire's models: Codex's, asked once and kept; ACP's came with the
    /// session. Claude Code names none on its wire — the page lists the
    /// catalog's for the agent instead.
    pub(crate) fn models(&self) -> Result<Vec<WireModel>, String> {
        let held = self
            .state
            .lock()
            .ok()
            .map(|state| state.models.clone())
            .unwrap_or_default();
        if !held.is_empty() || self.protocol != Protocol::AppServer {
            return Ok(held);
        }
        let listed = self.request("model/list", serde_json::json!({}), REPLY_WAIT)?;
        let models: Vec<WireModel> = listed
            .get("data")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|model| model.get("hidden").and_then(serde_json::Value::as_bool) != Some(true))
            .map(|model| WireModel {
                id: text_of(model.get("id")),
                display_name: text_of(model.get("displayName")),
            })
            .collect();
        if let Ok(mut state) = self.state.lock() {
            state.models.clone_from(&models);
        }
        Ok(models)
    }

    fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for WireSession {
    fn drop(&mut self) {
        self.kill();
    }
}

/// What the page reads: the pane log's shape, with the wire's own state.
#[derive(Serialize)]
pub(crate) struct WireLog {
    pub(crate) found: bool,
    pub(crate) skipped: bool,
    pub(crate) next: u64,
    pub(crate) turns: Vec<TranscriptTurn>,
    pub(crate) status: &'static str,
    pub(crate) asks: Vec<WireAsk>,
    /// The words still being said, for the page's streaming row.
    pub(crate) live: Vec<WireLive>,
    pub(crate) agent: String,
    pub(crate) protocol: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    pub(crate) models: Vec<WireModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mode: Option<String>,
    pub(crate) modes: Vec<WireMode>,
    pub(crate) commands: Vec<WireCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    /// How full the context is, when the session said (the composer's meter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) usage: Option<WireUsage>,
    /// The helpers at work (t-6323 A6).
    pub(crate) tasks: Vec<WireTask>,
}

#[derive(Default)]
pub(crate) struct WireRuntime {
    sessions: Mutex<HashMap<WireId, Arc<WireSession>>>,
    next: AtomicU32,
}

impl WireRuntime {
    pub(crate) fn get(&self, id: WireId) -> Result<Arc<WireSession>, String> {
        self.sessions
            .lock()
            .map_err(|_| "wire sessions lock".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("wire {id} is not open"))
    }

    pub(crate) fn stop(&self, id: WireId) -> Result<(), String> {
        let held = self
            .sessions
            .lock()
            .map_err(|_| "wire sessions lock".to_string())?
            .remove(&id);
        if let Some(session) = held {
            session.kill();
            if let Ok(mut state) = session.state.lock() {
                state.status = "ended";
                state.tasks.clear();
            }
        }
        Ok(())
    }

    /// Start `agent` on its wire in `cwd`, walk the handshake, and hold the
    /// session. `binary` is the agent's executable as this machine found it;
    /// `resume` continues the conversation with that session id where the
    /// road has a flag for it; `env` is the launch's own environment (which
    /// account it runs as — an empty value removes the variable); `changed`
    /// is told the session's id each time the page should look.
    #[allow(clippy::too_many_arguments)] // Each is one independent fact of one launch.
    pub(crate) fn start(
        &self,
        agent: &str,
        binary: &Path,
        cwd: &Path,
        version: &str,
        resume: Option<&str>,
        env: &[(String, String)],
        changed: impl Fn(WireId) + Send + Sync + 'static,
    ) -> Result<Arc<WireSession>, String> {
        let road =
            zerocode_core::agent::wire_road(agent).ok_or_else(|| format!("{agent} has no wire"))?;
        let protocol = Protocol::named(road.protocol)
            .ok_or_else(|| format!("unknown wire {}", road.protocol))?;
        let mut command = crate::proc::quiet_command(binary);
        command
            .args(wire_argv(agent, &road, resume)?)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(path) = crate::shell_path::launch_path() {
            command.env("PATH", path);
        }
        // A wire child is not a pane: the hook environment that names panes
        // must not name it, or its own hooks would report as somebody's pane.
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("ZEROCODE") || key.starts_with("CLAUDE") {
                command.env_remove(&*key);
            }
        }
        for (key, value) in env {
            if value.is_empty() {
                command.env_remove(key);
            } else {
                command.env(key, value);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("{}: {error}", binary.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin on the wire")?;
        let stdout = child.stdout.take().ok_or("no stdout on the wire")?;
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let session = Arc::new(WireSession {
            id,
            agent: agent.to_string(),
            protocol,
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next_request: AtomicI64::new(1),
            state: Mutex::new(WireState {
                status: "starting",
                // The one place an agent fact enters the adapter: the
                // catalog's, read here where the agent is still named.
                plan_tool: zerocode_core::agent::agent_voice(agent).plan_tool,
                ..WireState::default()
            }),
            replies: Mutex::new(HashMap::new()),
            changed: Box::new(changed),
            last_live_notice: Mutex::new(Instant::now()),
        });
        let reader = Arc::clone(&session);
        std::thread::Builder::new()
            .name(format!("wire-{id}"))
            .spawn(move || {
                let mut lines = BufReader::new(stdout).lines();
                while let Some(Ok(line)) = lines.next() {
                    reader.take_line(&line);
                }
                if let Ok(mut state) = reader.state.lock() {
                    state.flush_live();
                    // The helpers ran inside the process that just left.
                    state.tasks.clear();
                    if state.status != "failed" {
                        state.status = "ended";
                    }
                }
                // A request nobody will answer fails now, not at its timeout.
                if let Ok(mut replies) = reader.replies.lock() {
                    for (_, sender) in replies.drain() {
                        let _ = sender.try_send(
                            serde_json::json!({ "error": { "message": "the wire closed" } }),
                        );
                    }
                }
                reader.settled();
            })
            .map_err(|error| error.to_string())?;
        self.sessions
            .lock()
            .map_err(|_| "wire sessions lock".to_string())?
            .insert(id, Arc::clone(&session));
        if let Err(error) = handshake(&session, binary, cwd, version) {
            let _ = self.stop(id);
            return Err(error);
        }
        Ok(session)
    }
}

/// The arguments a wire child starts with: the road's own, then the resume
/// flag and the session when a conversation is continued. A session id is
/// re-validated on the way in — it came off a hook payload and through the
/// webview, and this is the line where it becomes argv — and a road with no
/// resume flag refuses rather than guessing one.
fn wire_argv(
    agent: &str,
    road: &zerocode_core::agent::WireRoad,
    resume: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut argv: Vec<String> = road.args.iter().map(|arg| (*arg).to_string()).collect();
    if let Some(session) = resume {
        let flag = road
            .resume
            .ok_or_else(|| format!("{agent} cannot continue a conversation on its wire"))?;
        if !zerocode_core::provider_session::is_usable_session_id(session) {
            return Err("that is not a session id".to_string());
        }
        argv.push(flag.to_string());
        argv.push(session.to_string());
    }
    Ok(argv)
}

/// The handshake each protocol asks for, blocking until the wire has a
/// thread to talk to.
fn handshake(
    session: &Arc<WireSession>,
    binary: &Path,
    cwd: &Path,
    version: &str,
) -> Result<(), String> {
    let cwd = cwd.to_string_lossy().into_owned();
    match session.protocol {
        Protocol::ClaudeStream => {
            // Claude Code announces itself (`system/init`: version, model,
            // mode, the session's commands) with its first turn, not before:
            // the wire is open the moment the child is. The modes it takes
            // are read off its own `--help`, its version off `--version`,
            // until the announcement says.
            let help = crate::slash_catalog::captured_lines(binary, &["--help"], PROBE_WAIT)
                .unwrap_or_default();
            let modes = help_choices(&help, "--permission-mode");
            let announced =
                crate::slash_catalog::captured_lines(binary, &["--version"], PROBE_WAIT)
                    .and_then(|lines| lines.into_iter().find(|line| !line.trim().is_empty()))
                    .and_then(|line| line.split_whitespace().next().map(str::to_string));
            let mut state = session
                .state
                .lock()
                .map_err(|_| "wire state lock".to_string())?;
            state.modes = modes
                .into_iter()
                .map(|id| WireMode {
                    name: id.clone(),
                    id,
                })
                .collect();
            state.version = announced;
            state.status = "idle";
            Ok(())
        }
        Protocol::AppServer => {
            session.request(
                "initialize",
                serde_json::json!({ "clientInfo": { "name": "zerocode", "title": "ZeroCode", "version": version } }),
                HANDSHAKE_WAIT,
            )?;
            session.notify("initialized", serde_json::json!({}))?;
            let started = session.request(
                "thread/start",
                serde_json::json!({ "cwd": cwd, "approvalPolicy": "on-request", "sandbox": "workspace-write" }),
                HANDSHAKE_WAIT,
            )?;
            let mut state = session
                .state
                .lock()
                .map_err(|_| "wire state lock".to_string())?;
            let thread = started.get("thread").unwrap_or(&serde_json::Value::Null);
            state.thread = thread
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            state.model = started
                .get("model")
                .or_else(|| thread.get("model"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            state.version = thread
                .get("cliVersion")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            state.mode = started
                .get("approvalPolicy")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            state.status = "idle";
            Ok(())
        }
        Protocol::Acp => {
            let opened = session.request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
                    "clientInfo": { "name": "zerocode", "version": version },
                }),
                HANDSHAKE_WAIT,
            )?;
            let methods: Vec<String> = opened
                .get("authMethods")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|method| method.get("id").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect();
            if let Ok(mut state) = session.state.lock() {
                state.version = opened
                    .get("agentInfo")
                    .and_then(|info| info.get("version"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                state.auth_methods.clone_from(&methods);
            }
            let new_session = serde_json::json!({ "cwd": cwd, "mcpServers": [] });
            let result = match session.request("session/new", new_session.clone(), HANDSHAKE_WAIT) {
                Ok(result) => result,
                Err(error) if !methods.is_empty() => {
                    // An unauthenticated agent refuses the session; its first
                    // method is the one the CLI itself would use.
                    session
                        .request(
                            "authenticate",
                            serde_json::json!({ "methodId": methods[0] }),
                            HANDSHAKE_WAIT,
                        )
                        .map_err(|auth| format!("{error}; {auth}"))?;
                    session.request("session/new", new_session, HANDSHAKE_WAIT)?
                }
                Err(error) => return Err(error),
            };
            let mut state = session
                .state
                .lock()
                .map_err(|_| "wire state lock".to_string())?;
            acp_session_opened(&mut state, &result);
            state.status = "idle";
            Ok(())
        }
    }
}

/// The page's read of one session from `after` on.
pub(crate) fn log_of(session: &WireSession, after: u64) -> Result<WireLog, String> {
    let state = session
        .state
        .lock()
        .map_err(|_| "wire state lock".to_string())?;
    let (turns, next, skipped) = state.log_from(after);
    Ok(WireLog {
        found: true,
        skipped,
        next,
        turns,
        status: state.status,
        asks: state.asks.clone(),
        live: state.live.clone(),
        agent: session.agent.clone(),
        protocol: session.protocol.name(),
        model: state.model.clone(),
        models: state.models.clone(),
        mode: state.mode.clone(),
        modes: state.modes.clone(),
        commands: state.commands.clone(),
        version: state.version.clone(),
        usage: state.usage,
        tasks: state.tasks.clone(),
    })
}

/// Where one picture the page asks for stands ([`WireImage`]).
pub(crate) fn image_of(session: &WireSession, at: &str) -> Result<WireImage, String> {
    session
        .state
        .lock()
        .map_err(|_| "wire state lock".to_string())?
        .image(at)
}

/// The session continues this pane's conversation: its history's pictures
/// stand in the pane's transcript (`WireState::history`).
pub(crate) fn remember_history(session: &WireSession, path: std::path::PathBuf) {
    if let Ok(mut state) = session.state.lock() {
        state.history = Some(path);
    }
}

/// The window log's line for a pane handing its conversation to a wire —
/// the receipt (how long the screen took to leave) or the refusal (why it
/// kept it), beside the resume line's vocabulary (`term N …`).
pub(crate) fn hand_over_line(
    term: crate::TermId,
    agent: &str,
    session: Option<&str>,
    wire: WireId,
    outcome: &Result<std::time::Duration, String>,
) -> String {
    let session: String = session
        .unwrap_or_default()
        .chars()
        .take(SESSION_LOG_PREFIX_CHARS)
        .collect();
    match outcome {
        Ok(left_in) => format!(
            "term {term} handed {agent} {session} to wire {wire}: the screen left in {}ms",
            left_in.as_millis()
        ),
        Err(reason) => format!(
            "term {term} kept {agent} {session} on its screen, wire {wire} stopped: {reason}"
        ),
    }
}

/// The window log's line for a pane whose wire never stood — the refusal
/// came before any hand-over (a worker's pane, a CLI without an exit command,
/// a binary not on the PATH, an account door, a handshake that failed). The
/// same vocabulary as [`hand_over_line`]'s refusal, so the log reads as one
/// story: what the pane kept, and why.
pub(crate) fn refusal_line(
    term: crate::TermId,
    agent: &str,
    session: Option<&str>,
    reason: &str,
) -> String {
    let session: String = session
        .unwrap_or_default()
        .chars()
        .take(SESSION_LOG_PREFIX_CHARS)
        .collect();
    format!("term {term} kept {agent} {session} on its screen, no wire started: {reason}")
}

/// How much of a session id the log line carries — enough to tell sessions
/// apart, not the whole uuid on every line.
const SESSION_LOG_PREFIX_CHARS: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(state: &WireState) -> Vec<(String, String)> {
        state
            .turns
            .iter()
            .map(|turn| (turn.turn.role.clone(), turn.turn.text.clone()))
            .collect()
    }

    #[test]
    fn codex_items_become_the_pages_turns_and_the_turn_ends_idle() {
        let mut state = WireState::default();
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"thread/started","params":{"thread":{"id":"t1","model":"gpt-5.6-sol","cliVersion":"0.154.0"}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"turn/started","params":{"threadId":"t1","turn":{"id":"u1"}}}),
        );
        assert_eq!(state.status, "working");
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/started","params":{"item":{"type":"reasoning","id":"r1","summary":[],"content":[]}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/reasoning/textDelta","params":{"itemId":"r1","delta":"Look at "}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/reasoning/textDelta","params":{"itemId":"r1","delta":"the file."}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/completed","params":{"item":{"type":"reasoning","id":"r1","summary":["**Reading** the file"],"content":[]}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/started","params":{"item":{"type":"commandExecution","id":"c1","command":"cat probe.txt","status":"inProgress"}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/commandExecution/outputDelta","params":{"itemId":"c1","delta":"hello\n"}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/completed","params":{"item":{"type":"commandExecution","id":"c1","command":"cat probe.txt","status":"completed","exitCode":0,"aggregatedOutput":"hello\n"}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/started","params":{"item":{"type":"agentMessage","id":"m1","text":""}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"do"}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"ne"}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m1","text":"done"}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"u1","status":"completed","error":null}}}),
        );
        assert_eq!(
            rows(&state),
            vec![
                ("thinking".to_string(), "**Reading** the file".to_string()),
                ("tool".to_string(), "shell · cat probe.txt".to_string()),
                ("tool_result".to_string(), "hello\n".to_string()),
                ("assistant".to_string(), "done".to_string()),
            ]
        );
        let call = state.turns[1].turn.tool.as_ref().expect("a call");
        assert_eq!(
            (
                call.call_id.as_str(),
                call.name.as_str(),
                call.input.as_str()
            ),
            ("c1", "shell", "cat probe.txt")
        );
        let result = state.turns[2].turn.tool.as_ref().expect("a result");
        assert_eq!((result.call_id.as_str(), result.is_error), ("c1", false));
        assert_eq!(state.status, "idle");
        assert_eq!(state.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(state.version.as_deref(), Some("0.154.0"));
        assert!(state.open.is_empty());
    }

    #[test]
    fn a_codex_file_change_carries_its_diff_and_the_approval_asks_about_it() {
        let mut state = WireState::default();
        let diff = "@@ -1,2 +1,2 @@\n hello\n-old\n+new\n";
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/started","params":{"item":{"type":"fileChange","id":"f1","status":"inProgress","changes":[{"path":"src/a.rs","kind":{"type":"update"},"diff":diff}]}}}),
        );
        let call = state.turns[0].turn.tool.as_ref().expect("a call");
        assert_eq!(call.name, "apply_patch");
        assert_eq!(call.edits.len(), 1);
        assert_eq!(call.edits[0].path, "src/a.rs");
        assert_eq!(
            call.edits[0]
                .lines
                .iter()
                .map(|line| line.kind.as_ref())
                .collect::<Vec<_>>(),
            vec!["hunk", "ctx", "del", "add"]
        );
        let out = take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"id":7,"method":"item/fileChange/requestApproval","params":{"itemId":"f1","threadId":"t","turnId":"u","reason":null}}),
        );
        assert!(out.is_empty(), "a question waits on the person");
        assert_eq!(state.status, "asking");
        let ask = &state.asks[0];
        assert_eq!(
            (ask.kind, ask.tool.as_str(), ask.summary.as_deref()),
            ("approval", "apply_patch", Some("src/a.rs"))
        );
        assert_eq!(ask.edits.len(), 1);
        assert_eq!(
            ask.options
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>(),
            vec!["accept", "acceptForSession", "decline"]
        );
        assert_eq!(
            codex_answer(ask, Some("acceptForSession"), &[]),
            serde_json::json!({"decision": "acceptForSession"})
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"item/completed","params":{"item":{"type":"fileChange","id":"f1","status":"completed","changes":[{"path":"src/a.rs","kind":{"type":"update"},"diff":diff}]}}}),
        );
        let result = state.turns[1].turn.tool.as_ref().expect("a result");
        assert!(!result.is_error);
        assert_eq!(state.turns[1].turn.text, "1 · completed");
    }

    #[test]
    fn a_codex_command_approval_and_a_user_input_question_are_asks_with_replies() {
        let mut state = WireState::default();
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"id":3,"method":"item/commandExecution/requestApproval","params":{"itemId":"c","command":"rm -rf build","cwd":"/r","reason":"cleans"}}),
        );
        assert_eq!(state.asks[0].summary.as_deref(), Some("rm -rf build"));
        assert_eq!(
            codex_answer(&state.asks[0], Some("decline"), &[]),
            serde_json::json!({"decision": "decline"})
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"id":4,"method":"item/tool/requestUserInput","params":{"itemId":"q","isBlocking":true,"questions":[{"id":"colour","header":"Colour","question":"Which?","options":[{"label":"red","description":"warm"}],"isOther":true}]}}),
        );
        let ask = &state.asks[1];
        assert_eq!(ask.kind, "question");
        assert_eq!(ask.questions[0].options[0].label, "red");
        assert!(ask.questions[0].other);
        assert_eq!(
            codex_answer(ask, None, &[vec!["red".to_string()]]),
            serde_json::json!({"answers": {"colour": {"answers": ["red"]}}})
        );
        // Permissions carry the requested profile back with the scope chosen.
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"id":5,"method":"item/permissions/requestApproval","params":{"itemId":"p","permissions":{"network":{"enabled":true}},"reason":"fetch docs"}}),
        );
        let ask = &state.asks[2];
        assert_eq!(ask.summary.as_deref(), Some("fetch docs"));
        assert_eq!(
            codex_answer(ask, Some("session"), &[]),
            serde_json::json!({"permissions": {"network": {"enabled": true}}, "scope": "session"})
        );
        assert_eq!(
            codex_answer(ask, Some("decline"), &[]),
            serde_json::json!({"permissions": {}, "scope": "turn"})
        );
        // Requests this side answers itself, and ones it refuses by name.
        let out = take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"id":6,"method":"mcpServer/elicitation/request","params":{"message":"x","mode":"form"}}),
        );
        assert_eq!(
            out,
            vec![Outgoing::Reply {
                id: serde_json::json!(6),
                result: serde_json::json!({"action": "decline"})
            }]
        );
        let out = take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"id":8,"method":"attestation/generate","params":{}}),
        );
        assert!(matches!(&out[0], Outgoing::Refuse { id, .. } if id == &serde_json::json!(8)));
    }

    #[test]
    fn a_codex_error_and_a_failed_turn_are_said_and_fail_the_session() {
        let mut state = WireState::default();
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u"}}}),
        );
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"thread/status/changed","params":{"threadId":"t","status":{"type":"active","activeFlags":["waitingOnApproval"]}}}),
        );
        assert_eq!(state.status, "asking");
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"error","params":{"error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded"},"willRetry":false,"threadId":"t","turnId":"u"}}),
        );
        assert_eq!(
            rows(&state),
            vec![(
                "system".to_string(),
                "You've hit your usage limit.".to_string()
            )]
        );
        assert_eq!(state.status, "failed");
        take(
            Protocol::AppServer,
            &mut state,
            &serde_json::json!({"method":"turn/completed","params":{"threadId":"t","turn":{"id":"u","status":"failed","error":{"message":"You've hit your usage limit."}}}}),
        );
        assert_eq!(state.status, "failed");
        assert!(state.turn.is_none());
    }

    #[test]
    fn acp_chunks_gather_into_turns_around_tool_calls_and_the_prompt_reply_ends_the_turn() {
        let mut state = WireState {
            prompt_request: Some(serde_json::json!(9)),
            status: "working",
            ..WireState::default()
        };
        let update = |update: serde_json::Value| serde_json::json!({"method":"session/update","params":{"sessionId":"s","update":update}});
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"I should read it."}}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Reading "}}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"now."}}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"tool_call","toolCallId":"k1","title":"ReadFile probe.txt","kind":"read","status":"pending","rawInput":{"file_path":"probe.txt"}}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"tool_call_update","toolCallId":"k1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"hello"}}]}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"tool_call","toolCallId":"k2","title":"Edit probe.txt","kind":"edit","status":"completed","content":[{"type":"diff","path":"probe.txt","oldText":"hello","newText":"bye"}]}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"available_commands_update","availableCommands":[{"name":"help","description":"Show help"}]}),
            ),
        );
        take(
            Protocol::Acp,
            &mut state,
            &update(
                serde_json::json!({"sessionUpdate":"current_mode_update","currentModeId":"auto_edit"}),
            ),
        );
        take_response(
            Protocol::Acp,
            &mut state,
            &serde_json::json!({"id":9,"result":{"stopReason":"end_turn"}}),
        );
        assert_eq!(
            rows(&state),
            vec![
                ("thinking".to_string(), "I should read it.".to_string()),
                ("assistant".to_string(), "Reading now.".to_string()),
                ("tool".to_string(), "read · ReadFile probe.txt".to_string()),
                ("tool_result".to_string(), "hello".to_string()),
                ("tool".to_string(), "edit · Edit probe.txt".to_string()),
                ("tool_result".to_string(), String::new()),
                ("assistant".to_string(), "done".to_string()),
            ]
        );
        let edit = state.turns[4].turn.tool.as_ref().expect("a call");
        assert_eq!(edit.edits[0].path, "probe.txt");
        assert_eq!(
            edit.edits[0]
                .lines
                .iter()
                .map(|line| line.kind.as_ref())
                .collect::<Vec<_>>(),
            vec!["del", "add"]
        );
        assert_eq!(
            state.commands,
            vec![WireCommand {
                name: "/help".into(),
                about: "Show help".into()
            }]
        );
        assert_eq!(state.mode.as_deref(), Some("auto_edit"));
        assert_eq!(state.status, "idle");
        assert!(state.prompt_request.is_none());
    }

    /// The context meter's two numbers come off the CLI, and only off it.
    ///
    /// The Claude shapes here are the frames measured on 2026-09-21 (a
    /// `result` line carrying `usage` and `modelUsage` at its top level);
    /// Codex's is its app-server schema for 0.155.1. A frame that names no
    /// window keeps 0 — the chip then says the count instead of drawing a
    /// ring over a denominator nobody gave.
    #[test]
    fn a_session_reports_the_context_it_carries_and_never_invents_the_window() {
        let mut state = WireState::default();
        take(
            Protocol::ClaudeStream,
            &mut state,
            &serde_json::json!({"type":"system","subtype":"init","model":"claude-fable-5-1"}),
        );
        take(
            Protocol::ClaudeStream,
            &mut state,
            &serde_json::json!({"type":"result","subtype":"success","is_error":false,
                "usage":{"input_tokens":2,"cache_creation_input_tokens":17628,"cache_read_input_tokens":9000,"output_tokens":4},
                "modelUsage":{"claude-fable-5-1":{"contextWindow":1_000_000,"maxOutputTokens":64000}}}),
        );
        assert_eq!(
            state.usage,
            Some(WireUsage {
                tokens: 26630,
                window: 1_000_000
            }),
            "the last request's whole prompt, in the model's own window"
        );
        // A local command reaches no model: its `modelUsage` is `{}` and the
        // window stays unknown rather than borrowing the last one.
        let mut local = WireState {
            model: Some("claude-fable-5-1".into()),
            ..WireState::default()
        };
        take(
            Protocol::ClaudeStream,
            &mut local,
            &serde_json::json!({"type":"result","subtype":"success","is_error":false,
                "usage":{"input_tokens":1200,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},
                "modelUsage":{}}),
        );
        assert_eq!(
            local.usage,
            Some(WireUsage {
                tokens: 1200,
                window: 0
            })
        );
        // A result that spent nothing says nothing.
        let mut quiet = WireState::default();
        take(
            Protocol::ClaudeStream,
            &mut quiet,
            &serde_json::json!({"type":"result","subtype":"success","is_error":false,
                "usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},
                "modelUsage":{}}),
        );
        assert!(quiet.usage.is_none());
        // Codex says it on its own notification: `last` is the context, and
        // `total` — the thread's whole spend — is not.
        let mut codex = WireState::default();
        take(
            Protocol::AppServer,
            &mut codex,
            &serde_json::json!({"method":"thread/tokenUsage/updated","params":{"threadId":"t1","turnId":"u1",
                "tokenUsage":{"last":{"inputTokens":34000,"cachedInputTokens":30000,"outputTokens":512,"reasoningOutputTokens":0,"totalTokens":34512},
                    "total":{"inputTokens":900000,"cachedInputTokens":0,"outputTokens":9000,"reasoningOutputTokens":0,"totalTokens":909000},
                    "modelContextWindow":272_000}}}),
        );
        assert_eq!(
            codex.usage,
            Some(WireUsage {
                tokens: 34512,
                window: 272_000
            })
        );
        // A new reading is settled news: the page hears it even when the turn
        // said nothing else.
        let before = codex.mark();
        take(
            Protocol::AppServer,
            &mut codex,
            &serde_json::json!({"method":"thread/tokenUsage/updated","params":{"threadId":"t1","turnId":"u2",
                "tokenUsage":{"last":{"inputTokens":40000,"cachedInputTokens":0,"outputTokens":0,"reasoningOutputTokens":0,"totalTokens":40000},
                    "total":{"inputTokens":0,"cachedInputTokens":0,"outputTokens":0,"reasoningOutputTokens":0,"totalTokens":0},
                    "modelContextWindow":null}}}),
        );
        assert!(!codex.mark().settled_as(&before));
        assert_eq!(
            codex.usage,
            Some(WireUsage {
                tokens: 40000,
                window: 0
            }),
            "a null window is unknown, not the one said a turn ago"
        );
    }

    #[test]
    fn an_acp_permission_request_offers_the_agents_own_options_and_the_answer_names_one() {
        let mut state = WireState::default();
        let out = take(
            Protocol::Acp,
            &mut state,
            &serde_json::json!({"id":"req-1","method":"session/request_permission","params":{"sessionId":"s","toolCall":{"toolCallId":"k3","title":"Shell: rm -rf build","kind":"execute","rawInput":{"command":"rm -rf build"}},"options":[{"optionId":"proceed_once","name":"Allow once","kind":"allow_once"},{"optionId":"proceed_always","name":"Always allow","kind":"allow_always"},{"optionId":"cancel","name":"No","kind":"reject_once"}]}}),
        );
        assert!(out.is_empty());
        let ask = &state.asks[0];
        assert_eq!(
            (ask.kind, ask.tool.as_str(), ask.summary.as_deref()),
            ("approval", "execute", Some("Shell: rm -rf build"))
        );
        assert_eq!(
            ask.options[1],
            WireOption {
                id: "proceed_always".into(),
                kind: "allow_always".into(),
                label: Some("Always allow".into())
            }
        );
        assert_eq!(
            acp_answer(Some("proceed_once")),
            serde_json::json!({"outcome": {"outcome": "selected", "optionId": "proceed_once"}})
        );
        assert_eq!(
            acp_answer(None),
            serde_json::json!({"outcome": {"outcome": "cancelled"}})
        );
        let out = take(
            Protocol::Acp,
            &mut state,
            &serde_json::json!({"id":"req-2","method":"fs/read_text_file","params":{"path":"/x"}}),
        );
        assert!(matches!(&out[0], Outgoing::Refuse { .. }));
    }

    #[test]
    fn an_acp_session_hands_over_its_models_and_modes() {
        let mut state = WireState::default();
        acp_session_opened(
            &mut state,
            &serde_json::json!({"sessionId":"s1","models":{"currentModelId":"gemini-3.8-flash","availableModels":[{"modelId":"gemini-3.8-flash","name":"Gemini 3.8 Flash"},{"modelId":"gemini-3.8-pro","name":"Gemini 3.8 Pro"}]},"modes":{"currentModeId":"default","availableModes":[{"id":"default","name":"Default"},{"id":"auto_edit","name":"Auto edit"},{"id":"yolo","name":"YOLO"}]}}),
        );
        assert_eq!(state.thread.as_deref(), Some("s1"));
        assert_eq!(state.model.as_deref(), Some("gemini-3.8-flash"));
        assert_eq!(state.models.len(), 2);
        assert_eq!(state.models[1].display_name, "Gemini 3.8 Pro");
        assert_eq!(
            state
                .modes
                .iter()
                .map(|mode| mode.id.as_str())
                .collect::<Vec<_>>(),
            vec!["default", "auto_edit", "yolo"]
        );
    }

    #[test]
    fn the_log_reads_on_from_a_seq_and_says_when_the_front_was_dropped() {
        let mut state = WireState::default();
        for n in 0..(TURN_CAP + 5) {
            state.say("assistant", format!("turn {n}"));
        }
        assert_eq!(state.turns.len(), TURN_CAP);
        let (turns, next, skipped) = state.log_from(0);
        assert!(skipped, "the front was dropped");
        assert_eq!(turns.len(), TURN_CAP);
        assert_eq!(next, (TURN_CAP + 5) as u64);
        let (later, next_again, skipped) = state.log_from(next - 2);
        assert!(!skipped);
        assert_eq!(later.len(), 2);
        assert_eq!(next_again, next);
        let (none, _, _) = state.log_from(next);
        assert!(none.is_empty());
    }

    fn claude(state: &mut WireState, message: serde_json::Value) -> Vec<Outgoing> {
        take(Protocol::ClaudeStream, state, &message)
    }

    /// A tool's screenshot comes down the stream once: the session keeps it
    /// under its own key (`wire:<n>`) for the page to fetch, the turn carries
    /// only that key, and the oldest go past the cap (t-6323 A8).
    #[test]
    fn a_tools_image_is_kept_by_the_session_under_its_own_key() {
        let mut state = WireState::default();
        let shot = |n: usize| serde_json::json!({"type":"user","message":{"role":"user","content":[{"tool_use_id":format!("t{n}"),"type":"tool_result","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":format!("{n}").repeat(300)}}]}]},"session_id":"s1"});
        claude(&mut state, shot(1));
        let turn = &state.turns.last().expect("a turn").turn;
        assert_eq!(turn.role, "tool_result");
        assert_eq!(turn.images[0].at, "wire:1");
        assert!(
            serde_json::to_string(turn).unwrap().len() < 400,
            "the payload stays off the turn"
        );
        assert_eq!(
            state
                .images
                .back()
                .map(|(key, payload)| (*key, payload.len())),
            Some((1, 300))
        );
        for n in 2..=WIRE_IMAGE_CAP + 1 {
            claude(&mut state, shot(n % 10));
        }
        assert_eq!(state.images.len(), WIRE_IMAGE_CAP);
        assert!(
            state.images.iter().all(|(key, _)| *key != 1),
            "the oldest went past the cap"
        );
        // A key the session kept hands its payload back; one pushed out, or
        // a file place in a session that continues no pane, hands nothing.
        let newest = format!("wire:{}", state.image_seq);
        assert!(matches!(state.image(&newest), Ok(WireImage::Held(_))));
        assert!(state.image("wire:1").is_err());
        assert!(state.image("120:44").is_err());
        // A session that continues a pane finds its history's pictures in
        // that pane's transcript.
        state.history = Some(std::path::PathBuf::from("/tmp/pane.jsonl"));
        assert_eq!(
            state.image("120:44"),
            Ok(WireImage::InFile(std::path::PathBuf::from(
                "/tmp/pane.jsonl"
            )))
        );
    }

    /// Claude Code's helper frames, in the shapes a live `-p` stream sent
    /// them (2.1.280, 2026-09-23): a local agent is kept from its
    /// `task_started` — under the Agent call that spawned it — through its
    /// `task_progress` (tokens, tools, and its latest step: the frame's own
    /// words where they differ from the description, else the tool it last
    /// used; a summary once it gives one) to its end, which `task_updated`
    /// or `task_notification` says. A background shell is no helper, and a
    /// roll call that no longer lists a background helper ends it.
    #[test]
    fn claude_keeps_a_helper_from_its_start_to_its_end_under_the_call_that_spawned_it() {
        let mut state = WireState::default();
        let started = |task: &str, call: &str, background: bool| serde_json::json!({"type":"system","subtype":"task_started","task_id":task,"tool_use_id":call,"description":"count md files","subagent_type":"general-purpose","is_backgrounded":background,"task_type":"local_agent","session_id":"s1"});
        claude(&mut state, started("a1", "toolu_1", true));
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_2","description":"npm test","task_type":"local_bash","session_id":"s1"}),
        );
        assert_eq!(state.tasks.len(), 1, "a background shell is no helper");
        let task = &state.tasks[0];
        assert_eq!(
            (task.task.as_str(), task.call.as_deref()),
            ("a1", Some("toolu_1"))
        );
        assert_eq!(task.description, "count md files");
        assert!(task.started_ms > 0);
        let started_ms = task.started_ms;
        let moved = state.mark();
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"task_progress","task_id":"a1","tool_use_id":"toolu_1","description":"Running Count .md files","usage":{"total_tokens":12842,"tool_uses":1,"duration_ms":5522},"last_tool_name":"Bash","summary":null,"session_id":"s1"}),
        );
        let task = &state.tasks[0];
        assert_eq!((task.tokens, task.tools), (12842, 1));
        assert_eq!(task.step.as_deref(), Some("Running Count .md files"));
        assert_eq!(task.started_ms, started_ms, "progress keeps the clock");
        assert!(
            !state.mark().settled_as(&moved),
            "a helper's progress is news"
        );
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"task_progress","task_id":"a1","description":"count md files","usage":{"total_tokens":13000,"tool_uses":2,"duration_ms":6000},"last_tool_name":"Glob","session_id":"s1"}),
        );
        assert_eq!(state.tasks[0].step.as_deref(), Some("Glob"));
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"task_progress","task_id":"a1","description":"count md files","usage":{"total_tokens":13100,"tool_uses":2,"duration_ms":6100},"last_tool_name":"Glob","summary":"Counting the files","session_id":"s1"}),
        );
        assert_eq!(
            state.tasks[0].summary.as_deref(),
            Some("Counting the files")
        );
        let log = serde_json::to_value(&state.tasks).unwrap();
        assert_eq!(log[0]["call"], "toolu_1");
        assert!(
            log[0].get("backgrounded").is_none(),
            "provenance stays off the wire"
        );
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"task_updated","task_id":"a1","patch":{"status":"completed","end_time":1},"session_id":"s1"}),
        );
        assert!(state.tasks.is_empty(), "an end in the patch ends it");
        claude(&mut state, started("a2", "toolu_3", false));
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"task_notification","task_id":"a2","tool_use_id":"toolu_3","status":"completed","summary":"**2**","session_id":"s1"}),
        );
        assert!(state.tasks.is_empty(), "the notification ends it");
        claude(&mut state, started("a3", "toolu_4", true));
        claude(&mut state, started("a4", "toolu_5", false));
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"background_tasks_changed","tasks":[],"session_id":"s1"}),
        );
        let left: Vec<&str> = state.tasks.iter().map(|task| task.task.as_str()).collect();
        assert_eq!(
            left,
            ["a4"],
            "an unlisted background helper ends; a foreground one stays"
        );
    }

    /// Claude Code's lines (2.1.272, read off this machine 2026-09-16): the
    /// announcement fills the head, the deltas stream as live text, the
    /// closing `assistant`/`user` lines become the transcript's turns and
    /// clear the live text, a subagent's lines are not this conversation,
    /// and `result` ends the turn.
    #[test]
    fn claude_lines_stream_live_text_and_close_into_turns_through_the_transcript_reader() {
        let mut state = WireState::default();
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"init","cwd":"/w","session_id":"s1","tools":["Bash"],"model":"claude-fable-5-1","permissionMode":"default","slash_commands":["help","commit"],"claude_code_version":"2.1.272","uuid":"u0"}),
        );
        assert_eq!(state.thread.as_deref(), Some("s1"));
        assert_eq!(state.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(state.mode.as_deref(), Some("default"));
        assert_eq!(state.version.as_deref(), Some("2.1.272"));
        assert_eq!(
            state
                .commands
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["/help", "/commit"]
        );
        claude(
            &mut state,
            serde_json::json!({"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-fable-5-1","id":"m1","type":"message","role":"assistant","content":[]}},"session_id":"s1","parent_tool_use_id":null,"uuid":"u1"}),
        );
        assert_eq!(state.status, "working");
        claude(
            &mut state,
            serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Look."}},"session_id":"s1","parent_tool_use_id":null}),
        );
        claude(
            &mut state,
            serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"do"}},"session_id":"s1","parent_tool_use_id":null}),
        );
        claude(
            &mut state,
            serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"ne"}},"session_id":"s1","parent_tool_use_id":null}),
        );
        claude(
            &mut state,
            serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"co"}},"session_id":"s1","parent_tool_use_id":null}),
        );
        assert_eq!(
            state.live,
            vec![
                WireLive {
                    role: "thinking",
                    text: "Look.".into(),
                    done: false,
                },
                WireLive {
                    role: "assistant",
                    text: "done".into(),
                    done: false,
                },
            ]
        );
        assert!(rows(&state).is_empty(), "nothing closed yet");
        // A subagent's line (Task) is not this conversation.
        claude(
            &mut state,
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"inner"}]},"parent_tool_use_id":"toolu_9","session_id":"s1"}),
        );
        assert!(rows(&state).is_empty());
        claude(
            &mut state,
            serde_json::json!({"type":"assistant","message":{"model":"claude-fable-5-1","id":"m1","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"Look.","signature":"x"},{"type":"text","text":"done"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"printf hello > probe.txt","description":"Write hello"}}]},"parent_tool_use_id":null,"session_id":"s1","uuid":"u2","timestamp":"2026-09-15T19:46:01.586Z"}),
        );
        assert!(state.live.is_empty(), "the close clears the live text");
        assert_eq!(
            rows(&state),
            vec![
                ("thinking".to_string(), "Look.".to_string()),
                ("assistant".to_string(), "done".to_string()),
                (
                    "tool".to_string(),
                    "Bash · printf hello > probe.txt".to_string()
                ),
            ]
        );
        let call = state.turns[2].turn.tool.as_ref().expect("a call");
        assert_eq!(
            (call.call_id.as_str(), call.name.as_str()),
            ("toolu_1", "Bash")
        );
        assert_eq!(state.turns[2].turn.at_ms, Some(1_789_501_561_586));
        claude(
            &mut state,
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"(Bash completed with no output)","is_error":false}]},"parent_tool_use_id":null,"session_id":"s1","uuid":"u3","timestamp":"2026-09-15T19:46:01.758Z","tool_use_result":{"stdout":"","stderr":"","interrupted":false}}),
        );
        let result = state.turns[3].turn.tool.as_ref().expect("a result");
        assert_eq!(state.turns[3].turn.role, "tool_result");
        assert_eq!(result.call_id, "toolu_1");
        assert!(!result.is_error);
        claude(
            &mut state,
            serde_json::json!({"duration_api_ms":15542,"stop_reason":"end_turn","session_id":"s1","total_cost_usd":0.76,"type":"result","subtype":"success","is_error":false,"duration_ms":16000,"num_turns":2,"result":"done"}),
        );
        assert_eq!(state.status, "idle");
        assert_eq!(
            rows(&state).len(),
            4,
            "the result's text is the answer already said"
        );
        claude(
            &mut state,
            serde_json::json!({"type":"system","subtype":"status","status":null,"permissionMode":"acceptEdits","session_id":"s1"}),
        );
        assert_eq!(state.mode.as_deref(), Some("acceptEdits"));
        claude(
            &mut state,
            serde_json::json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":"Rate limited","session_id":"s1"}),
        );
        assert_eq!(state.status, "failed");
        assert_eq!(
            rows(&state).last().cloned(),
            Some(("system".to_string(), "Rate limited".to_string()))
        );
    }

    /// `control_request can_use_tool` is the card's question, worded by the
    /// pane view's own reader; the answers are `control_response`s in the
    /// CLI's shape — an allow repeats the input, an allow for the session
    /// carries the CLI's own rule suggestions, a denial is a sentence.
    #[test]
    fn a_claude_permission_question_is_the_cards_ask_and_its_answers_are_control_responses() {
        let mut state = WireState::default();
        state.stream("assistant", "Running it.");
        let suggestions = serde_json::json!([{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"printf hello *"}],"behavior":"allow","destination":"localSettings"}]);
        let input =
            serde_json::json!({"command":"printf hello > probe.txt","description":"Write hello"});
        let out = claude(
            &mut state,
            serde_json::json!({"type":"control_request","request_id":"cd58","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":input,"description":"Write hello","permission_suggestions":suggestions,"blocked_path":"/w/probe.txt","tool_use_id":"toolu_1"}}),
        );
        assert!(out.is_empty());
        assert_eq!(state.status, "asking");
        assert_eq!(
            rows(&state),
            vec![("assistant".to_string(), "Running it.".to_string())],
            "the live words close before the question stands"
        );
        let ask = state.asks[0].clone();
        assert_eq!(
            (ask.kind, ask.tool.as_str(), ask.summary.as_deref()),
            ("approval", "Bash", Some("printf hello > probe.txt"))
        );
        assert_eq!(
            ask.options
                .iter()
                .map(|option| (option.id.as_str(), option.kind.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("allow", "allow_once"),
                ("allow_always", "allow_always"),
                ("deny", "reject_once")
            ]
        );
        assert_eq!(
            claude_answer(&ask, Some("allow"), &[], None),
            serde_json::json!({"behavior":"allow","updatedInput":input})
        );
        assert_eq!(
            claude_answer(&ask, Some("allow_always"), &[], None),
            serde_json::json!({"behavior":"allow","updatedInput":input,"updatedPermissions":suggestions})
        );
        assert_eq!(
            claude_answer(&ask, Some("deny"), &[], None)["behavior"],
            "deny"
        );
        assert_eq!(claude_answer(&ask, None, &[], None)["behavior"], "deny");
        assert_eq!(
            reply_line(
                Protocol::ClaudeStream,
                &ask.id,
                serde_json::json!({"behavior":"allow"})
            ),
            serde_json::json!({"type":"control_response","response":{"subtype":"success","request_id":"cd58","response":{"behavior":"allow"}}})
        );
        // An edit's question carries its diff; no suggestions, no "always".
        let mut state = WireState::default();
        claude(
            &mut state,
            serde_json::json!({"type":"control_request","request_id":"e1","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{"file_path":"/w/a.rs","old_string":"fn a() {}","new_string":"fn b() {}"},"permission_suggestions":[]}}),
        );
        let ask = &state.asks[0];
        assert_eq!(ask.tool, "Edit");
        assert_eq!(ask.edits.len(), 1);
        assert_eq!(ask.edits[0].path, "/w/a.rs");
        assert_eq!(ask.options.len(), 2);
        // A control request this window has no answer for is refused in the
        // CLI's error shape.
        let out = claude(
            &mut state,
            serde_json::json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"c"}}),
        );
        assert_eq!(out.len(), 1);
        let Outgoing::Refuse { id, message } = &out[0] else {
            panic!("a refusal");
        };
        assert_eq!(id, &serde_json::json!("h1"));
        assert!(message.contains("hook_callback"));
        assert_eq!(
            refuse_line(Protocol::ClaudeStream, id, message),
            serde_json::json!({"type":"control_response","response":{"subtype":"error","request_id":"h1","error":message}})
        );
    }

    /// The plan permission (`ExitPlanMode`, the extension's "Claude's Plan"):
    /// the plan rides the ask so the card can preview it, and the refusal
    /// carries the person's own words — which is what Claude Code shows the
    /// model as its reason. The plan is the CATALOG's fact: a session whose
    /// agent names no plan tool reads the same request as an ordinary
    /// approval.
    #[test]
    fn a_plan_permission_carries_its_plan_and_the_refusal_carries_the_words() {
        let plan = "## Plan\n1. read\n2. write";
        let request = serde_json::json!({
            "type": "control_request",
            "request_id": "p1",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "ExitPlanMode",
                "input": { "plan": plan },
            },
        });
        let mut state = WireState {
            plan_tool: zerocode_core::agent::agent_voice("claude").plan_tool,
            ..WireState::default()
        };
        assert_eq!(state.plan_tool, Some("ExitPlanMode"));
        claude(&mut state, request.clone());
        let ask = state.asks[0].clone();
        assert_eq!((ask.kind, ask.tool.as_str()), ("approval", "ExitPlanMode"));
        assert_eq!(ask.plan.as_deref(), Some(plan));
        // The refusal is the plan's feedback road: given words, they travel;
        // given none, the window's own sentence still stands.
        assert_eq!(
            claude_answer(&ask, Some("deny"), &[], Some("drop step 2")),
            serde_json::json!({ "behavior": "deny", "message": "drop step 2" })
        );
        assert_eq!(
            claude_answer(&ask, Some("deny"), &[], Some("   "))["message"],
            "The person declined this tool call in the ZeroCode window.",
            "blank feedback is no feedback"
        );
        assert_eq!(
            claude_answer(&ask, Some("allow"), &[], Some("ignored"))["behavior"],
            "allow",
            "the words ride a refusal, not an approval"
        );
        // An agent the catalog gives no plan tool asks an ordinary approval.
        let mut silent = WireState::default();
        claude(&mut silent, request);
        assert_eq!(silent.asks[0].plan, None);
    }

    /// `AskUserQuestion` comes through permission too: its questions stand as
    /// the question card, and the answers ride the tool's input keyed by the
    /// question's own text with the option's label, as the CLI's dialog
    /// answers it (`updatedInput.answers`).
    #[test]
    fn a_claude_ask_user_question_is_a_question_ask_answered_on_the_input() {
        let mut state = WireState::default();
        let input = serde_json::json!({"questions":[{"question":"Which one?","header":"Pick","options":[{"label":"A","description":"a"},{"label":"B","description":"b"}],"multiSelect":false}]});
        claude(
            &mut state,
            serde_json::json!({"type":"control_request","request_id":"q1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":input}}),
        );
        let ask = state.asks[0].clone();
        assert_eq!(
            (ask.kind, ask.tool.as_str()),
            ("question", "AskUserQuestion")
        );
        assert_eq!(ask.questions.len(), 1);
        assert_eq!(
            (
                ask.questions[0].id.as_str(),
                ask.questions[0].header.as_str(),
                ask.questions[0].options.len(),
                ask.questions[0].other
            ),
            ("Which one?", "Pick", 2, true)
        );
        let mut answered = input.clone();
        answered["answers"] = serde_json::json!({"Which one?":"A"});
        assert_eq!(
            claude_answer(&ask, None, &[vec!["A".to_string()]], None),
            serde_json::json!({"behavior":"allow","updatedInput":answered})
        );
        assert_eq!(claude_answer(&ask, None, &[], None)["behavior"], "deny");
    }

    /// Our own requests and their answers wear each protocol's shape: JSON-RPC
    /// for Codex and ACP, `control_request`/`control_response` with the id as
    /// a string for Claude Code — and the answer comes back in one shape.
    #[test]
    fn our_requests_and_their_answers_wear_each_protocols_shape() {
        assert_eq!(
            request_line(
                Protocol::ClaudeStream,
                3,
                "set_permission_mode",
                serde_json::json!({"mode":"acceptEdits"})
            ),
            serde_json::json!({"type":"control_request","request_id":"3","request":{"subtype":"set_permission_mode","mode":"acceptEdits"}})
        );
        assert_eq!(
            request_line(
                Protocol::AppServer,
                3,
                "turn/start",
                serde_json::json!({"a":1})
            ),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"turn/start","params":{"a":1}})
        );
        assert_eq!(
            response_of(
                Protocol::ClaudeStream,
                &serde_json::json!({"type":"control_response","response":{"subtype":"success","request_id":"3","response":{"mode":"acceptEdits"}}})
            ),
            Some((3, serde_json::json!({"result":{"mode":"acceptEdits"}})))
        );
        assert_eq!(
            response_of(
                Protocol::ClaudeStream,
                &serde_json::json!({"type":"control_response","response":{"subtype":"error","request_id":"4","error":"nope"}})
            ),
            Some((4, serde_json::json!({"error":{"message":"nope"}})))
        );
        assert_eq!(
            response_of(
                Protocol::ClaudeStream,
                &serde_json::json!({"type":"assistant","message":{}})
            ),
            None
        );
        let answer = serde_json::json!({"id":9,"result":{"stopReason":"end_turn"}});
        assert_eq!(
            response_of(Protocol::Acp, &answer),
            Some((9, answer.clone()))
        );
        assert_eq!(
            response_of(
                Protocol::Acp,
                &serde_json::json!({"id":9,"method":"session/request_permission"})
            ),
            None
        );
        for (name, protocol) in PROTOCOL_NAMES {
            assert_eq!(Protocol::named(name), Some(protocol));
            assert_eq!(protocol.name(), name);
        }
        assert!(!Protocol::ClaudeStream.json_rpc() && Protocol::Acp.json_rpc());
    }

    /// Codex's deltas are the live text until their item closes; a close
    /// with no text of its own takes the streamed words; the turn's end
    /// clears whatever was left.
    #[test]
    fn codex_deltas_are_live_until_their_item_closes() {
        let mut state = WireState::default();
        let codex = |state: &mut WireState, message: serde_json::Value| {
            take(Protocol::AppServer, state, &message)
        };
        codex(
            &mut state,
            serde_json::json!({"method":"item/started","params":{"item":{"type":"agentMessage","id":"m1","text":""}}}),
        );
        codex(
            &mut state,
            serde_json::json!({"method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"Hel"}}),
        );
        codex(
            &mut state,
            serde_json::json!({"method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"lo"}}),
        );
        codex(
            &mut state,
            serde_json::json!({"method":"item/reasoning/textDelta","params":{"itemId":"r1","delta":"hm"}}),
        );
        assert_eq!(
            state.live,
            vec![
                WireLive {
                    role: "assistant",
                    text: "Hello".into(),
                    done: false,
                },
                WireLive {
                    role: "thinking",
                    text: "hm".into(),
                    done: false,
                }
            ]
        );
        codex(
            &mut state,
            serde_json::json!({"method":"item/completed","params":{"item":{"type":"reasoning","id":"r1","summary":[],"content":[]}}}),
        );
        assert_eq!(
            rows(&state),
            vec![("thinking".to_string(), "hm".to_string())]
        );
        assert_eq!(state.live.len(), 1);
        codex(
            &mut state,
            serde_json::json!({"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m1","text":""}}}),
        );
        assert_eq!(
            rows(&state)[1],
            ("assistant".to_string(), "Hello".to_string())
        );
        assert!(state.live.is_empty());
        codex(
            &mut state,
            serde_json::json!({"method":"item/agentMessage/delta","params":{"itemId":"m2","delta":"left over"}}),
        );
        codex(
            &mut state,
            serde_json::json!({"method":"turn/completed","params":{"turn":{"id":"u1","status":"completed"}}}),
        );
        assert!(state.live.is_empty());
        assert_eq!(state.status, "idle");
    }

    /// The mark the reader compares: turns, status, questions and the head's
    /// words settle it; deltas only move its live part.
    #[test]
    fn a_mark_settles_on_turns_status_and_questions_and_moves_live_on_deltas() {
        let mut state = WireState::default();
        let quiet = state.mark();
        state.stream("assistant", "x");
        let streaming = state.mark();
        assert!(quiet.settled_as(&streaming));
        assert_ne!(quiet.live, streaming.live);
        state.stream("assistant", "y");
        assert_ne!(streaming.live, state.mark().live);
        state.flush_live();
        let closed = state.mark();
        assert!(!streaming.settled_as(&closed));
        state.status = "asking";
        assert!(!closed.settled_as(&state.mark()));
        let asking = state.mark();
        state.mode = Some("acceptEdits".into());
        assert!(!asking.settled_as(&state.mark()));
    }

    /// A continued conversation rides the road's own resume flag with a
    /// validated id; a road without one refuses, as does a bad id.
    #[test]
    fn a_wire_continues_a_conversation_by_its_roads_own_flag() {
        let claude = zerocode_core::agent::wire_road("claude").expect("claude wire");
        let fresh = wire_argv("claude", &claude, None).expect("fresh");
        assert_eq!(
            fresh,
            claude
                .args
                .iter()
                .map(|a| (*a).to_string())
                .collect::<Vec<_>>()
        );
        let continued = wire_argv(
            "claude",
            &claude,
            Some("8504a820-e857-4b23-86b0-eddb49cafcd2"),
        )
        .expect("resume");
        assert_eq!(
            &continued[continued.len() - 2..],
            &[
                "--resume".to_string(),
                "8504a820-e857-4b23-86b0-eddb49cafcd2".to_string()
            ]
        );
        assert!(wire_argv("claude", &claude, Some("--continue")).is_err());
        let codex = zerocode_core::agent::wire_road("codex").expect("codex wire");
        assert!(wire_argv("codex", &codex, Some("thread-1")).is_err());
        assert_eq!(
            wire_argv("codex", &codex, None).expect("fresh"),
            vec!["app-server".to_string()]
        );
    }

    /// The modes Claude Code takes are read off its own `--help`, where clap
    /// wraps the choice list over lines.
    #[test]
    fn help_choices_reads_a_wrapped_clap_choice_list() {
        let lines: Vec<String> = [
            "  --permission-mode <mode>          Permission mode to use for the session",
            "                                    (choices: \"acceptEdits\", \"auto\",",
            "                                    \"bypassPermissions\", \"manual\",",
            "                                    \"dontAsk\", \"plan\")",
            "  --permission-prompts <target>     Who answers permission prompts with",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(
            help_choices(&lines, "--permission-mode"),
            vec![
                "acceptEdits",
                "auto",
                "bypassPermissions",
                "manual",
                "dontAsk",
                "plan"
            ]
        );
        assert!(help_choices(&lines, "--permission-prompts").is_empty());
        assert!(help_choices(&lines, "--model").is_empty());
    }

    /// The hand-over's log line is a receipt with the leaving's duration or a
    /// refusal with its reason, in the resume line's vocabulary.
    #[test]
    fn a_hand_over_logs_its_receipt_or_its_refusal() {
        let took = Ok(std::time::Duration::from_millis(340));
        assert_eq!(
            hand_over_line(2, "claude", Some("b7695d1e-1846-43f6"), 7, &took),
            "term 2 handed claude b7695d1e to wire 7: the screen left in 340ms"
        );
        let kept = Err("the pane's CLI kept its screen 8s after `/exit`".to_string());
        assert_eq!(
            hand_over_line(2, "claude", None, 7, &kept),
            "term 2 kept claude  on its screen, wire 7 stopped: the pane's CLI kept its screen 8s after `/exit`"
        );
    }

    /// A wire refused before it stood leaves the same kind of line: until
    /// 2026-09-21 these refusals reached only a toast, and a window log with
    /// twenty-two Claude panes and no hand-over line could not say why the
    /// conversation never streamed.
    #[test]
    fn a_wire_refused_before_it_stands_logs_the_refusal_too() {
        assert_eq!(
            refusal_line(
                4,
                "claude",
                Some("b7695d1e-1846-43f6"),
                "a worker's pane stays a pane"
            ),
            "term 4 kept claude b7695d1e on its screen, no wire started: a worker's pane stays a pane"
        );
    }
}
