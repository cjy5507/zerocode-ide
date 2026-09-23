//! Reading what a conversation last said, out of the files the vendors write.
//!
//! Two consumers, one reader. The vault reads a transcript's HEAD to preview a
//! past session; the board reads a live transcript's TAIL to put the agent's
//! last words on its card. Both are the same job — JSONL lines whose `content`
//! is a string for one vendor and an array of typed parts for another — so the
//! extraction lives here once, and the two surfaces cannot learn to read the
//! same file differently.
//!
//! The board's half copies Orca's measured flow (out/main/index.js:9842-9859):
//! a `Stop` hook carries `last_assistant_message` directly, or names a
//! `transcript_path` whose tail holds the answer, or carries neither — and
//! that last case CLEARS the card's line rather than leaving the previous
//! turn's answer under a new turn's prompt. The prompt itself arrives on
//! `UserPromptSubmit` as a payload field (`prompt`) and needs no file at all.
//!
//! Everything here is read-only and bounded, for the vault's reason: these
//! are the vendors' files, records of somebody's work, and a reader that
//! could be made to slurp a 2 GB file into memory is a reader that takes the
//! window down with it.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How long a title or preview line is kept.
pub const SUMMARY_CHARS: usize = 240;

/// How much of a transcript's tail is searched for the last assistant turn.
///
/// Orca scans backwards in 64 KiB chunks up to 4 MiB (out/main/index.js:9092).
/// One bounded read of the last 256 KiB finds the same answer in practice —
/// an assistant turn further back than that is buried under a quarter
/// megabyte of newer lines, and the honest answer for a tail that holds no
/// assistant text is nothing, not a deeper dig.
pub const MAX_TAIL_BYTES: u64 = 256 * 1024;

/// The label zo puts on reasoning it carries into a later turn as a text
/// block — its "reasoning passport" (`runtime::convert_messages::
/// REASONING_PASSPORT_LABEL` in zo-ide; spelled once more here because this
/// crate cannot depend on zo). The label is the model's to read, never the
/// person's: a text block that starts with it is reasoning, and the page shows
/// it as such (2026-09-08: the agent tab printed `[earlier reasoning] **…**`
/// as prose).
pub const REASONING_PASSPORT_LABEL: &str = "[earlier reasoning]";

/// The reasoning a part carries, if it is reasoning at all: a `thinking`
/// block's thought (Claude Code and zo both write one), or a text block that
/// wears the passport label (the label taken off).
fn reasoning_text(part: &serde_json::Value) -> Option<String> {
    let kind = part.get("type").and_then(serde_json::Value::as_str)?;
    let text = match kind {
        "thinking" => part.get("thinking").and_then(serde_json::Value::as_str)?,
        "text" => {
            let text = part.get("text").and_then(serde_json::Value::as_str)?;
            text.trim_start().strip_prefix(REASONING_PASSPORT_LABEL)?
        }
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The words out of a `content` value, whatever shape it came in.
///
/// A string for one vendor, an array of typed parts for another, and inside the
/// array either `text` or `content`. Nothing here guesses at a tool result — a
/// part with no text contributes nothing rather than a `[object Object]`.
pub fn message_text(content: Option<&serde_json::Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    let parts = content.as_array()?;
    let mut out = String::new();
    for part in parts {
        let text = part
            .get("text")
            .and_then(|v| v.as_str())
            .or_else(|| part.get("content").and_then(|v| v.as_str()));
        if let Some(text) = text {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(text.trim());
        }
    }
    let trimmed = out.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// One line, whitespace collapsed, cut on a word break.
///
/// The same rule a task title uses ([`crate::task::WorktreeTask`]) because a
/// session and a dispatched task are the same thing to a reader — see
/// [`crate::session`] for why that matters.
pub fn clamp(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= SUMMARY_CHARS {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(SUMMARY_CHARS).collect();
    let at = cut.rfind(' ').unwrap_or(cut.len());
    format!("{}…", cut[..at].trim_end())
}

/// The assistant's words out of one transcript line, or nothing.
///
/// The shapes are the ones the vault's parsers already read from real files:
/// claude writes `{type:"assistant", message:{role, content:[{text}]}}`, and
/// the flatter vendors put `role`/`content` at the top. A line that is not an
/// assistant turn — a user turn, a tool result, a summary record — answers
/// nothing, so the backwards scan keeps walking.
fn assistant_line_text(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let record = value.as_object()?;
    let message = record.get("message").and_then(|v| v.as_object());
    let role = record
        .get("role")
        .and_then(|v| v.as_str())
        .or_else(|| message?.get("role")?.as_str())
        .or_else(|| {
            (record.get("type").and_then(|v| v.as_str()) == Some("assistant"))
                .then_some("assistant")
        });
    if role != Some("assistant") {
        return None;
    }
    let content = message
        .and_then(|m| m.get("content"))
        .or_else(|| record.get("content"));
    message_text(content)
}

/// The bounded tail of the file at `path`, as whole lines.
///
/// One read serves every question asked of a transcript's END — the last
/// assistant turn for a card, whether a Codex rollout stops inside a turn for
/// a wake — so the two cannot learn to read the same file differently. When
/// the read starts mid-file its first line is somebody's half — dropped, not
/// parsed: half a JSON line is not a JSON line, and the half that decodes by
/// luck would be read as a record nobody wrote.
struct Tail {
    text: String,
    /// Whether the file went on above the window: this is a tail, not the
    /// whole file.
    cut: bool,
}

impl Tail {
    fn read(path: &Path) -> Option<Self> {
        let mut file = std::fs::File::open(path).ok()?;
        let size = file.metadata().ok()?.len();
        if size == 0 {
            return None;
        }
        let start = size.saturating_sub(MAX_TAIL_BYTES);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_TAIL_BYTES).read_to_end(&mut bytes).ok()?;
        Some(Self {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            cut: start > 0,
        })
    }

    /// The whole lines in the window, oldest first.
    fn lines(&self) -> impl DoubleEndedIterator<Item = &str> {
        let mut lines = self.text.lines();
        if self.cut {
            lines.next();
        }
        lines
    }
}

/// The whole lines in the bounded tail of the file at `path`, oldest first.
///
/// The same `Tail` every other question about a transcript's END is asked
/// of, handed out as lines for a reader in another crate — the quota-wall
/// marker table, which reads a Codex rollout's last turn edge and a Claude
/// transcript's last record. `None` is a file that cannot be read or is
/// empty, never an empty tail of a file that exists.
pub fn tail_lines(path: &Path) -> Option<Vec<String>> {
    let tail = Tail::read(path)?;
    Some(tail.lines().map(str::to_string).collect())
}

/// The last assistant turn in the transcript at `path`, clamped for a card.
///
/// A bounded read of the file's tail, scanned backwards line by line.
pub fn last_assistant_message(path: &Path) -> Option<String> {
    let tail = Tail::read(path)?;
    tail.lines()
        .rev()
        .find_map(|line| assistant_line_text(line.trim()))
        .map(|said| clamp(&said))
}

/// Codex's own words for the two edges of a turn, as its rollout records them:
/// `{type:"event_msg", payload:{type:…}}`. Measured against
/// `codex-runtime-home/home/sessions/**/rollout-*.jsonl` (243 files, every one
/// carrying at least one edge; an Esc lands as `turn_aborted` with
/// `reason:"interrupted"`), and the same three names sit in the codex binary's
/// string table.
const CODEX_TURN_OPENED: &str = "task_started";
const CODEX_TURN_CLOSED: [&str; 2] = ["task_complete", "turn_aborted"];

/// Whether the Codex rollout at `path` stops inside a turn.
///
/// The hook is a poor witness for a Codex pane's turn: a pane that went
/// `idle` (this window's own "agent left" observation) keeps that word while
/// the rollout goes on recording tool calls, and Codex's `Stop` hook fires
/// 5 ms–3 s BEFORE `task_complete` reaches the file. The rollout is the one
/// witness that outlives the process, so a wake asks it: `Some(true)` when the
/// newest turn edge in the tail is `task_started`, `Some(false)` when it is
/// `task_complete` or `turn_aborted`, `None` when there is no file to ask.
///
/// A window that holds no edge at all is decided by whether it is the whole
/// file. A finished turn closes within a few KiB of its last words (≤18 KiB
/// after `task_complete`, across those 243 rollouts), so a full window of
/// items with no edge is the middle of a turn longer than the window — while a
/// whole file with no edge is a session in which no turn ever began.
pub fn codex_turn_open(path: &Path) -> Option<bool> {
    let tail = Tail::read(path)?;
    for line in tail.lines().rev() {
        match codex_turn_edge(line.trim()) {
            Some(TurnEdge::Opened) => return Some(true),
            Some(TurnEdge::Closed) => return Some(false),
            None => {}
        }
    }
    Some(tail.cut)
}

enum TurnEdge {
    Opened,
    Closed,
}

/// The turn edge one rollout line records, if it records one.
///
/// Decided on the parsed record's `type` fields, never on the bytes: a tool
/// output that happens to quote `"task_complete"` is a string inside an item,
/// not an edge. The substring test before the parse only spares the parse for
/// the items and token counts that make up most of a rollout.
fn codex_turn_edge(line: &str) -> Option<TurnEdge> {
    if !line.contains("event_msg") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
        return None;
    }
    let kind = value.get("payload")?.get("type")?.as_str()?;
    if kind == CODEX_TURN_OPENED {
        Some(TurnEdge::Opened)
    } else if CODEX_TURN_CLOSED.contains(&kind) {
        Some(TurnEdge::Closed)
    } else {
        None
    }
}

/// The envelopes a CLI wraps around a turn that nobody typed.
///
/// A harness resuming an agent, a scheduled wake-up landing, a slash command
/// expanding — each of those arrives at the hook as a `prompt`, because from
/// the CLI's side it IS this turn's input. It is not what a person said, and a
/// row that names an agent after `<task-notification>` has told the reader
/// nothing except that this window shows its own plumbing.
///
/// A closed list rather than "anything in angle brackets": somebody pasting
/// `<div>` into a prompt meant to paste `<div>`.
const MACHINE_ENVELOPES: [&str; 9] = [
    "task-notification",
    "system-reminder",
    "local-command-caveat",
    "command-name",
    "command-message",
    "command-args",
    "command-contents",
    // What a local slash command printed, replayed into the next turn. The
    // session-title road already skips it (`claude_session_title`); a row that
    // names an agent after it would be reading the same machinery aloud.
    "local-command-stdout",
    "user-prompt-submit-hook",
];

/// Drops the leading machine envelopes and returns what a person actually
/// typed after them, which is often nothing.
fn past_the_envelopes(prompt: &str) -> &str {
    let mut rest = prompt.trim_start();
    // Bounded rather than `loop`: a payload is outside this program's control,
    // and the real ones carry two or three of these at most.
    for _ in 0..8 {
        let Some(tag) = MACHINE_ENVELOPES
            .iter()
            .find(|tag| rest.starts_with(&format!("<{tag}>")))
        else {
            return rest;
        };
        let close = format!("</{tag}>");
        // An unclosed envelope swallows the rest of the text — there is no
        // person's sentence to find after a tag that never ends.
        let Some(at) = rest.find(&close) else {
            return "";
        };
        rest = rest[at + close.len()..].trim_start();
    }
    rest
}

/// The prompt a `UserPromptSubmit` hook payload carries, clamped for a card.
///
/// Machine envelopes are dropped first (see `MACHINE_ENVELOPES`), so a card
/// shows the sentence a person typed under the scaffolding rather than the
/// scaffolding — and shows nothing at all when the whole turn was machinery.
pub fn prompt_in_payload(payload: &str) -> Option<String> {
    prompt_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`prompt_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn prompt_in_parsed(payload: &crate::payload::HookPayload<'_>) -> Option<String> {
    let value = payload.tree()?;
    let prompt = past_the_envelopes(value.get("prompt")?.as_str()?).trim();
    (!prompt.is_empty()).then(|| clamp(prompt))
}

/// What the agent said as its turn ended, from a `Stop`-family payload.
///
/// Orca's own three answers in order (out/main/index.js:9843-9859): the
/// payload's explicit field, the tail of the transcript the payload names,
/// or nothing — and nothing means the caller CLEARS its line, because a card
/// showing the previous turn's answer under this turn's prompt is a card
/// lying about which question was answered.
pub fn said_in_payload(payload: &str) -> Option<String> {
    said_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`said_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn said_in_parsed(payload: &crate::payload::HookPayload<'_>) -> Option<String> {
    let value = payload.tree()?;
    for key in ["last_assistant_message", "lastAssistantMessage", "message"] {
        if let Some(direct) = value.get(key).and_then(|v| v.as_str())
            && !direct.trim().is_empty()
        {
            return Some(clamp(direct));
        }
    }
    let path = value
        .get("transcript_path")
        .or_else(|| value.get("transcriptPath"))?
        .as_str()?;
    last_assistant_message(Path::new(path))
}

/// What a tool answered mid-turn, from a `PostToolUse`-family payload.
///
/// Orca updates the card's answer line with each tool result as it lands
/// (`extractClaudeToolFields`, out/main/index.js:9541 — `PostToolUse` writes
/// `lastAssistantMessage` from the response, `PostToolUseFailure` falls back
/// through `error` and `message`), so a person supervising five agents reads
/// what each one just did without waiting for the turn to end. The response
/// shapes are `extractToolResponseText`'s (:9067): the string itself, an
/// object's `text_result_for_llm`/`textResultForLlm`/`text`, or the first
/// `content[]` part with non-blank text.
pub fn tool_said_in_payload(payload: &str) -> Option<String> {
    tool_said_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`tool_said_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn tool_said_in_parsed(payload: &crate::payload::HookPayload<'_>) -> Option<String> {
    let said = tool_response_text(payload.tree()?.get("tool_response")?)?;
    Some(clamp(&said))
}

/// The failure spelling: the response's own text when it carries one, else
/// the payload's `error`, else its `message` (out/main/index.js:9548).
pub fn tool_failure_in_payload(payload: &str) -> Option<String> {
    tool_failure_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`tool_failure_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn tool_failure_in_parsed(payload: &crate::payload::HookPayload<'_>) -> Option<String> {
    let value = payload.tree()?;
    let said = value
        .get("tool_response")
        .and_then(tool_response_text)
        .or_else(|| {
            ["error", "message"].iter().find_map(|key| {
                value
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
            })
        })?;
    Some(clamp(&said))
}

/// `extractToolResponseText` (:9067), shape for shape.
fn tool_response_text(response: &serde_json::Value) -> Option<String> {
    if let Some(text) = response.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }
    let record = response.as_object()?;
    for key in ["text_result_for_llm", "textResultForLlm", "text"] {
        if let Some(direct) = record.get(key).and_then(|v| v.as_str())
            && !direct.is_empty()
        {
            return Some(direct.to_string());
        }
    }
    record
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|part| part.get("text").and_then(|v| v.as_str()))
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

/// What the agent stopped to ask, out of a `NeedsAttention` payload.
///
/// Three shapes, in the order they identify themselves (the payload's own
/// fields, not the event name — a `PermissionRequest` and a `Notification`
/// both land here and each carries a different half):
///
/// 1. An `AskUserQuestion`/`RequestUserInput` tool call — the question text
///    itself, out of `tool_input.questions[].question`. Orca detects the
///    same two tool names, normalised the same way (`isAskUserQuestionTool`,
///    out/main/index.js:8177).
/// 2. Any other tool asking for permission — the tool's name and the one
///    input string that identifies what it wants (`command`, `file_path`,
///    `path`, `url` or `pattern` — Orca's `summarizeApprovalInput` fields,
///    :9011).
/// 3. A notification — its `message`.
///
/// One measured divergence, on purpose: Orca's card shows the RAW JSON of
/// its `interactivePrompt` (`{"approval":{"tool":…}}`, drawn verbatim at
/// I18nProvider-4EBrmTGg.js:96446). The JSON shape is protocol for its
/// interactive answer UI, not copy — a person supervising five agents reads
/// `Bash · rm -rf build` faster than an escaped object, so what leaves here
/// is the sentence, not the wire format.
pub fn ask_in_payload(payload: &str) -> Option<String> {
    ask_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`ask_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn ask_in_parsed(payload: &crate::payload::HookPayload<'_>) -> Option<String> {
    let value = payload.tree()?;
    let tool = value
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let input = value.get("tool_input");
    let normalized: String = tool
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized == "askuserquestion" || normalized == "requestuserinput" {
        let questions = input
            .and_then(|v| v.get("questions"))
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|q| q.get("question").and_then(|v| v.as_str()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        if !questions.trim().is_empty() {
            return Some(clamp(&questions));
        }
    }
    if !tool.is_empty() {
        let telling = ["command", "file_path", "path", "url", "pattern"]
            .iter()
            .find_map(|key| input.and_then(|v| v.get(key)).and_then(|v| v.as_str()))
            .unwrap_or("");
        return Some(clamp(&if telling.is_empty() {
            tool.to_string()
        } else {
            format!("{tool} · {telling}")
        }));
    }
    let message = value.get("message").and_then(|v| v.as_str())?.trim();
    (!message.is_empty()).then(|| clamp(message))
}

/// One turn of a transcript, as a reader meets it.
///
/// `role` is the vendor's own word for who spoke — `user`, `assistant`, or
/// `tool` for a call the assistant made. The WORDS around it belong to the
/// window: a card already says 나 and the agent's name in the language in
/// force, and a second vocabulary down here would be that catalog's rival.
///
/// `at_ms` is the moment the line was written, when the vendor stamped one
/// (see `moment_of`) — what lets a page say how long a run of tool calls
/// took rather than when it happened to read them. Absent, the page keeps
/// its own clock; this file never guesses a moment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TranscriptTurn {
    pub role: String,
    pub text: String,
    pub at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<TranscriptTool>,
    /// The images the turn carries — a person's pasted screenshot, what a
    /// tool handed back — as references ([`TranscriptImage`]); the page
    /// asks for each one when it comes into view.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<TranscriptImage>,
}

/// An image a turn carries, as a reference: its kind and where its base64
/// stands (`"<offset>:<len>"` bytes into the file the turn was read from, as
/// [`elide_payloads`] left it), never the payload itself — a screenshot is
/// half a megabyte, and a page shows a dozen of them as pills (t-6323 A8).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TranscriptImage {
    pub media_type: String,
    pub at: String,
}

/// The images among a message's parts: `image` blocks whose base64 was set
/// aside ([`PAYLOAD_AT_KEY`]). An image still inline in the line — one no
/// reader set aside, or too small to — has no place to be fetched from and
/// is left out.
fn images_in(parts: Option<&serde_json::Value>) -> Vec<TranscriptImage> {
    parts
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("image"))
        .filter_map(|part| {
            let source = part.get("source")?;
            let at = source.get(PAYLOAD_AT_KEY)?.as_str()?;
            Some(TranscriptImage {
                media_type: source
                    .get("media_type")
                    .and_then(serde_json::Value::as_str)
                    .filter(|kind| kind.starts_with("image/"))?
                    .to_string(),
                at: at.to_string(),
            })
        })
        .collect()
}

/// The vendor's call identity and bounded details, used to join a result to
/// its command even when several tools run between two assistant messages.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TranscriptTool {
    pub call_id: String,
    pub name: String,
    pub input: String,
    pub is_error: bool,
    /// The edits the call describes, as rows to draw under it — the
    /// extension's inline diff. Empty for a call that edits nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<TranscriptEdit>,
    /// The file the call read or wrote, and where in it ([`file_in`]) —
    /// what the page's tool row opens. `None` for a call that names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<TranscriptFile>,
}

/// The file a tool call read or wrote, and where in it — what the Claude
/// Code extension's tool header links (`fileToolHeader`, 2.1.280): a read
/// opens where it began, an edit where its new text stands, a write at the
/// file's top.
///
/// Carried as the call said it. `offset` and `limit` are the CLI's own count
/// and are not converted here: Claude Code's `offset` IS the first line it
/// returns (measured 2026-09-23 — `offset: 10930` came back opening
/// `10930→`), zo's `read_file` counts from 0 (its schema). Which one a page
/// holds is the agent's catalog fact (`AgentVoice::read_offset_base`); this
/// reader cannot know it and does not guess.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TranscriptFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// The start of the text an edit wrote, to find its place by (the
    /// extension hands its host the new string as `searchText`) — at most
    /// [`FILE_SEARCH_CHARS`], which places it as surely as the whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
}

/// How much of an edit's new text the page searches its file for.
pub const FILE_SEARCH_CHARS: usize = 400;

/// The file a tool call names, when the call is a read, an edit or a write
/// (`hook::Tool::named` — the tool's reduced name, never its vendor): a
/// search names a directory to look in and a command a line to run, and
/// neither is a file to open. Field names are the whole test, as for
/// [`edits_in`]: `file_path` (Claude Code, Gemini CLI), `notebook_path`, and
/// zo's `path`, camelCase accepted.
#[must_use]
pub fn file_in(name: &str, input: Option<&serde_json::Value>) -> Option<TranscriptFile> {
    use crate::hook::Tool;
    let tool = Tool::named(name)?;
    if !matches!(tool, Tool::Read | Tool::Edit | Tool::Write) {
        return None;
    }
    let map = tool_input(input)?;
    let text_at = |map: &serde_json::Map<String, serde_json::Value>, keys: &[&str]| {
        keys.iter()
            .find_map(|key| map.get(*key)?.as_str())
            .map(str::to_string)
    };
    let path = text_at(
        &map,
        &[
            "file_path",
            "filePath",
            "notebook_path",
            "notebookPath",
            "path",
        ],
    )
    .filter(|path| !path.trim().is_empty())?;
    let reads = matches!(tool, Tool::Read);
    let number = |key: &str| reads.then(|| map.get(key)?.as_u64()).flatten();
    let search = matches!(tool, Tool::Edit)
        .then(|| {
            text_at(&map, &["new_string", "newString"]).or_else(|| {
                let first = map.get("edits")?.as_array()?.first()?.as_object()?;
                text_at(first, &["new_string", "newString"])
            })
        })
        .flatten()
        .filter(|text| !text.trim().is_empty())
        .map(|text| text.chars().take(FILE_SEARCH_CHARS).collect());
    Some(TranscriptFile {
        path,
        offset: number("offset"),
        limit: number("limit"),
        search,
    })
}

/// One file's change inside a tool call, drawn as an inline diff under the
/// call's row (the Claude Code extension's block beneath `● Write`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptEdit {
    pub path: String,
    pub lines: Vec<crate::compact_diff::DiffLine>,
    /// Rows past [`EDIT_DIFF_LINES`], left out; the page says how many.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub truncated: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// The most rows one edit is drawn with. A 2,000-line `Write` is a file, not
/// a turn; the page shows this much and says how much more there was.
pub const EDIT_DIFF_LINES: usize = 400;

impl TranscriptEdit {
    pub fn at(path: &str, lines: Vec<crate::compact_diff::DiffLine>) -> Self {
        let truncated = lines.len().saturating_sub(EDIT_DIFF_LINES);
        let mut lines = lines;
        lines.truncate(EDIT_DIFF_LINES);
        Self {
            path: path.to_string(),
            lines,
            truncated,
        }
    }

    /// A whole file written: every line added, numbered from its top.
    fn written(path: &str, content: &str) -> Self {
        let hunks = crate::compact_diff::compact_line_diff("", content);
        Self::at(path, crate::compact_diff::hunk_lines(&hunks, true))
    }

    /// A snippet replaced: the diff of the two strings, its place in the
    /// file unknown, so the gutters stay blank.
    pub fn replaced(path: &str, old: &str, new: &str) -> Option<Self> {
        let hunks = crate::compact_diff::compact_line_diff(old, new);
        (!hunks.is_empty()).then(|| Self::at(path, crate::compact_diff::hunk_lines(&hunks, false)))
    }
}

/// The edits a tool call describes, read from the shapes the vendors write:
///
/// - Claude Code — `Edit {file_path, old_string, new_string}`, `MultiEdit
///   {file_path, edits: [{old_string, new_string}]}`, `Write {file_path,
///   content}`, `NotebookEdit {notebook_path, new_source}`;
/// - zo — `edit_file`/`write_file` with `path` (camelCase accepted);
/// - Gemini CLI — `replace {file_path, old_string, new_string}`, `write_file`;
/// - Codex — `apply_patch`: its own tool whose input IS the patch, `shell
///   ["apply_patch", patch]`, or an `exec` script calling
///   `tools.apply_patch("…")` (the patch a JS string literal inside it).
///
/// Which vendor wrote the call is not asked — the field names and the
/// tool's reduced name (`hook::Tool::named`) are the whole test, so a fifth
/// CLI writing any of these shapes is read too. A call that edits nothing
/// (a read, a search, a plain command) answers empty.
#[must_use]
pub fn edits_in(name: &str, input: Option<&serde_json::Value>) -> Vec<TranscriptEdit> {
    let Some(input) = input else {
        return Vec::new();
    };
    let tool = crate::hook::Tool::named(name);
    if let Some(map) = tool_input(Some(input)) {
        return edits_of_object(tool.as_ref(), &map);
    }
    let serde_json::Value::String(text) = input else {
        return Vec::new();
    };
    match tool {
        Some(crate::hook::Tool::Edit) => patch_edits(text),
        Some(crate::hook::Tool::Bash) => script_patch_edits(text),
        _ => Vec::new(),
    }
}

fn edits_of_object(
    tool: Option<&crate::hook::Tool>,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Vec<TranscriptEdit> {
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| map.get(*key).and_then(serde_json::Value::as_str))
    };
    let path = text(&["file_path", "path", "notebook_path", "filePath"]).unwrap_or_default();
    match tool {
        Some(crate::hook::Tool::Edit) => {
            if let Some(patch) = text(&["input", "patch"])
                && patch.contains("*** Begin Patch")
            {
                return patch_edits(patch);
            }
            if let Some(edits) = map.get("edits").and_then(serde_json::Value::as_array) {
                return edits
                    .iter()
                    .filter_map(serde_json::Value::as_object)
                    .filter_map(|edit| replaced_in(path, edit))
                    .collect();
            }
            if let Some(edit) = replaced_in(path, map) {
                return vec![edit];
            }
            text(&["new_source"])
                .map(|content| vec![TranscriptEdit::written(path, content)])
                .unwrap_or_default()
        }
        Some(crate::hook::Tool::Write) => text(&["content", "new_source", "text"])
            .map(|content| vec![TranscriptEdit::written(path, content)])
            .unwrap_or_default(),
        Some(crate::hook::Tool::Bash) => match map.get("command") {
            Some(serde_json::Value::Array(argv))
                if argv.first().and_then(serde_json::Value::as_str) == Some("apply_patch") =>
            {
                argv.get(1)
                    .and_then(serde_json::Value::as_str)
                    .map(patch_edits)
                    .unwrap_or_default()
            }
            Some(serde_json::Value::String(command)) => script_patch_edits(command),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn replaced_in(
    path: &str,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<TranscriptEdit> {
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| map.get(*key).and_then(serde_json::Value::as_str))
    };
    let old = text(&["old_string", "oldString", "old_str", "old_text"]);
    let new = text(&["new_string", "newString", "new_str", "new_text"]);
    if old.is_none() && new.is_none() {
        return None;
    }
    TranscriptEdit::replaced(path, old.unwrap_or_default(), new.unwrap_or_default())
}

/// Codex's patch format, one edit per file:
///
/// ```text
/// *** Begin Patch
/// *** Update File: path      (hunks: `@@ heading`, ` ctx`, `-old`, `+new`)
/// *** Move to: new path
/// *** Add File: path         (`+` lines, numbered from 1)
/// *** Delete File: path
/// *** End Patch
/// ```
#[must_use]
pub fn patch_edits(patch: &str) -> Vec<TranscriptEdit> {
    use crate::compact_diff::DiffLine;
    let mut edits: Vec<TranscriptEdit> = Vec::new();
    let mut current: Option<(String, Vec<DiffLine>)> = None;
    let mut added_at: Option<u32> = None;
    let close = |current: &mut Option<(String, Vec<DiffLine>)>, edits: &mut Vec<TranscriptEdit>| {
        if let Some((path, lines)) = current.take() {
            edits.push(TranscriptEdit::at(&path, lines));
        }
    };
    for raw in patch.lines() {
        if let Some(path) = raw.strip_prefix("*** Update File: ") {
            close(&mut current, &mut edits);
            current = Some((path.trim().to_string(), Vec::new()));
            added_at = None;
            continue;
        }
        if let Some(path) = raw.strip_prefix("*** Add File: ") {
            close(&mut current, &mut edits);
            current = Some((path.trim().to_string(), Vec::new()));
            added_at = Some(1);
            continue;
        }
        if let Some(path) = raw.strip_prefix("*** Delete File: ") {
            close(&mut current, &mut edits);
            edits.push(TranscriptEdit::at(
                path.trim(),
                vec![DiffLine::marker("meta", raw)],
            ));
            continue;
        }
        let Some((_, lines)) = current.as_mut() else {
            continue;
        };
        if raw.starts_with("*** Move to: ") {
            lines.push(DiffLine::marker("meta", raw));
            continue;
        }
        if raw.starts_with("@@") {
            lines.push(DiffLine::marker("hunk", raw));
            continue;
        }
        if raw.starts_with("*** ") {
            // `*** End Patch`, `*** End of File`.
            continue;
        }
        let mut body = raw.chars();
        match body.next() {
            Some('+') => {
                let new = added_at;
                if let Some(at) = added_at.as_mut() {
                    *at += 1;
                }
                lines.push(DiffLine {
                    kind: "add".into(),
                    text: body.as_str().to_string(),
                    old: None,
                    new,
                });
            }
            Some('-') => lines.push(DiffLine {
                kind: "del".into(),
                text: body.as_str().to_string(),
                old: None,
                new: None,
            }),
            Some(' ') => lines.push(DiffLine {
                kind: "ctx".into(),
                text: body.as_str().to_string(),
                old: None,
                new: None,
            }),
            _ => lines.push(DiffLine {
                kind: "ctx".into(),
                text: raw.to_string(),
                old: None,
                new: None,
            }),
        }
    }
    close(&mut current, &mut edits);
    edits
}

/// The patches a script applies — every `apply_patch(` whose argument is a
/// string literal carrying a patch. Codex's `exec` tool writes
/// `text(await tools.apply_patch("*** Begin Patch\n…"))`.
fn script_patch_edits(script: &str) -> Vec<TranscriptEdit> {
    let mut edits = Vec::new();
    let mut from = 0;
    while let Some(found) = script[from..].find("apply_patch(") {
        let open = from + found + "apply_patch(".len();
        from = open;
        let rest = &script[open..];
        let quote_at = open + rest.len() - rest.trim_start().len();
        if let Some(literal) = js_string_literal(script, quote_at)
            && literal.contains("*** Begin Patch")
        {
            edits.extend(patch_edits(&literal));
        }
    }
    edits
}

/// The JS string literal opening at `at` (`"…"`, `'…'` or a template),
/// unescaped, or `None` when nothing opens there.
fn js_string_literal(text: &str, at: usize) -> Option<String> {
    let mut chars = text.get(at..)?.chars();
    let quote = chars.next()?;
    if !matches!(quote, '"' | '\'' | '`') {
        return None;
    }
    let mut out = String::new();
    while let Some(character) = chars.next() {
        if character == quote {
            return Some(out);
        }
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            '\n' => {}
            'x' => {
                let code: String = chars.by_ref().take(2).collect();
                out.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
            }
            'u' => {
                let code: String = if text.get(at..)?.is_empty() {
                    String::new()
                } else {
                    let mut probe = chars.clone();
                    if probe.next() == Some('{') {
                        chars.next();
                        chars.by_ref().take_while(|c| *c != '}').collect()
                    } else {
                        chars.by_ref().take(4).collect()
                    }
                };
                out.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
            }
            other => out.push(other),
        }
    }
    None
}

/// Complete JSONL records inside a byte-limited read. Offsets are bytes, not
/// the length of a lossy UTF-8 conversion. An oversized line is skipped in
/// bounded steps so it cannot block every subsequent conversation turn.
pub struct TranscriptChunk<'a> {
    pub bytes: &'a [u8],
    /// Where `bytes` begins in the read — past a half line the read opened
    /// inside.
    pub start: usize,
    pub consumed: usize,
    pub skipped: bool,
}

#[must_use]
pub fn complete_transcript_chunk(
    bytes: &[u8],
    starts_mid_line: bool,
    full: bool,
) -> TranscriptChunk<'_> {
    let end = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    let start = if starts_mid_line {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(end, |at| at + 1)
    } else {
        0
    };
    let oversized = end == 0 && full;
    TranscriptChunk {
        bytes: &bytes[start..end],
        start,
        consumed: if oversized { bytes.len() } else { end },
        skipped: starts_mid_line || oversized,
    }
}

/* ---- inline payloads (t-6323 A8) ---- */

/// The inline payloads a transcript line may carry — a pasted image's or a
/// tool's screenshot's base64, which Claude Code writes twice on a tool's
/// line (`message.content[].source.data` and `toolUseResult.file.base64`) —
/// are what makes such a line too long to read: of the 146 image lines in
/// this machine's last 40 transcripts, 135 were past the 256 KB read and
/// were dropped whole, words and all (measured 2026-09-23, t-6323 A8).
/// Set aside, each payload leaves behind where it stands in the file, and
/// the page asks for it when the image comes into view.
const PAYLOAD_KEYS: [&[u8]; 2] = [b"\"data\":\"", b"\"base64\":\""];

/// A base64 run shorter than this stays where it is: it weighs nothing, and
/// a short run under a `data` key may be a value a person reads (a digest, a
/// key id) — the smallest picture is well past it.
pub const PAYLOAD_ELIDE_MIN: usize = 256;

/// The key an elided payload leaves behind: `"<offset>:<len>"`, bytes into
/// the file the line was read from.
pub const PAYLOAD_AT_KEY: &str = "zerocode_at";

fn is_base64(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
}

/// `bytes` with every inline payload of at least [`PAYLOAD_ELIDE_MIN`]
/// base64 bytes set aside: `"data":"<base64>"` becomes `"data":"",
/// "zerocode_at":"<offset>:<len>"`, the offset counted from the start of the
/// file (`base` is where `bytes` begins there). A payload is a JSON string
/// of base64 alone under a `data` or `base64` key, so the text around it
/// stays valid JSON; a quote inside a string is escaped (`\"`) and never
/// looks like a key. Borrowed when there is nothing to set aside.
#[must_use]
pub fn elide_payloads(bytes: &[u8], base: u64) -> std::borrow::Cow<'_, [u8]> {
    let mut out: Option<Vec<u8>> = None;
    let mut kept = 0;
    let mut at = 0;
    while at < bytes.len() {
        let Some((key, hit)) = PAYLOAD_KEYS
            .iter()
            .filter_map(|key| find(&bytes[at..], key).map(|hit| (key, at + hit)))
            .min_by_key(|(_, hit)| *hit)
        else {
            break;
        };
        let start = hit + key.len();
        let end = start
            + bytes[start..]
                .iter()
                .take_while(|byte| is_base64(**byte))
                .count();
        if end < bytes.len() && bytes[end] == b'"' && end - start >= PAYLOAD_ELIDE_MIN {
            let out = out.get_or_insert_with(|| Vec::with_capacity(bytes.len() / 4));
            out.extend_from_slice(&bytes[kept..start]);
            let offset = base + start as u64;
            out.extend_from_slice(
                format!("\",\"{PAYLOAD_AT_KEY}\":\"{offset}:{}", end - start).as_bytes(),
            );
            kept = end;
            at = end + 1;
        } else {
            at = start;
        }
    }
    match out {
        Some(mut out) => {
            out.extend_from_slice(&bytes[kept..]);
            std::borrow::Cow::Owned(out)
        }
        None => std::borrow::Cow::Borrowed(bytes),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Whether `bytes` is a payload as [`elide_payloads`] sets one aside:
/// base64 and nothing else — what a place handed back to the window must
/// hold, so the place cannot be aimed at anything else a file keeps.
#[must_use]
pub fn is_payload(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|byte| is_base64(*byte))
}

/// A transcript read is bounded by bytes; individual expanded tool cells also
/// need a bound so one large result does not monopolise the conversation.
const TOOL_DETAIL_CHARS: usize = 16 * 1024;

fn tool_detail(text: &str) -> String {
    let mut chars = text.chars();
    let mut kept: String = chars.by_ref().take(TOOL_DETAIL_CHARS).collect();
    if chars.next().is_some() {
        kept.push('…');
    }
    kept
}

fn transcript_tool(part: &serde_json::Value, result: bool) -> TranscriptTool {
    let name = part
        .get(if result { "tool_name" } else { "name" })
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let input = if result {
        String::new()
    } else {
        tool_input(part.get("input").or_else(|| part.get("arguments")))
            .and_then(|input| {
                input
                    .get("command")
                    .or_else(|| input.get("cmd"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| serde_json::to_string_pretty(&input).ok())
            })
            .or_else(|| {
                part.get("input")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    };
    TranscriptTool {
        call_id: part
            .get("call_id")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                part.get(if result { "tool_use_id" } else { "id" })
                    .and_then(serde_json::Value::as_str)
            })
            .unwrap_or_default()
            .to_string(),
        name: name.to_string(),
        input: tool_detail(&input),
        edits: if result {
            Vec::new()
        } else {
            edits_in(name, part.get("input").or_else(|| part.get("arguments")))
        },
        file: if result {
            None
        } else {
            file_in(name, part.get("input").or_else(|| part.get("arguments")))
        },
        is_error: part
            .get("is_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }
}

/// The moment a transcript line was written, in epoch milliseconds.
///
/// Claude Code stamps every line `timestamp` (ISO 8601, with a zone); zo's
/// session store stamps a message record `updated_at_ms` (epoch ms). Both are
/// read here, and nothing else is: a stamp without a zone is refused by the
/// calendar crate rather than resolved in whatever zone this machine is in.
fn moment_of(row: &serde_json::Value) -> Option<i64> {
    if let Some(stamp) = row.get("timestamp").and_then(serde_json::Value::as_str) {
        return crate::civil::epoch_ms_of_iso(stamp);
    }
    row.get("updated_at_ms")
        .and_then(serde_json::Value::as_u64)
        .and_then(|ms| i64::try_from(ms).ok())
}

/// What one tool call is called, and the one argument worth showing.
///
/// The verb alone ("Read") says nothing about which read; the whole input is
/// a JSON object nobody wants in a conversation. The argument shown is the
/// one a terminal transcript shows ([`tool_target`]).
fn tool_line(part: &serde_json::Value) -> Option<String> {
    let name = part.get("name").and_then(serde_json::Value::as_str)?;
    let said = tool_input(part.get("input").or_else(|| part.get("arguments")))
        .and_then(|input| tool_target(name, &input));
    Some(match said {
        Some(said) => format!("{name} · {}", clamp(&said)),
        None => name.to_string(),
    })
}

/// The call's arguments as an object. Claude Code writes them as one; zo's
/// session store writes the JSON as a STRING — read either, and refuse
/// anything that is not an object. Read as an object only, every zo helper's
/// page showed the verb alone ("읽기", "찾기") with nothing it read or sought.
fn tool_input(
    input: Option<&serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    match input? {
        serde_json::Value::Object(map) => Some(map.clone()),
        serde_json::Value::String(text) => match serde_json::from_str(text).ok()? {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        },
        _ => None,
    }
}

/// The keys a tool names its subject under, most telling first, for a tool
/// that is neither a read nor a search.
const TARGET_KEYS: [&str; 9] = [
    "file_path",
    "path",
    "command",
    "cmd",
    "pattern",
    "query",
    "url",
    "description",
    "prompt",
];

/// The one argument worth showing, the way codex's and Claude Code's own
/// transcripts show it: a read names its file and, when it read a window of
/// it, the lines (`path:12-61`); a search names its pattern and where it
/// looked; a command its command line; anything else the first thing that
/// reads as a name.
fn tool_target(name: &str, input: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let text = |key: &str| {
        input
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let flat = name.to_ascii_lowercase();
    if flat.contains("read")
        && let Some(path) = text("file_path").or_else(|| text("path"))
    {
        let offset = input.get("offset").and_then(serde_json::Value::as_u64);
        let limit = input
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .filter(|count| *count > 0);
        return Some(match (offset, limit) {
            (Some(start), Some(count)) => {
                format!(
                    "{path}:{start}-{}",
                    start.saturating_add(count).saturating_sub(1)
                )
            }
            (Some(start), None) => format!("{path}:{start}-"),
            (None, Some(count)) => format!("{path}:1-{count}"),
            (None, None) => path.to_string(),
        });
    }
    if (flat.contains("grep") || flat.contains("search") || flat.contains("glob"))
        && let Some(pattern) = text("pattern").or_else(|| text("query"))
    {
        return Some(match text("path").or_else(|| text("file_path")) {
            Some(path) => format!("{pattern} in {path}"),
            None => pattern.to_string(),
        });
    }
    TARGET_KEYS
        .iter()
        .find_map(|key| text(key))
        .or_else(|| {
            input
                .values()
                .find_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .map(str::to_string)
}

fn tool_result_turn(part: &serde_json::Value, at_ms: Option<i64>) -> TranscriptTurn {
    let output = part.get("content").or_else(|| part.get("output"));
    let text = match output {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(value @ serde_json::Value::Object(_)) => {
            serde_json::to_string_pretty(value).unwrap_or_default()
        }
        _ => String::new(),
    };
    TranscriptTurn {
        role: "tool_result".into(),
        text: tool_detail(&text),
        at_ms,
        tool: Some(transcript_tool(part, true)),
        images: images_in(output),
    }
}

/// The model a transcript says wrote it — its newest word, or `None`.
///
/// The window learns a pane's model from a hook payload carrying `model`, and
/// most events carry none: a pane adopted after the fact, or one whose window
/// restarted after the event that said so, wore its mark with no model beside
/// it for the rest of its life. The transcript the conversation already reads
/// says which model wrote every assistant message, and both vendors write it
/// in the same place — inside `message`, under `assistant` for Claude Code and
/// under `message` for zo.
///
/// `<synthetic>` is skipped. Claude Code writes it on the messages it composes
/// itself — an interrupt notice, an API error — and it names nothing a person
/// could have chosen or could switch away from.
#[must_use]
pub fn model_in(chunk: &str) -> Option<String> {
    chunk.lines().rev().find_map(|line| {
        let row: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let model = row
            .get("message")
            .and_then(|message| message.get("model"))
            // Codex writes it once per turn, on the turn's own context record.
            .or_else(|| turn_context(&row)?.get("model"))?
            .as_str()?
            .trim();
        (!model.is_empty() && !model.starts_with('<')).then(|| model.to_string())
    })
}

/// The spelling of the field [`usage_in`] looks for, used as a byte filter
/// before any line is parsed.
const USAGE_FIELD: &str = "\"usage\"";

/// The context a transcript's last answer stood on — the tokens, and the
/// window they stand in.
///
/// A transcript records what was SPENT, never what the model can hold, so
/// `window` is always 0 here and the page draws the count rather than a
/// ring. The shape is the wire's (`WireUsage`) and zo's channel's on
/// purpose: one chip reads all three roads without branching on which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct TranscriptUsage {
    pub tokens: u64,
    pub window: u64,
}

/// The context the LAST answer in this chunk carried — `input_tokens` plus
/// both cache counts, the same three the wire adds.
///
/// Claude Code and zo stamp it in the same place (`message.usage`, measured
/// against both fixtures) and only on an answer, so the presence of the
/// field is itself the test for whose line it is; asking the row's kind
/// again would be the same question spelled twice, once per vendor. A chunk
/// with no such line answers `None` and nothing is drawn.
///
/// The chunk is a quarter of a megabyte and this runs on every read, so the
/// lines that cannot hold the field are turned away by a byte scan before
/// any of them is parsed: parsing all of them a second time (`turns_in`
/// already parsed them once) put a measured third onto the shell suite's
/// wall clock. A line whose prose merely says `"usage"` costs one parse and
/// the walk goes on, so the answer is the same either way.
#[must_use]
pub fn usage_in(chunk: &str) -> Option<TranscriptUsage> {
    chunk
        .lines()
        .rev()
        .filter(|line| line.contains(USAGE_FIELD))
        .find_map(|line| {
            let row: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
            let usage = row.get("message")?.get("usage")?;
            let count = |name: &str| {
                usage
                    .get(name)
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            };
            let tokens = count("input_tokens")
                + count("cache_creation_input_tokens")
                + count("cache_read_input_tokens");
            (tokens > 0).then_some(TranscriptUsage { tokens, window: 0 })
        })
}

/// A Codex rollout's `turn_context` payload, for a row that is one.
fn turn_context(row: &serde_json::Value) -> Option<&serde_json::Value> {
    (row.get("type").and_then(serde_json::Value::as_str) == Some("turn_context"))
        .then(|| row.get("payload"))
        .flatten()
}

/// The effort a transcript says its newest turn ran at — its newest word, or
/// `None` where the vendor has not said (t-5637).
///
/// Both vendors write it, in their own places: Claude Code stamps `effort`
/// on every assistant record (and the turn's own `perTurnEffort` beside it,
/// which is the one to read — measured 2026-09-21 on 2.1.278, an `/effort
/// low` typed between two turns put `"effort":"low","perTurnEffort":"low"`
/// on the next record); Codex writes `effort` in each turn's `turn_context`
/// (`gpt-6-astra` / `xhigh` in this machine's rollouts). This is the witness
/// the step-effort seat reads a move's landing off: the CLI's own record of
/// what the request went out at, never the word the window typed.
#[must_use]
pub fn effort_in(chunk: &str) -> Option<String> {
    chunk.lines().rev().find_map(|line| {
        let row: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let effort = if row.get("type").and_then(serde_json::Value::as_str) == Some("assistant") {
            row.get("perTurnEffort").or_else(|| row.get("effort"))?
        } else {
            turn_context(&row)?.get("effort")?
        };
        let effort = effort.as_str()?.trim();
        (!effort.is_empty()).then(|| effort.to_string())
    })
}

/// The turns in a stretch of transcript, in the order they were written.
///
/// Complete lines only — the caller reads by byte and stops at the last
/// newline, because a half-written line is not yet a fact. Lines that are not
/// conversation (summaries and meta records) contribute nothing. Tool results
/// carry the vendor's call ID separately from prose; they never impersonate a
/// user message and are expanded only inside their tool cell.
#[must_use]
pub fn turns_in(chunk: &str) -> Vec<TranscriptTurn> {
    let mut turns = Vec::new();
    for line in chunk.lines() {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let response = (row.get("type").and_then(serde_json::Value::as_str)
            == Some("response_item"))
        .then(|| row.get("payload"))
        .flatten();
        if let Some(payload) = response {
            let at_ms = moment_of(&row);
            match payload.get("type").and_then(serde_json::Value::as_str) {
                Some("function_call" | "custom_tool_call") => {
                    if let Some(text) = tool_line(payload) {
                        turns.push(TranscriptTurn {
                            role: "tool".into(),
                            text,
                            at_ms,
                            tool: Some(transcript_tool(payload, false)),
                            images: Vec::new(),
                        });
                    }
                    continue;
                }
                Some("function_call_output" | "custom_tool_call_output") => {
                    turns.push(tool_result_turn(payload, at_ms));
                    continue;
                }
                Some("reasoning") => {
                    // Only the provider's displayed summaries, never its
                    // encrypted reasoning or internal metadata.
                    if let Some(summary) = message_text(payload.get("summary")) {
                        turns.push(TranscriptTurn {
                            role: "thinking".into(),
                            text: summary,
                            at_ms,
                            tool: None,
                            images: Vec::new(),
                        });
                    }
                    continue;
                }
                Some("message") => {}
                _ => continue,
            }
        }
        let message = response.unwrap_or_else(|| row.get("message").unwrap_or(&row));
        // Who is speaking. Claude Code stamps the speaker on the line itself
        // (`type: user|assistant`); zo's session store stamps every
        // conversation line `message` and keeps the speaker on the message
        // (`role`), with tool results as a `tool` role of their own and
        // harness reminders as `system` — neither is somebody speaking.
        let kind = match row
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
        {
            kind @ ("user" | "assistant") => kind,
            "message" | "response_item" => message
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            _ => continue,
        };
        if kind != "user" && kind != "assistant" && kind != "tool" {
            continue;
        }
        // The parts of the message: `content` for Claude Code, `blocks` for zo.
        let content = message.get("content").or_else(|| message.get("blocks"));
        let at_ms = moment_of(&row);
        // Reasoning first — it precedes the answer it led to — as its own
        // turns; the answer's text is read from the parts that are not it.
        let mut spoken = Vec::new();
        for part in content
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if part.get("type").and_then(serde_json::Value::as_str) == Some("tool_result") {
                turns.push(tool_result_turn(part, at_ms));
                continue;
            }
            match reasoning_text(part) {
                Some(thought) if kind == "assistant" => turns.push(TranscriptTurn {
                    role: "thinking".to_string(),
                    text: thought,
                    at_ms,
                    tool: None,
                    images: Vec::new(),
                }),
                _ => spoken.push(part.clone()),
            }
        }
        let said = match content {
            Some(serde_json::Value::Array(_)) => Some(serde_json::Value::Array(spoken)),
            other => other.cloned(),
        };
        let images = if kind == "user" {
            images_in(content)
        } else {
            Vec::new()
        };
        if let Some(text) =
            message_text(said.as_ref()).or_else(|| (!images.is_empty()).then(String::new))
        {
            // A person's turn may be the CLI's own plumbing replayed rather
            // than anything they said: a slash command reaches the transcript
            // as its caveat, its name, its message, its args and its stdout,
            // each as a turn of its own. The hook's `prompt` road has stripped
            // these since it was written (`prompt_in_parsed`); the transcript
            // the conversation page reads never did, so every one of them
            // stood in the conversation wearing the person's voice.
            let text = if kind == "user" {
                past_the_envelopes(&text)
            } else {
                text.as_str()
            };
            let text = text.trim();
            if !text.is_empty() || !images.is_empty() {
                turns.push(TranscriptTurn {
                    role: kind.to_string(),
                    text: text.to_string(),
                    at_ms,
                    tool: None,
                    images,
                });
            }
        }
        for part in content
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if part.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
                continue;
            }
            if let Some(said) = tool_line(part) {
                turns.push(TranscriptTurn {
                    role: "tool".to_string(),
                    text: said,
                    at_ms,
                    tool: Some(transcript_tool(part, false)),
                    images: Vec::new(),
                });
            }
        }
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(edit: &TranscriptEdit) -> Vec<(&str, &str, Option<u32>, Option<u32>)> {
        edit.lines
            .iter()
            .map(|line| (line.kind.as_ref(), line.text.as_str(), line.old, line.new))
            .collect()
    }

    /// A line's inline payloads step aside (t-6323 A8): each base64 run under
    /// `data` or `base64` becomes a place in the file — `"<offset>:<len>"`
    /// counted from where the bytes began — and the line around it is still
    /// the JSON it was. A short run, a quote escaped inside a string and a
    /// line with nothing to set aside are left as they are.
    #[test]
    fn a_lines_payloads_step_aside_and_leave_their_place_in_the_file() {
        let payload = "iVBORw0KGgo".repeat(40);
        let line = format!(
            "{{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{{\"content\":[{{\"tool_use_id\":\"t1\",\"type\":\"tool_result\",\"content\":[{{\"type\":\"image\",\"source\":{{\"type\":\"base64\",\"media_type\":\"image/png\",\"data\":\"{payload}\"}}}}]}}]}},\"toolUseResult\":{{\"type\":\"image\",\"file\":{{\"base64\":\"{payload}\",\"type\":\"image/png\"}}}}}}\n"
        );
        let base: u64 = 1000;
        let read = elide_payloads(line.as_bytes(), base);
        let read = std::str::from_utf8(&read).unwrap();
        assert!(
            read.len() < 400,
            "both copies stepped aside: {} bytes left",
            read.len()
        );
        let row: serde_json::Value = serde_json::from_str(read.trim()).expect("still JSON");
        let at = row["message"]["content"][0]["content"][0]["source"][PAYLOAD_AT_KEY]
            .as_str()
            .unwrap();
        let (offset, len) = at.split_once(':').unwrap();
        let (offset, len): (usize, usize) = (offset.parse().unwrap(), len.parse().unwrap());
        let base = usize::try_from(base).unwrap();
        assert_eq!(
            &line[offset - base..offset - base + len],
            payload,
            "the place is the payload's"
        );
        assert!(is_payload(payload.as_bytes()) && !is_payload(b"a\"b"));
        // The turn says its image by that place.
        let turns = turns_in(read);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].role, "tool_result");
        assert_eq!(
            turns[0].images,
            vec![TranscriptImage {
                media_type: "image/png".into(),
                at: at.to_string()
            }]
        );
        // Left alone: a short run, a digest a person reads, an escaped quote,
        // a plain line.
        let short = r#"{"source":{"data":"QUJD"}}"#;
        assert!(matches!(
            elide_payloads(short.as_bytes(), 0),
            std::borrow::Cow::Borrowed(_)
        ));
        let digest = format!(r#"{{"data":"{}"}}"#, "9f86d081".repeat(8));
        assert!(matches!(
            elide_payloads(digest.as_bytes(), 0),
            std::borrow::Cow::Borrowed(_)
        ));
        let quoted = format!(r#"{{"text":"say \"data\":\"{payload}\" here"}}"#);
        assert_eq!(&*elide_payloads(quoted.as_bytes(), 0), quoted.as_bytes());
    }

    /// A person's pasted image is on their turn, and a message that is only
    /// an image is still their turn — with no words.
    #[test]
    fn a_persons_image_is_on_their_turn_even_with_no_words() {
        let payload = "R0lGODlh".repeat(40);
        let both = format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"[Image #1] 이거 봐\"}},{{\"type\":\"image\",\"source\":{{\"type\":\"base64\",\"media_type\":\"image/gif\",\"data\":\"{payload}\"}}}}]}}}}\n"
        );
        let only = format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"image\",\"source\":{{\"type\":\"base64\",\"media_type\":\"image/jpeg\",\"data\":\"{payload}\"}}}}]}}}}\n"
        );
        let text = format!("{both}{only}");
        let read = elide_payloads(text.as_bytes(), 0);
        let turns = turns_in(std::str::from_utf8(&read).unwrap());
        assert_eq!(turns.len(), 2);
        assert_eq!(
            (turns[0].role.as_str(), turns[0].text.as_str()),
            ("user", "[Image #1] 이거 봐")
        );
        assert_eq!(turns[0].images[0].media_type, "image/gif");
        assert_eq!((turns[1].text.as_str(), turns[1].images.len()), ("", 1));
        // An image nobody set aside has no place to be fetched from.
        assert!(turns_in(&both).iter().all(|turn| turn.images.is_empty()));
    }

    #[test]
    fn a_claude_edit_is_a_snippet_diff_with_blank_gutters() {
        let edits = edits_in(
            "Edit",
            Some(&serde_json::json!({
                "file_path": "/repo/a.rs",
                "old_string": "fn a() {}\nfn b() {}",
                "new_string": "fn a() {}\nfn c() {}"
            })),
        );
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, "/repo/a.rs");
        assert_eq!(
            rows(&edits[0]),
            vec![
                ("ctx", "fn a() {}", None, None),
                ("del", "fn b() {}", None, None),
                ("add", "fn c() {}", None, None),
            ]
        );
        assert_eq!(edits[0].truncated, 0);
    }

    #[test]
    fn a_write_is_every_line_added_and_numbered_from_the_top() {
        let edits = edits_in(
            "Write",
            Some(&serde_json::json!({"file_path": "/repo/new.rs", "content": "one\ntwo\nthree"})),
        );
        assert_eq!(
            rows(&edits[0]),
            vec![
                ("add", "one", None, Some(1)),
                ("add", "two", None, Some(2)),
                ("add", "three", None, Some(3)),
            ]
        );
    }

    #[test]
    fn a_multi_edit_is_one_diff_per_edit_and_a_notebook_edit_is_its_new_source() {
        let edits = edits_in(
            "MultiEdit",
            Some(&serde_json::json!({
                "file_path": "/repo/a.rs",
                "edits": [
                    {"old_string": "x", "new_string": "y"},
                    {"old_string": "p", "new_string": "q"},
                    {"old_string": "same", "new_string": "same"}
                ]
            })),
        );
        assert_eq!(edits.len(), 2, "an edit that changes nothing draws nothing");
        assert_eq!(
            rows(&edits[1]),
            vec![("del", "p", None, None), ("add", "q", None, None)]
        );
        let notebook = edits_in(
            "NotebookEdit",
            Some(&serde_json::json!({"notebook_path": "/repo/n.ipynb", "new_source": "print(1)"})),
        );
        assert_eq!(notebook[0].path, "/repo/n.ipynb");
        assert_eq!(rows(&notebook[0]), vec![("add", "print(1)", None, Some(1))]);
    }

    #[test]
    fn zo_and_gemini_edit_shapes_read_the_same_way() {
        // zo's session store writes the arguments as a STRING, under `path`,
        // camelCase accepted.
        let zo = edits_in(
            "edit_file",
            Some(&serde_json::json!(
                "{\"path\":\"src/main.rs\",\"oldString\":\"a\",\"newString\":\"b\"}"
            )),
        );
        assert_eq!(zo[0].path, "src/main.rs");
        assert_eq!(
            rows(&zo[0]),
            vec![("del", "a", None, None), ("add", "b", None, None)]
        );
        let gemini = edits_in(
            "replace",
            Some(&serde_json::json!({"file_path": "lib.ts", "old_string": "a", "new_string": "b"})),
        );
        assert_eq!(gemini[0].path, "lib.ts");
        let written = edits_in(
            "write_file",
            Some(&serde_json::json!({"path": "notes.md", "content": "# hi"})),
        );
        assert_eq!(rows(&written[0]), vec![("add", "# hi", None, Some(1))]);
    }

    #[test]
    fn a_codex_patch_reads_update_add_move_and_delete() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n*** Move to: src/b.rs\n@@ fn main\n ctx\n-old\n+new\n*** Add File: src/c.rs\n+line one\n+line two\n*** Delete File: src/d.rs\n*** End Patch\n";
        let edits = edits_in("apply_patch", Some(&serde_json::json!(patch)));
        assert_eq!(
            edits
                .iter()
                .map(|edit| edit.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/a.rs", "src/c.rs", "src/d.rs"]
        );
        assert_eq!(
            rows(&edits[0]),
            vec![
                ("meta", "*** Move to: src/b.rs", None, None),
                ("hunk", "@@ fn main", None, None),
                ("ctx", "ctx", None, None),
                ("del", "old", None, None),
                ("add", "new", None, None),
            ]
        );
        assert_eq!(
            rows(&edits[1]),
            vec![
                ("add", "line one", None, Some(1)),
                ("add", "line two", None, Some(2))
            ]
        );
        assert_eq!(
            rows(&edits[2]),
            vec![("meta", "*** Delete File: src/d.rs", None, None)]
        );
        // The same patch as a function call's argument object, and as a
        // shell argv — the two other ways Codex writes it.
        let as_arguments = edits_in("apply_patch", Some(&serde_json::json!({"input": patch})));
        assert_eq!(as_arguments.len(), 3);
        let as_shell = edits_in(
            "shell",
            Some(&serde_json::json!({"command": ["apply_patch", patch]})),
        );
        assert_eq!(as_shell.len(), 3);
    }

    #[test]
    fn a_codex_exec_script_carries_its_patch_as_a_string_literal() {
        let script = "const r = await Promise.all([\n  text(await tools.apply_patch(\"*** Begin Patch\\n*** Update File: a.rs\\n-x \\\"q\\\"\\n+y\\n*** End Patch\")),\n  tools.exec_command({cmd: \"ls\"})\n]);";
        let edits = edits_in("exec", Some(&serde_json::json!(script)));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].path, "a.rs");
        assert_eq!(
            rows(&edits[0]),
            vec![("del", "x \"q\"", None, None), ("add", "y", None, None)]
        );
    }

    #[test]
    fn a_long_write_is_clipped_and_says_how_much_was_left_out() {
        let content: String = (1..=EDIT_DIFF_LINES + 25)
            .map(|n| format!("l{n}\n"))
            .collect();
        let edits = edits_in(
            "Write",
            Some(&serde_json::json!({"file_path": "big", "content": content})),
        );
        assert_eq!(edits[0].lines.len(), EDIT_DIFF_LINES);
        assert_eq!(edits[0].truncated, 25);
    }

    #[test]
    fn reads_searches_and_commands_carry_no_edit() {
        assert!(edits_in("Read", Some(&serde_json::json!({"file_path": "a"}))).is_empty());
        assert!(edits_in("Grep", Some(&serde_json::json!({"pattern": "x"}))).is_empty());
        assert!(edits_in("Bash", Some(&serde_json::json!({"command": "cargo test"}))).is_empty());
        assert!(edits_in("Edit", None).is_empty());
    }

    #[test]
    fn a_transcript_tool_turn_rides_its_edits() {
        let chunk = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/repo/a.rs","old_string":"a","new_string":"b"}}]}}"#;
        let turns = turns_in(chunk);
        let tool = turns[0].tool.as_ref().expect("a tool turn");
        assert_eq!(tool.edits.len(), 1);
        assert_eq!(tool.edits[0].path, "/repo/a.rs");
        let json = serde_json::to_value(&turns[0]).expect("serializes");
        assert_eq!(json["tool"]["edits"][0]["lines"][0]["kind"], "del");
        assert!(
            json["tool"]["edits"][0].get("truncated").is_none(),
            "zero is left off the wire"
        );
    }

    #[test]
    fn a_call_names_the_file_it_read_or_wrote_and_where_as_it_said_it() {
        use serde_json::json;
        // Claude Code's Read: the file and its window, in the CLI's own count.
        let read = file_in(
            "Read",
            Some(&json!({"file_path": "/w/a.rs", "offset": 10930, "limit": 480})),
        )
        .expect("a read names its file");
        assert_eq!(
            read,
            TranscriptFile {
                path: "/w/a.rs".into(),
                offset: Some(10930),
                limit: Some(480),
                search: None,
            }
        );
        // An edit is found by the text it wrote; a multi-edit by its first.
        let edit = file_in(
            "Edit",
            Some(&json!({"file_path": "/w/b.rs", "old_string": "a", "new_string": "let fixed = true;"})),
        )
        .expect("an edit names its file");
        assert_eq!(edit.search.as_deref(), Some("let fixed = true;"));
        assert_eq!((edit.offset, edit.limit), (None, None));
        let multi = file_in(
            "MultiEdit",
            Some(&json!({"file_path": "/w/c.rs", "edits": [{"old_string": "x", "new_string": "y = 1"}]})),
        )
        .expect("a multi-edit names its file");
        assert_eq!(multi.search.as_deref(), Some("y = 1"));
        // A write opens at the top; zo's read names its file `path`.
        let write = file_in(
            "Write",
            Some(&json!({"file_path": "/w/d.rs", "content": "x"})),
        )
        .expect("a write names its file");
        assert_eq!((write.search, write.offset), (None, None));
        let zo = file_in(
            "read_file",
            Some(&json!({"path": "src/e.rs", "offset": 0, "limit": 20})),
        )
        .expect("zo's read names its file");
        assert_eq!(
            (zo.path.as_str(), zo.offset, zo.limit),
            ("src/e.rs", Some(0), Some(20))
        );
        // A search names a directory and a command a line: no file.
        assert_eq!(
            file_in("Grep", Some(&json!({"pattern": "x", "path": "/w"}))),
            None
        );
        assert_eq!(
            file_in("Bash", Some(&json!({"command": "cat /w/a.rs"}))),
            None
        );
        // A patch's files are its diff's, not one path to open.
        assert_eq!(
            file_in("apply_patch", Some(&json!("*** Begin Patch"))),
            None
        );
        // A long new text is searched by its start.
        let long = "x".repeat(FILE_SEARCH_CHARS * 2);
        let searched = file_in(
            "Edit",
            Some(&json!({"file_path": "/w/f.rs", "new_string": long})),
        )
        .and_then(|file| file.search)
        .expect("an edit's text is searched");
        assert_eq!(searched.chars().count(), FILE_SEARCH_CHARS);
        // And the transcript's own turn carries it.
        let call = json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/w/a.rs", "offset": 5}}
        ]}});
        let turns = turns_in(&format!("{call}\n"));
        assert_eq!(
            turns[0]
                .tool
                .as_ref()
                .and_then(|tool| tool.file.as_ref())
                .and_then(|file| file.offset),
            Some(5)
        );
    }

    #[test]
    fn tool_details_preserve_command_and_output_whitespace() {
        let input = "  printf 'value'\n";
        let output = "\n  indented output  \n";
        let call = serde_json::json!({"type":"assistant","message":{"content":[{
            "type":"tool_use","id":"exact","name":"Bash","input":{"command":input}
        }]}});
        let result = serde_json::json!({"type":"user","message":{"content":[{
            "type":"tool_result","tool_use_id":"exact","content":output
        }]}});
        let turns = turns_in(&format!("{call}\n{result}\n"));
        assert_eq!(turns[0].tool.as_ref().unwrap().input, input);
        assert_eq!(turns[1].text, output);
    }

    #[test]
    fn codex_response_items_share_the_tool_cell_contract_without_internal_metadata() {
        let source = [
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","id":"record-a","call_id":"call-a","name":"functions.exec","input":"text(1);\ntext(2);"}}"#,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call_output","id":"record-b","call_id":"call-a","output":"1\n2"}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Visible summary"}],"encrypted_content":"not-display-content"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
        ].join("\n");
        let turns = turns_in(&source);
        assert_eq!(
            turns
                .iter()
                .map(|turn| turn.role.as_str())
                .collect::<Vec<_>>(),
            ["user", "tool", "tool_result", "thinking", "assistant"]
        );
        assert_eq!(turns[1].tool.as_ref().unwrap().call_id, "call-a");
        assert_eq!(turns[2].tool.as_ref().unwrap().call_id, "call-a");
        assert_eq!(turns[1].tool.as_ref().unwrap().input, "text(1);\ntext(2);");
        assert_eq!(turns[3].text, "Visible summary");
        assert!(
            !serde_json::to_string(&turns)
                .unwrap()
                .contains("not-display-content")
        );
    }

    #[test]
    fn tool_results_keep_the_call_identity_and_never_become_user_speech() {
        let input = [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"call-a","name":"Bash","input":{"command":"printf first\\nprintf second"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call-a","is_error":true,"content":"first\nsecond\nexit code 1"}]}}"#,
            r#"{"type":"message","message":{"role":"tool","blocks":[{"type":"tool_result","tool_use_id":"call-b","tool_name":"bash","output":"another result","is_error":false}]}}"#,
        ].join("\n");
        let turns = turns_in(&input);
        assert_eq!(
            turns
                .iter()
                .map(|turn| turn.role.as_str())
                .collect::<Vec<_>>(),
            ["tool", "tool_result", "tool_result"]
        );
        let call = turns[0].tool.as_ref().expect("tool identity");
        assert_eq!(call.call_id, "call-a");
        assert!(call.input.contains("printf second"));
        let result = turns[1].tool.as_ref().expect("result identity");
        assert_eq!(result.call_id, call.call_id);
        assert!(result.is_error);
        assert_eq!(turns[1].text, "first\nsecond\nexit code 1");
        assert_eq!(turns[2].tool.as_ref().expect("zo result").call_id, "call-b");
    }

    #[test]
    fn an_expanded_tool_result_is_bounded_without_splitting_unicode() {
        let source = serde_json::json!({"type":"user","message":{"content":[{
            "type":"tool_result","tool_use_id":"big","content":"가".repeat(TOOL_DETAIL_CHARS + 10)
        }]}})
        .to_string();
        let turns = turns_in(&source);
        assert_eq!(turns[0].text.chars().count(), TOOL_DETAIL_CHARS + 1);
        assert!(turns[0].text.ends_with('…'));
    }

    #[test]
    fn oversized_or_split_utf8_records_cannot_stall_the_following_turns() {
        let first = complete_transcript_chunk(b"{\"large\":\"abcdef", false, true);
        assert!(first.bytes.is_empty() && first.skipped);
        assert_eq!(first.consumed, b"{\"large\":\"abcdef".len());
        let next = b"tail\"}\n{\"type\":\"user\",\"message\":{\"content\":\"next\"}}\n\xe3\x81";
        let complete = complete_transcript_chunk(next, true, true);
        assert_eq!(complete.consumed, next.len() - 2);
        assert_eq!(
            turns_in(std::str::from_utf8(complete.bytes).unwrap())[0].text,
            "next"
        );
        let unfinished = complete_transcript_chunk(b"{\"not-yet-complete", false, false);
        assert_eq!(unfinished.consumed, 0);
        assert!(!unfinished.skipped);
    }

    /// A turn nobody typed does not name an agent.
    ///
    /// Reported from the sidebar: a row whose title was
    /// `<task-notification> <task-id>…`. The harness resumed the agent, the
    /// CLI passed that envelope as the turn's prompt, and the row printed this
    /// window's own plumbing back at the person reading it.
    #[test]
    fn a_machine_envelope_is_not_a_prompt() {
        let payload = |prompt: &str| serde_json::json!({ "prompt": prompt }).to_string();

        // Nothing but machinery: no prompt at all, and the row falls back to
        // its state word.
        let notification = "<task-notification>\n<task-id>b0fvr1uba</task-id>\n             <status>completed</status>\n</task-notification>";
        assert_eq!(prompt_in_payload(&payload(notification)), None);

        // Scaffolding with a person's sentence after it: the sentence.
        let commanded = "<command-name>/loop</command-name>\n             <command-message>loop</command-message>\n             <command-args>사용량 화면 확인</command-args>\n사용량 화면 확인";
        assert_eq!(
            prompt_in_payload(&payload(commanded)).as_deref(),
            Some("사용량 화면 확인")
        );

        // A reminder wrapped around a real question.
        let reminded =
            "<system-reminder>context follows</system-reminder>\nwhy is the chart empty?";
        assert_eq!(
            prompt_in_payload(&payload(reminded)).as_deref(),
            Some("why is the chart empty?")
        );

        // An unclosed envelope has no sentence to find after it.
        assert_eq!(
            prompt_in_payload(&payload("<task-notification>partial")),
            None
        );

        // A local command's own output, replayed with the caveat that
        // introduces it and the sentence the person typed underneath — seen
        // verbatim after `/model` in a live session.
        let printed = "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>\n<command-name>/model</command-name>\n<command-args></command-args>\n<local-command-stdout>Set model to Opus 5</local-command-stdout>\n계속";
        assert_eq!(
            prompt_in_payload(&payload(printed)).as_deref(),
            Some("계속")
        );

        // And a person pasting markup meant to paste markup.
        assert_eq!(
            prompt_in_payload(&payload("<div>keep me</div>")).as_deref(),
            Some("<div>keep me</div>")
        );
    }

    /// How full a pane's context is, recovered from the file it already
    /// reads. A pane with neither a wire nor a channel has no other source:
    /// hooks carry no token counts, so without this the composer's meter
    /// would be blank for exactly the panes that run longest.
    #[test]
    fn a_transcript_names_the_context_its_last_answer_stood_on() {
        let answered = |input: u64, write: u64, read: u64| {
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "usage": {
                "input_tokens": input, "cache_creation_input_tokens": write,
                "cache_read_input_tokens": read, "output_tokens": 40}}})
            .to_string()
        };
        // The three input counts are the prompt; the output is not context yet.
        assert_eq!(
            usage_in(&answered(2, 17628, 9000)),
            Some(TranscriptUsage {
                tokens: 26630,
                window: 0
            })
        );
        // The LAST answer is the one that says where the session stands now.
        let asked =
            serde_json::json!({"type": "user", "message": {"role": "user", "content": "go"}})
                .to_string();
        assert_eq!(
            usage_in(&[answered(1, 100, 0), asked, answered(1, 0, 900)].join("\n"))
                .map(|held| held.tokens),
            Some(901)
        );
        // zo stamps it in the same place under its own line kind.
        assert_eq!(
            usage_in(
                &serde_json::json!({"type": "message", "message": {"role": "assistant", "usage": {
                    "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "input_tokens": 2,
                    "output_tokens": 14}}})
                    .to_string()
            )
            .map(|held| held.tokens),
            Some(2)
        );
        // Nothing spent, nothing said — and a file with no such line at all.
        assert_eq!(usage_in(&answered(0, 0, 0)), None);
        assert_eq!(usage_in(&asked_only()), None);
        assert_eq!(usage_in(""), None);
        assert_eq!(usage_in("not json at all"), None);
    }

    fn asked_only() -> String {
        serde_json::json!({"type": "user", "message": {"role": "user", "content": "go"}})
            .to_string()
    }

    /// The model a pane is on, recovered from the transcript it already
    /// reads. The window learns a model only from a hook payload that carries
    /// one (`ui/shell.js`), and most events carry none — so a pane adopted
    /// after the fact, or one whose window restarted, wore `✻ Claude ▾` with
    /// no model beside it for the rest of its life. Every assistant message
    /// says which model wrote it.
    #[test]
    fn a_transcript_names_the_model_that_wrote_it() {
        let wrote = |model: &str| {
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "model": model, "content": "hi"}})
                .to_string()
        };
        assert_eq!(
            model_in(&wrote("claude-opus-5")),
            Some("claude-opus-5".to_string())
        );
        // zo writes the same field under `type: "message"`.
        assert_eq!(
            model_in(
                &serde_json::json!({"type": "message", "message": {"role": "assistant", "model": "gpt-5.6-sol", "blocks": []}})
                    .to_string()
            ),
            Some("gpt-5.6-sol".to_string())
        );
        // The newest word wins: a session whose model was switched mid-way is
        // on the one it is on now.
        assert_eq!(
            model_in(&[wrote("claude-opus-5"), wrote("claude-fable-5")].join("\n")),
            Some("claude-fable-5".to_string())
        );
        // `<synthetic>` is what Claude Code writes on the messages it makes up
        // itself — an interrupt notice, an error. Naming it on the chip would
        // put a word there that no person can choose.
        assert_eq!(
            model_in(&[wrote("claude-opus-5"), wrote("<synthetic>")].join("\n")),
            Some("claude-opus-5".to_string())
        );
        assert_eq!(model_in(""), None);
        assert_eq!(model_in("not json at all"), None);
    }

    /// A slash command's replay is the CLI's own plumbing, not a person
    /// speaking. `/effort` reaches the transcript as four turns — a caveat,
    /// the command's name, its message, its args — and then the local
    /// command's stdout as a fifth. Every one of them was standing in the
    /// conversation as a bubble with the person's own voice, because the
    /// envelope stripper was only ever wired to the hook's `prompt` field
    /// (`prompt_in_parsed`) and never to the transcript the page reads.
    /// Each vendor's own word for the effort a turn ran at, newest first —
    /// Claude Code's per-turn stamp and Codex's turn context — and nothing
    /// for a record that carries none.
    #[test]
    fn effort_in_reads_each_vendors_own_stamp() {
        let claude = |effort: &str, per_turn: Option<&str>| {
            let mut row = serde_json::json!({
                "type": "assistant", "effort": effort,
                "message": {"role": "assistant", "model": "claude-fable-5-1", "content": []}
            });
            if let Some(per_turn) = per_turn {
                row["perTurnEffort"] = serde_json::json!(per_turn);
            }
            row.to_string()
        };
        let codex = |model: &str, effort: &str| {
            serde_json::json!({
                "type": "turn_context",
                "payload": {"turn_id": "t", "model": model, "effort": effort, "summary": "none"}
            })
            .to_string()
        };
        assert_eq!(
            effort_in(&[claude("medium", Some("medium")), claude("low", Some("low"))].join("\n")),
            Some("low".to_string())
        );
        // The turn's own word outranks the session's when the two differ.
        assert_eq!(
            effort_in(&claude("xhigh", Some("ultracode"))),
            Some("ultracode".to_string())
        );
        assert_eq!(effort_in(&claude("high", None)), Some("high".to_string()));
        assert_eq!(
            effort_in(&[codex("gpt-6-astra", "xhigh"), codex("gpt-6-astra", "low")].join("\n")),
            Some("low".to_string())
        );
        assert_eq!(
            model_in(&codex("gpt-6-astra", "xhigh")),
            Some("gpt-6-astra".to_string())
        );
        assert_eq!(
            effort_in(r#"{"type":"user","message":{"role":"user","content":"hi"}}"#),
            None
        );
        assert_eq!(effort_in(""), None);
        assert_eq!(effort_in("not json"), None);
    }

    #[test]
    fn a_slash_commands_replay_is_not_somebody_speaking() {
        let said = |text: &str| {
            serde_json::json!({"type": "user", "message": {"role": "user", "content": text}})
                .to_string()
        };
        let chunk = [
            said("<local-command-caveat>Caveat: The messages below were generated by the user while running local commands. DO NOT respond to these messages or otherwise consider them in your response unless the user explicitly asks you to.</local-command-caveat>"),
            said("<command-name>/effort</command-name>\n          <command-message>effort</command-message>\n          <command-args></command-args>"),
            said("<local-command-stdout>Set effort level to max (this session only): Maximum capability with deepest reasoning.</local-command-stdout>"),
            said("<system-reminder>\n# Recalled memory\n</system-reminder>"),
            said("<local-command-caveat>ignore me</local-command-caveat>and this is mine"),
            said("<div>I meant to paste this</div>"),
        ]
        .join("\n");
        assert_eq!(
            turns_in(&chunk)
                .into_iter()
                .map(|turn| (turn.role, turn.text))
                .collect::<Vec<_>>(),
            vec![
                // What was typed after the envelope is still the person's.
                ("user".to_string(), "and this is mine".to_string()),
                // A closed list, not "anything in angle brackets": somebody
                // pasting `<div>` meant to paste `<div>`.
                (
                    "user".to_string(),
                    "<div>I meant to paste this</div>".to_string()
                ),
            ],
            "a replayed slash command has nothing in it a reader wants"
        );
    }

    /// A helper's page is the CONVERSATION, not the wire: the person's turns,
    /// the assistant's, and the tools it reached for — and nothing else, so a
    /// page nobody can read never stands where a page somebody can read was
    /// asked for.
    #[test]
    fn a_stretch_of_transcript_reads_as_its_conversation() {
        let chunk = [
            r#"{"type":"user","message":{"role":"user","content":"map the vault"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Reading it now."},{"type":"tool_use","name":"Read","input":{"file_path":"/repo/vault.rs"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"…file…"}]}}"#,
            r#"{"type":"summary","summary":"nothing a reader wants"}"#,
            "not json at all",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash"}]}}"#,
        ]
        .join("\n");
        let turns: Vec<_> = turns_in(&chunk)
            .into_iter()
            .filter(|turn| turn.role != "tool_result")
            .map(|mut turn| {
                turn.tool = None;
                turn
            })
            .collect();
        assert_eq!(
            turns,
            vec![
                TranscriptTurn {
                    role: "user".into(),
                    text: "map the vault".into(),
                    at_ms: None,
                    tool: None,
                    images: Vec::new(),
                },
                TranscriptTurn {
                    role: "assistant".into(),
                    text: "Reading it now.".into(),
                    at_ms: None,
                    tool: None,
                    images: Vec::new(),
                },
                TranscriptTurn {
                    role: "tool".into(),
                    text: "Read · /repo/vault.rs".into(),
                    at_ms: None,
                    tool: None,
                    images: Vec::new(),
                },
                TranscriptTurn {
                    role: "tool".into(),
                    text: "Bash".into(),
                    at_ms: None,
                    tool: None,
                    images: Vec::new(),
                },
            ],
            "the conversation came back with the wire in it, or short of a turn"
        );
        // A half-written last line is not yet a fact — the caller stops at the
        // last newline, and what it hands over parses whole.
        assert!(turns_in(r#"{"type":"user","message":{"content":"cut o"#).is_empty());
    }

    /// A turn carries the moment it was written, so a page can say how long
    /// a run of tool calls took ("8m 45s 동안 작업") instead of the moment it
    /// happened to read them. Claude Code stamps every line `timestamp` (ISO
    /// 8601); zo's session store stamps a message record `updated_at_ms`
    /// (epoch ms). A line with neither has no moment, and the page falls back
    /// to its own clock rather than this file guessing one.
    #[test]
    fn a_turn_carries_the_moment_it_was_written() {
        let chunk = [
            r#"{"type":"user","timestamp":"2026-09-07T02:10:11.500Z","message":{"role":"user","content":"go"}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-07T02:10:20Z","message":{"role":"assistant","content":[{"type":"text","text":"Reading."},{"type":"tool_use","name":"Read","input":{"file_path":"/repo/a.rs"}}]}}"#,
            r#"{"message":{"blocks":[{"text":"done","type":"text"}],"role":"assistant"},"turn_index":2,"updated_at_ms":1788747100000,"type":"message"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"unstamped"}]}}"#,
        ]
        .join("\n");
        let moments: Vec<Option<i64>> = turns_in(&chunk).iter().map(|turn| turn.at_ms).collect();
        assert_eq!(
            moments,
            vec![
                Some(1_788_747_011_500),
                Some(1_788_747_020_000),
                // The tool call is stamped with the message that made it.
                Some(1_788_747_020_000),
                Some(1_788_747_100_000),
                None,
            ],
            "a turn's moment is read from the line that carries it, or is absent"
        );
    }

    /// zo's session store writes a call's arguments as a JSON string, and a
    /// transcript shows the argument the terminal would: the file with its
    /// window of lines, the pattern with where it looked, the command line.
    #[test]
    fn a_tool_names_what_a_terminal_transcript_would_whatever_shape_its_input_took() {
        let line = |name: &str, input: serde_json::Value| {
            tool_line(&serde_json::json!({ "type": "tool_use", "name": name, "input": input }))
                .expect("a named call")
        };
        // The arguments as a string, the way zo writes them.
        assert_eq!(
            line(
                "read_file",
                serde_json::json!("{\"path\": \"/repo/ui/tokens.css\", \"limit\": 200}")
            ),
            "read_file · /repo/ui/tokens.css:1-200"
        );
        assert_eq!(
            line(
                "read_file",
                serde_json::json!({ "path": "/repo/a.rs", "offset": 6620, "limit": 50 })
            ),
            "read_file · /repo/a.rs:6620-6669"
        );
        assert_eq!(
            line(
                "grep_search",
                serde_json::json!(
                    "{\"path\": \"/repo/ui\", \"pattern\": \"makeTermView\", \"head_limit\": 20}"
                )
            ),
            "grep_search · makeTermView in /repo/ui"
        );
        assert_eq!(
            line("glob_search", serde_json::json!({ "pattern": "**/*.css" })),
            "glob_search · **/*.css"
        );
        // A command line, and a spawn's description — the key wins over the
        // order the arguments happened to be written in.
        assert_eq!(
            line(
                "bash",
                serde_json::json!({ "timeout": "30", "command": "cargo test" })
            ),
            "bash · cargo test"
        );
        assert_eq!(
            line(
                "Agent",
                serde_json::json!({ "subagent_type": "Explore", "description": "map the vault", "prompt": "…" })
            ),
            "Agent · map the vault"
        );
        // Not an object at all: the verb alone, as before.
        assert_eq!(line("bash", serde_json::json!("not json")), "bash");
        assert_eq!(line("bash", serde_json::json!(7)), "bash");
    }

    /// zo keeps a helper's transcript in its own session store: every
    /// conversation line is `type: message` with the speaker on the message,
    /// text in `blocks`, tool results as a `tool` role and harness reminders
    /// as `system`. The page reads the same conversation out of that shape —
    /// the person's prompt first, so a helper's row answers "what was it sent".
    #[test]
    fn a_zo_session_store_reads_as_the_same_conversation() {
        let chunk = [
            r#"{"created_at_ms":1,"session_id":"session-1","type":"session_meta","version":1}"#,
            r#"{"message":{"blocks":[{"text":"조사하되 코드는 수정하지 마라","type":"text"}],"role":"user"},"turn_index":0,"type":"message"}"#,
            r#"{"message":{"blocks":[{"text":"I'll start by exploring.","type":"text"},{"id":"toolu_1","input":{"command":"ls -la"},"name":"bash","type":"tool_use"}],"role":"assistant"},"turn_index":1,"type":"message"}"#,
            r#"{"message":{"blocks":[{"is_error":false,"output":"…listing…","tool_name":"bash","tool_use_id":"toolu_1","type":"tool_result"}],"role":"tool"},"turn_index":1,"type":"message"}"#,
            r#"{"message":{"blocks":[{"text":"<system-reminder>todo</system-reminder>","type":"text"}],"role":"system"},"turn_index":1,"type":"message"}"#,
        ]
        .join("\n");
        assert_eq!(
            turns_in(&chunk)
                .into_iter()
                .filter(|turn| turn.role != "tool_result")
                .map(|mut turn| {
                    turn.tool = None;
                    turn
                })
                .collect::<Vec<_>>(),
            vec![
                TranscriptTurn {
                    role: "user".into(),
                    text: "조사하되 코드는 수정하지 마라".into(),
                    at_ms: None,
                    tool: None,
                    images: Vec::new(),
                },
                TranscriptTurn {
                    role: "assistant".into(),
                    text: "I'll start by exploring.".into(),
                    at_ms: None,
                    tool: None,
                    images: Vec::new(),
                },
                TranscriptTurn {
                    role: "tool".into(),
                    text: "bash · ls -la".into(),
                    at_ms: None,
                    tool: None,
                    images: Vec::new(),
                },
            ],
            "zo's store came back with its wire in it, or short of a turn"
        );
    }
    /// Reasoning is a turn of its own, never prose. zo writes the model's
    /// thought as a `thinking` block and carries an earlier turn's thought
    /// into the next as a text block wearing `[earlier reasoning]` — the
    /// page printed that label and the bold summary as the agent's answer.
    #[test]
    fn reasoning_blocks_and_the_passport_are_thinking_turns_not_prose() {
        let chunk = [
            r#"{"message":{"blocks":[{"text":"go","type":"text"}],"role":"user"},"turn_index":0,"type":"message"}"#,
            concat!(
                r#"{"message":{"blocks":["#,
                r#"{"signature":"","thinking":"**Checking the flow**\n\nThe flag gates the call.","type":"thinking"},"#,
                r#"{"text":"[earlier reasoning]\n**Locating the source**\n\nThe strings match.","type":"text"},"#,
                r#"{"text":"Reading the procedure now.","type":"text"}"#,
                r#"],"role":"assistant"},"turn_index":1,"type":"message"}"#
            ),
        ]
        .join("\n");
        let roles: Vec<(String, String)> = turns_in(&chunk)
            .into_iter()
            .map(|turn| (turn.role, turn.text))
            .collect();
        assert_eq!(
            roles,
            vec![
                ("user".to_string(), "go".to_string()),
                (
                    "thinking".to_string(),
                    "**Checking the flow**\n\nThe flag gates the call.".to_string()
                ),
                (
                    "thinking".to_string(),
                    "**Locating the source**\n\nThe strings match.".to_string()
                ),
                (
                    "assistant".to_string(),
                    "Reading the procedure now.".to_string()
                ),
            ],
            "a thought or a passport leaked into the prose, or lost its words"
        );
        // A person's text block that happens to start with the label is not
        // reasoning: only the assistant reasons.
        let person = r#"{"message":{"blocks":[{"text":"[earlier reasoning] is what it printed","type":"text"}],"role":"user"},"turn_index":0,"type":"message"}"#;
        assert_eq!(turns_in(person)[0].role, "user");
    }
    use std::io::Write as _;

    /// The two real shapes, from real files: claude's nested message and a
    /// flat `role`/`content` line; tool-result and user lines answer nothing.
    #[test]
    fn the_tail_finds_the_last_assistant_turn_and_skips_what_is_not_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("session.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"an older answer"}]}}"#, "\n",
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash"}]}}"#, "\n",
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the last answer"}]}}"#, "\n",
                r#"{"type":"user","message":{"role":"user","content":"a question after it"}}"#, "\n",
                "not json at all\n",
            ),
        )
        .expect("write");
        assert_eq!(
            last_assistant_message(&file).as_deref(),
            Some("the last answer"),
            "the user turn and the junk line after it must not stop the scan"
        );

        let flat = dir.path().join("flat.jsonl");
        std::fs::write(
            &flat,
            "{\"role\":\"assistant\",\"content\":\"flat words\"}\n",
        )
        .expect("write");
        assert_eq!(last_assistant_message(&flat).as_deref(), Some("flat words"));
    }

    /// A tail that starts mid-file drops its first, partial line rather than
    /// parsing somebody's half — and the read never exceeds its bound.
    #[test]
    fn a_long_file_is_read_from_its_tail_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("long.jsonl");
        let mut writing = std::fs::File::create(&file).expect("create");
        // Far more than the bound, so the answer near the start is out of
        // reach and the one near the end is what the tail holds.
        writeln!(
            writing,
            r#"{{"role":"assistant","content":"buried beyond the bound"}}"#
        )
        .expect("head");
        let filler = format!(r#"{{"type":"filler","pad":"{}"}}"#, "x".repeat(1024));
        for _ in 0..((MAX_TAIL_BYTES / 1024) + 32) {
            writeln!(writing, "{filler}").expect("fill");
        }
        writeln!(
            writing,
            r#"{{"role":"assistant","content":"near the end"}}"#
        )
        .expect("tail");
        drop(writing);
        assert_eq!(
            last_assistant_message(&file).as_deref(),
            Some("near the end")
        );

        // Nothing but filler in the tail: the honest answer is nothing, not a
        // deeper dig past the bound.
        let quiet = dir.path().join("quiet.jsonl");
        let mut writing = std::fs::File::create(&quiet).expect("create");
        writeln!(
            writing,
            r#"{{"role":"assistant","content":"only at the very start"}}"#
        )
        .expect("head");
        for _ in 0..((MAX_TAIL_BYTES / 1024) + 32) {
            writeln!(writing, "{filler}").expect("fill");
        }
        drop(writing);
        assert_eq!(last_assistant_message(&quiet), None);
    }

    /// Orca's three answers in order: the explicit field wins, the named
    /// transcript is the fallback, and neither is a clear — spelled `None`.
    #[test]
    fn a_stop_payload_answers_directly_or_through_its_transcript_or_not_at_all() {
        assert_eq!(
            said_in_payload(r#"{"last_assistant_message":"done, tests pass"}"#).as_deref(),
            Some("done, tests pass")
        );
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("t.jsonl");
        std::fs::write(
            &file,
            "{\"role\":\"assistant\",\"content\":\"from the file\"}\n",
        )
        .expect("write");
        let payload = format!(
            r#"{{"hook_event_name":"Stop","transcript_path":{}}}"#,
            serde_json::json!(file.to_string_lossy())
        );
        assert_eq!(said_in_payload(&payload).as_deref(), Some("from the file"));
        assert_eq!(
            said_in_payload(r#"{"hook_event_name":"Stop"}"#),
            None,
            "no field and no path is a clear, not a keep"
        );
    }

    /// The tool-result shapes, each on its own row (`extractToolResponseText`,
    /// out/main/index.js:9067): the bare string, the object's telling fields
    /// in their order, the first `content[]` part with text — and the failure
    /// spelling's fallback through `error` and `message`.
    #[test]
    fn a_tool_result_updates_the_answer_line_in_orcas_shapes() {
        assert_eq!(
            tool_said_in_payload(r#"{"tool_response":"3 files changed"}"#).as_deref(),
            Some("3 files changed")
        );
        assert_eq!(
            tool_said_in_payload(
                r#"{"tool_response":{"text_result_for_llm":"the telling field","text":"not this"}}"#
            )
            .as_deref(),
            Some("the telling field")
        );
        assert_eq!(
            tool_said_in_payload(
                r#"{"tool_response":{"content":[{"type":"image"},{"type":"text","text":"from content"}]}}"#
            )
            .as_deref(),
            Some("from content")
        );
        assert_eq!(
            tool_said_in_payload(r#"{"tool_response":{"ok":true}}"#),
            None
        );
        assert_eq!(tool_said_in_payload(r#"{"tool_response":""}"#), None);
        assert_eq!(
            tool_failure_in_payload(r#"{"tool_response":{},"error":"exit 1: no such file"}"#)
                .as_deref(),
            Some("exit 1: no such file")
        );
        assert_eq!(
            tool_failure_in_payload(r#"{"message":"the tool timed out"}"#).as_deref(),
            Some("the tool timed out")
        );
        assert_eq!(
            tool_failure_in_payload(r#"{"hook_event_name":"PostToolUseFailure"}"#),
            None
        );
    }

    /// The three ask shapes, in the order the payload's own fields identify
    /// them — and the question tool wins over its own approval reading, or a
    /// card would say `AskUserQuestion` instead of the question.
    #[test]
    fn an_ask_names_the_question_the_approval_or_the_message() {
        let asked = ask_in_payload(
            r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion",
                "tool_input":{"questions":[{"question":"어느 DB로 갈까요?"},{"question":"마이그레이션은 지금?"}]}}"#,
        );
        assert_eq!(
            asked.as_deref(),
            Some("어느 DB로 갈까요? 마이그레이션은 지금?")
        );

        let approval = ask_in_payload(
            r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash",
                "tool_input":{"command":"rm -rf build"}}"#,
        );
        assert_eq!(approval.as_deref(), Some("Bash · rm -rf build"));
        // A tool with no identifying string is still named — the tool alone
        // beats a blank amber box.
        assert_eq!(
            ask_in_payload(r#"{"tool_name":"WebSearch","tool_input":{"query_count":3}}"#)
                .as_deref(),
            Some("WebSearch")
        );

        let told = ask_in_payload(
            r#"{"hook_event_name":"Notification","message":"Claude is waiting for your input"}"#,
        );
        assert_eq!(told.as_deref(), Some("Claude is waiting for your input"));

        assert_eq!(
            ask_in_payload(r#"{"hook_event_name":"Notification"}"#),
            None
        );
        assert_eq!(ask_in_payload("not json"), None);
    }

    /// The prompt needs no file: it rides the payload, and clamps like
    /// everything else that lands on a card.
    #[test]
    fn a_prompt_is_read_off_the_payload_and_clamped() {
        assert_eq!(
            prompt_in_payload(r#"{"prompt":"  fix the flaky test  "}"#).as_deref(),
            Some("fix the flaky test")
        );
        assert_eq!(prompt_in_payload(r#"{"prompt":"   "}"#), None);
        assert_eq!(prompt_in_payload("not json"), None);
        let long = format!(r#"{{"prompt":"{}"}}"#, "word ".repeat(200));
        let cut = prompt_in_payload(&long).expect("a long prompt still answers");
        assert!(cut.chars().count() <= SUMMARY_CHARS + 1, "{}", cut.len());
        assert!(cut.ends_with('…'));
    }

    /// Codex rollout lines, in the shapes the real files carry (measured
    /// against `codex-runtime-home/home/sessions/**/rollout-*.jsonl`).
    mod rollout {
        pub const META: &str = r#"{"timestamp":"2026-09-05T05:22:11.000Z","type":"session_meta","payload":{"id":"01a07004-65d6-7c32-adf7-577ce8419cff","cwd":"/wt","source":"cli"}}"#;
        pub const CONTEXT: &str = r#"{"timestamp":"2026-09-05T05:27:08.500Z","type":"turn_context","payload":{"cwd":"/wt","model":"gpt-6-astra"}}"#;
        pub const STARTED: &str = r#"{"timestamp":"2026-09-05T05:27:08.553Z","type":"event_msg","payload":{"type":"task_started","turn_id":"01a07008","model_context_window":258400}}"#;
        pub const CALL: &str = r#"{"timestamp":"2026-09-05T05:31:37.111Z","type":"response_item","payload":{"type":"custom_tool_call","status":"completed","call_id":"call_1","name":"exec","input":"cargo build"}}"#;
        pub const ITEM: &str = r#"{"timestamp":"2026-09-05T05:31:37.338Z","type":"event_msg","payload":{"type":"item_completed","turn_id":"01a07008","item":{"type":"CommandExecution","status":"completed"}}}"#;
        pub const OUTPUT: &str = r#"{"timestamp":"2026-09-05T05:31:37.375Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_1","output":"grep saw \"event_msg\" and \"task_complete\" in a file"}}"#;
        pub const TOKENS: &str = r#"{"timestamp":"2026-09-05T05:31:37.376Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1}}}}"#;
        pub const COMPLETE: &str = r#"{"timestamp":"2026-09-05T05:31:44.900Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"01a07008","last_agent_message":"done"}}"#;
        pub const SETTINGS: &str = r#"{"timestamp":"2026-09-05T05:31:45.000Z","type":"event_msg","payload":{"type":"thread_settings_applied","model":"gpt-6-astra"}}"#;
        pub const ABORTED: &str = r#"{"timestamp":"2026-09-05T05:31:44.900Z","type":"event_msg","payload":{"type":"turn_aborted","turn_id":"01a07008","reason":"interrupted","duration_ms":10406}}"#;
    }

    fn rollout_at(dir: &Path, name: &str, lines: &[&str]) -> std::path::PathBuf {
        let file = dir.join(name);
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(&file, text).expect("write a rollout");
        file
    }

    /// The t-2874 evidence: a Codex worker's hook said `idle` while its
    /// rollout went on recording a `CommandExecution` — the file, not the
    /// hook, knows the turn was cut. A turn closed by `task_complete` or by
    /// a person's Esc (`turn_aborted`) is not open, whatever follows it.
    #[test]
    fn a_codex_rollout_that_stops_inside_a_turn_is_open() {
        use rollout::*;
        let dir = tempfile::tempdir().expect("tempdir");
        let open = rollout_at(
            dir.path(),
            "open.jsonl",
            &[META, CONTEXT, STARTED, CALL, ITEM, OUTPUT, TOKENS],
        );
        assert_eq!(codex_turn_open(&open), Some(true));

        let closed = rollout_at(
            dir.path(),
            "closed.jsonl",
            &[META, CONTEXT, STARTED, CALL, ITEM, OUTPUT, TOKENS, COMPLETE],
        );
        assert_eq!(codex_turn_open(&closed), Some(false));

        // Codex writes its thread settings after the turn closes — a line
        // after the edge, not a new turn.
        let settled = rollout_at(
            dir.path(),
            "settled.jsonl",
            &[META, CONTEXT, STARTED, ITEM, COMPLETE, SETTINGS],
        );
        assert_eq!(codex_turn_open(&settled), Some(false));

        let aborted = rollout_at(
            dir.path(),
            "aborted.jsonl",
            &[META, CONTEXT, STARTED, CALL, ITEM, ABORTED],
        );
        assert_eq!(
            codex_turn_open(&aborted),
            Some(false),
            "a person's Esc ends the turn; a restart must not undo it"
        );

        // The newest edge decides: a second turn cut after a finished first.
        let again = rollout_at(
            dir.path(),
            "again.jsonl",
            &[
                META, CONTEXT, STARTED, ITEM, COMPLETE, CONTEXT, STARTED, CALL,
            ],
        );
        assert_eq!(codex_turn_open(&again), Some(true));

        // A session in which no turn ever began, and a file that is not a
        // rollout at all: nothing to continue.
        let fresh = rollout_at(dir.path(), "fresh.jsonl", &[META, CONTEXT]);
        assert_eq!(codex_turn_open(&fresh), Some(false));
        let claude = rollout_at(
            dir.path(),
            "claude.jsonl",
            &[r#"{"type":"assistant","message":{"role":"assistant","content":"hi"}}"#],
        );
        assert_eq!(codex_turn_open(&claude), Some(false));

        // No file, or an empty one: no witness, so the caller keeps its word.
        assert_eq!(codex_turn_open(&dir.path().join("missing.jsonl")), None);
        let empty = dir.path().join("empty.jsonl");
        std::fs::write(&empty, "").expect("write");
        assert_eq!(codex_turn_open(&empty), None);
    }

    /// A tool output quoting the edge words is an item, not an edge — the
    /// record's `type` fields decide, never the bytes.
    #[test]
    fn a_tool_output_that_quotes_the_edge_words_is_not_an_edge() {
        use rollout::*;
        let dir = tempfile::tempdir().expect("tempdir");
        let quoted = rollout_at(dir.path(), "quoted.jsonl", &[META, STARTED, OUTPUT]);
        assert_eq!(codex_turn_open(&quoted), Some(true));
        let spoofed = rollout_at(
            dir.path(),
            "spoofed.jsonl",
            &[
                META,
                STARTED,
                COMPLETE,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}"}]}}"#,
            ],
        );
        assert_eq!(codex_turn_open(&spoofed), Some(false));
    }

    /// A turn longer than the tail window has no edge in view, and is still
    /// a turn: a finished one would have closed within a few KiB of its last
    /// words. Only a WHOLE file with no edge is a session that never began.
    #[test]
    fn a_turn_longer_than_the_tail_window_is_still_open() {
        use rollout::*;
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("long.jsonl");
        let mut writing = std::fs::File::create(&file).expect("create");
        writeln!(writing, "{META}\n{CONTEXT}\n{STARTED}").expect("head");
        let filler = format!(
            r#"{{"timestamp":"2026-09-05T05:30:00.000Z","type":"response_item","payload":{{"type":"reasoning","encrypted_content":"{}"}}}}"#,
            "x".repeat(1024)
        );
        for _ in 0..((MAX_TAIL_BYTES / 1024) + 32) {
            writeln!(writing, "{filler}").expect("fill");
        }
        writeln!(writing, "{ITEM}").expect("tail");
        drop(writing);
        assert_eq!(codex_turn_open(&file), Some(true));

        // The same stretch, closed at the end: the edge is in the window.
        let mut writing = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .expect("append");
        writeln!(writing, "{COMPLETE}\n{SETTINGS}").expect("close");
        drop(writing);
        assert_eq!(codex_turn_open(&file), Some(false));
    }
}
