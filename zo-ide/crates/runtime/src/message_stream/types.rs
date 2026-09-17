//! Provider-neutral block types that flow from `message_stream/` into
//! the TUI layer.
//!
//! These are a verbatim implementation of the contract defined in
//! `.zo/contracts/render_block.rs`. See `code-rules.md` R1: nothing
//! below the `tui/` boundary may reference Anthropic- or `OpenAI`-specific
//! wire types. All adapters (`anthropic/`, and in the future `codex/`)
//! translate their native events into these variants.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use core_types::{CardModel, RateLimitSnapshot};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::usage::TokenUsage;

// ============================================================================
// Identity
// ============================================================================

/// Stable identifier for a single rendered block.
///
/// Streaming deltas that belong to the same logical block (e.g. the
/// text deltas that make up one assistant paragraph) share a
/// [`BlockId`] so the transcript updates in place rather than appending
/// duplicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockId(pub u64);

/// Provider-neutral identifier for a single tool invocation.
///
/// Used to correlate a [`RenderBlock::ToolCall`] with its eventual
/// [`RenderBlock::ToolResult`]. Each adapter mints these from its own
/// native id (Anthropic's `toolu_01ABC…`, `OpenAI`'s `call_abc123`, …).
/// The TUI never inspects the contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

// ============================================================================
// Active model
// ============================================================================

/// Provider-aware handle to the currently active model.
///
/// Read by the HUD's model field and written by the `/model` switcher.
/// The TUI never looks up provider-specific configuration; it only
/// reads `display_name`, `context_limit`, and (optionally) the
/// `provider` tag for badge styling.
#[derive(Debug, Clone)]
pub struct ActiveModel {
    /// Stable provider id, e.g. `"anthropic"`, `"codex"`. Matches
    /// [`crate::message_stream::ProviderStream::id`].
    pub provider: &'static str,
    /// Provider-specific alias or model id, e.g. `"opus"`, `"sonnet"`,
    /// or `"openai-latest"`.
    pub alias: String,
    /// Human-readable name shown in the HUD.
    pub display_name: String,
    /// Context window size in tokens.
    pub context_limit: u32,
}

// ============================================================================
// Top-level enum
// ============================================================================

/// The single currency that flows from `message_stream/` into `tui/`.
///
/// See `code-rules.md` R1: nothing else may cross that boundary.
#[derive(Debug)]
pub enum RenderBlock {
    /// A streaming text delta from the assistant. Multiple deltas with
    /// the same `id` are concatenated by the TUI transcript.
    TextDelta {
        /// Stable id of the logical text block.
        id: BlockId,
        /// Text chunk appended since the previous delta.
        text: String,
        /// Whether the block has finished streaming.
        done: bool,
    },

    /// A streaming "reasoning" delta — the model's chain-of-thought
    /// surface, rendered dimmed.
    ///
    /// Anthropic surfaces this as `thinking_delta`; `OpenAI` surfaces it
    /// as reasoning summaries on the Responses API. Both adapters
    /// translate into this neutral variant. The legacy Anthropic code
    /// at `main.rs:~2032` currently drops these; surfacing them here
    /// fulfils `code-rules.md` R6.
    Reasoning {
        /// Stable id of the logical reasoning block.
        id: BlockId,
        /// Reasoning chunk appended since the previous delta.
        text: String,
        /// Optional opaque signature carried alongside reasoning.
        ///
        /// Anthropic's `signature_delta` arrives after the final
        /// `thinking_delta` for a block. The adapter preserves it on
        /// the final (`done == true`) emission; the TUI treats it as
        /// metadata only.
        signature: Option<String>,
        /// Whether the block has finished streaming.
        done: bool,
    },

    /// A tool call has started. The TUI shows a card with a spinner.
    ToolCall {
        /// Stable id of the card block.
        id: BlockId,
        /// Provider-neutral tool call id.
        tool_call_id: ToolCallId,
        /// Canonical tool name (e.g. `bash`, `read_file`).
        name: String,
        /// One-line summary of the tool input for the collapsed header.
        summary: String,
        /// Structured preview of the tool input.
        preview: ToolPreview,
        /// Lifecycle status of the call.
        status: ToolCallStatus,
    },

    /// A tool call has produced its result. Carries the same
    /// `tool_call_id` as the originating [`RenderBlock::ToolCall`] so
    /// the transcript can collapse them into a single card.
    ToolResult {
        /// Stable id of the result block.
        id: BlockId,
        /// Provider-neutral tool call id.
        tool_call_id: ToolCallId,
        /// Whether the tool returned an error.
        is_error: bool,
        /// Structured tool result body.
        body: ToolResultBody,
    },

    /// A permission decision is required from the user.
    ///
    /// The agent task constructs this with a fresh [`oneshot`] channel,
    /// sends the [`RenderBlock::PermissionPrompt`] into the TUI, and
    /// awaits the receiver. This is the only variant that is *not*
    /// `Clone` and *not* serializable.
    PermissionPrompt(PermissionPrompt),

    /// A tool is asking the user a blocking question.
    ///
    /// The TUI owns presentation and keyboard input. Tool code sends
    /// this block through a user-question channel and waits on the
    /// responder instead of writing directly to stdin/stdout.
    UserQuestionPrompt(UserQuestionPrompt),

    /// An inline image attachment (e.g. pasted from the clipboard).
    ///
    /// The TUI renders this via terminal image protocols (iTerm2 /
    /// Kitty) when available, falling back to a styled badge.
    Image {
        /// Stable id of the image block.
        id: BlockId,
        /// Raw image bytes (PNG, JPEG, etc.).
        data: Vec<u8>,
        /// MIME type, e.g. `"image/png"`.
        media_type: String,
    },

    /// A user-submitted message shown in the transcript.
    UserMessage {
        /// Stable id for this user message block.
        id: BlockId,
        /// The user's prompt text.
        text: String,
    },

    /// A verbatim message the model pushed **to the user** mid-run via the
    /// `send_to_user` tool, shown without ending the turn.
    ///
    /// Distinct from [`RenderBlock::System`] notices: this is content the model
    /// wants the user to read *now* (findings, a diff, a URL), so the TUI frames
    /// it in a dedicated "to you" panel rather than a muted status line. Like
    /// [`RenderBlock::Card`] and folded user messages it has no live payload, so
    /// on the wire it projects to a `System` notice (see the CLI's
    /// `SerializableRenderBlock::from_block`).
    UserNotice {
        /// Stable id of the notice block.
        id: BlockId,
        /// Verbatim message text shown to the user.
        message: String,
    },

    /// A notification the model pushed toward the person with
    /// `PushNotification`, after its surface judged the road. The TUI shows
    /// one status line and, on the terminal road, rings the tty; on the wire
    /// it is the `notify` frame the `ZeroCode` window turns into an OS
    /// notification. Only delivered roads are ever sent — a skip is a receipt
    /// to the model, not a line for the person.
    Notification {
        /// Stable id of the notification block.
        id: BlockId,
        /// Which zo is calling, for a terminal's notification title.
        title: String,
        /// The one-line body the model wrote.
        body: String,
        /// Severity tint; `Info` today.
        level: SystemLevel,
        /// The road the surface took.
        road: NotificationRoad,
    },

    /// A "system" line — slash command output, banners, dividers,
    /// error notices. Always rendered muted.
    System {
        /// Stable id of the system block.
        id: BlockId,
        /// Severity level that drives tint.
        level: SystemLevel,
        /// Display text (no ANSI).
        text: String,
    },

    /// A structured command-output **card** — the rich form of slash
    /// command output (`/status`, `/cost`, `/context`, …). The TUI
    /// renders [`CardModel`] into a bordered, gauged panel; headless
    /// sinks fall back to [`CardModel::plain_text`].
    Card {
        /// Stable id of the card block.
        id: BlockId,
        /// Structured card content.
        card: CardModel,
    },

    /// A finished **background sub-agent's result**, landing in the transcript
    /// as its own author identity rather than an amber `You` user message.
    ///
    /// A re-injected agent completion is a user-role turn *under the hood* (the
    /// main model must read `body` to continue), but its visual author is the
    /// agent, not the user. The TUI renders this as a bordered, collapsible
    /// "agent result" card (label + status + line count, body one keystroke
    /// away); headless / attach sinks fall back to the plain `body` text. This
    /// keeps a 200-line agent dump from flooding the transcript as raw markdown
    /// while preserving the exact text the model receives.
    AgentResult {
        /// Stable id of the card block.
        id: BlockId,
        /// Sub-agent display label (e.g. `runtime-scout`).
        label: String,
        /// Completion status driving the header glyph / tint.
        status: AgentResultStatus,
        /// What the run cost — `12 tool uses · 45.3k tokens · 1m 20s` — for the
        /// card's `Done (…)` line. `None` when nothing was measured, so the
        /// header stays exactly as it reads without one.
        summary: Option<String>,
        /// The agent's raw result markdown, shown when expanded.
        body: String,
    },

    /// Live progress of an in-flight compaction summary: cumulative streamed
    /// chars of the (internal, never-rendered) summary response. Carries no
    /// `BlockId` — like [`RenderBlock::Usage`] it updates the live spinner,
    /// never the transcript, so a multi-minute summary shows movement instead
    /// of a frozen "Compacting conversation…" line.
    CompactionProgress {
        /// Total summary chars streamed so far.
        streamed_chars: u64,
    },

    /// A mid-turn usage snapshot, emitted once per agent iteration so the HUD
    /// reflects *real* token / cost figures instead of a char-count estimate.
    /// Carries no `BlockId` — it updates the live ledger, never the transcript.
    Usage {
        /// Absolute estimated context tokens after the latest model response.
        ctx_tokens: u64,
        /// Session-cumulative token usage (input / output / cache), priced
        /// TUI-side with the active model's [`crate::usage::ModelPricing`].
        /// Drives the *cost* figure, which is inherently a session total.
        cumulative: TokenUsage,
        /// The latest single request's usage. Drives the `ctx` breakdown
        /// (`new` vs `cached`) so that split describes the *current* context
        /// window — `current.context_tokens()` matches `ctx_tokens`, keeping
        /// the two lines on the same scale. Using `cumulative` here would show
        /// session-summed cache reads (millions) under a per-request ctx line.
        current: TokenUsage,
    },

    /// A unified rate-limit snapshot from response headers, emitted once at
    /// the start of a streamed turn so the HUD can show 5h/7d usage gauges.
    /// Like [`RenderBlock::Usage`] it carries no `BlockId` — it updates the
    /// live ledger only, never the transcript.
    RateLimit(RateLimitSnapshot),

    /// Where the model request stands, for the live status line: it left and
    /// nothing has come back yet, or a retryable failure is being waited out.
    /// Eight minutes of "Working" with nothing behind it read as a freeze
    /// (2026-09-04, a gpt-5.6 turn whose first token never came before the
    /// backend dropped the stream); this names the wait. Live ledger only,
    /// never the transcript — the transcript already carries the retry
    /// notice as a `System` row.
    StreamPhase(StreamPhase),

    /// Which model this request goes out on, and why (see [`WireModel`]).
    /// Emitted once per request dispatch, before the stream opens. Live
    /// ledger only — the footer's model field — never a transcript row: each
    /// swap's explanation is already a `System` row.
    WireModel(WireModel),
}

/// The request's standing, as the streaming loop knows it. The first content
/// block that follows a `RequestSent` ends the wait; the TUI reads that from
/// the blocks themselves rather than from a third variant, so a provider that
/// starts with a `Usage` or `RateLimit` block is still "waiting" until text,
/// reasoning or a tool call arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPhase {
    /// The request left (`attempt` is 1-based); no provider event yet.
    RequestSent { attempt: u32 },
    /// A retryable failure; the next attempt starts after `delay_secs`.
    Retrying { attempt: u32, delay_secs: u64 },
    /// The stream is alive but the model has produced nothing visible for
    /// `since_secs` — reasoning silently. A phrase for the status line that
    /// content ends, never a transcript row (2026-09-07: one long turn left
    /// five such rows behind).
    QuietReasoning { since_secs: u64 },
}

/// The model a request is about to run on, and why it is — or is not — the
/// session's configured model. A Fable safety-classifier refusal retried on
/// the Opus head, a quota fallback on another provider, a deep-gate leg on its
/// own model: each is a `System` row in the transcript already, but a footer
/// that keeps saying the configured model over a reply another model wrote is
/// not telling the truth. This is the fact it keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireModel {
    /// The model id the request goes out on.
    pub model: String,
    /// The decision that put that model on the wire.
    pub source: WireModelSource,
}

/// Why a request runs on the model it runs on — the runtime's resolution
/// order, most specific first (`ConversationRuntime::wire_model`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireModelSource {
    /// The session's configured model; nothing swapped.
    Session,
    /// A Fable/Mythos safety-classifier refusal retried this turn on the Opus
    /// family head.
    RefusalFallback,
    /// Consecutive refusals parked the session on the Opus family head for a
    /// cooldown; every turn inside it starts there.
    RefusalCooldown,
    /// The main model's quota window is exhausted; the turn continues on the
    /// cross-provider fallback.
    QuotaFallback,
    /// The provider shed the heavier tier; the turn continues on a lighter tier
    /// of the same provider.
    OverloadDemotion,
    /// The confidence cascade escalated this turn to a stronger model.
    Escalation,
    /// A deep-gate PLAN leg on its planner model.
    DeepPlan,
    /// A deep-gate VERIFY leg on its verifier model.
    DeepVerify,
    /// An Architect EXEC leg on the implementer model.
    ExecImplementer,
}

impl WireModelSource {
    /// Every source, in resolution order — the one table [`Self::as_tag`] and
    /// [`Self::from_tag`] both walk.
    pub const ALL: [Self; 9] = [
        Self::Session,
        Self::RefusalFallback,
        Self::RefusalCooldown,
        Self::QuotaFallback,
        Self::OverloadDemotion,
        Self::Escalation,
        Self::DeepPlan,
        Self::DeepVerify,
        Self::ExecImplementer,
    ];

    /// Stable `snake_case` wire tag for JSON sinks; [`Self::from_tag`] inverts it.
    #[must_use]
    pub const fn as_tag(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::RefusalFallback => "refusal_fallback",
            Self::RefusalCooldown => "refusal_cooldown",
            Self::QuotaFallback => "quota_fallback",
            Self::OverloadDemotion => "overload_demotion",
            Self::Escalation => "escalation",
            Self::DeepPlan => "deep_plan",
            Self::DeepVerify => "deep_verify",
            Self::ExecImplementer => "exec_implementer",
        }
    }

    /// The source a wire tag names, or `None` for an unknown tag.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_tag() == tag)
    }
}

// ============================================================================
// Tool call subtypes
// ============================================================================

/// Lifecycle status of a tool call.
///
/// Drives the spinner / checkmark / X icon in the card header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    /// Argument JSON still streaming via provider input-delta events.
    Pending,
    /// Argument JSON complete, runtime is executing the tool.
    Running,
    /// Tool returned successfully.
    Ok,
    /// Tool returned an error.
    Errored,
    /// User cancelled (Ctrl-C) before the tool returned.
    Cancelled,
}

/// How long a successful tool call belongs in the normal visible history.
///
/// The existing transcript call and result remain available; this only
/// controls whether a successful internal preparation occupies a completed
/// tool cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPresentation {
    /// Keep the completed call in normal history.
    Persistent,
    /// Show the running call, then remove it from normal history on success.
    TransientOnSuccess,
}

/// Structured preview of a tool *input*.
///
/// Variants mirror the 15 ad-hoc formatters at `main.rs:2252–2622`.
/// Each variant carries the typed data needed by the corresponding
/// `tui/blocks/tool_call.rs` renderer — never pre-formatted ANSI
/// strings (see `code-rules.md` R4).
#[derive(Debug, Clone)]
pub enum ToolPreview {
    /// `bash`, `shell`, etc.
    Bash {
        /// Command line as submitted by the model.
        command: String,
    },
    /// `read_file`, `view`, etc.
    Read {
        /// Absolute or repo-relative path the model asked to read.
        path: String,
        /// Optional `(start, end)` 1-based line range.
        range: Option<(u64, u64)>,
    },
    /// `write_file`, `create_file`, etc.
    Write {
        /// Destination path.
        path: String,
        /// Byte length of the payload.
        byte_count: usize,
    },
    /// `edit_file`, `str_replace`.
    Edit {
        /// File being edited.
        path: String,
        /// Number of hunks in the edit.
        hunk_count: usize,
    },
    /// `glob`.
    Glob {
        /// Glob pattern submitted by the model.
        pattern: String,
    },
    /// `grep`, `ripgrep`.
    Grep {
        /// Regex pattern submitted by the model.
        pattern: String,
        /// Optional scoping path.
        path: Option<String>,
    },
    /// `web_search`, `web_fetch`.
    Search {
        /// Query string submitted by the model.
        query: String,
    },
    /// MCP-provided tool we have no special handler for.
    Generic {
        /// Canonical name of the tool.
        name: String,
        /// Best-effort one-line summary of the input.
        input_summary: String,
    },
}

impl ToolPreview {
    /// Return the normal-history lifetime for this typed preview.
    #[must_use]
    pub fn presentation(&self) -> ToolPresentation {
        match self {
            Self::Generic { name, .. }
                if matches!(name.as_str(), "ToolSearch" | "Skill" | "TaskOutput") =>
            {
                ToolPresentation::TransientOnSuccess
            }
            _ => ToolPresentation::Persistent,
        }
    }
}

/// Structured tool *result* body.
///
/// Same spirit as [`ToolPreview`]: typed, provider-neutral, no ANSI.
#[derive(Debug, Clone)]
pub enum ToolResultBody {
    /// Generic textual output (with optional truncation marker).
    Text {
        /// Raw textual content.
        content: String,
        /// `true` if the adapter truncated the output for display.
        truncated: bool,
    },
    /// `bash` exit code plus split stdout / stderr.
    Bash(BashResult),
    /// File read result with optional language hint.
    Read {
        /// Path that was read.
        path: String,
        /// File contents.
        content: String,
        /// Best-effort language identifier for syntax highlighting.
        language: Option<String>,
        /// `true` if the adapter truncated the output for display.
        truncated: bool,
    },
    /// Patch / unified diff for a file edit.
    Diff(DiffView),
    /// `glob` / `grep` listing.
    Listing {
        /// File paths or match lines returned by the tool.
        entries: Vec<String>,
        /// `true` if the adapter truncated the listing.
        truncated: bool,
    },
    /// `TodoWrite` / `TaskList` checklist snapshot, rendered Claude-Code-style
    /// as a titled block of checkbox rows instead of raw (truncated) JSON.
    Todos(Vec<TodoResultItem>),
    /// MCP tool result we have no special handler for.
    Generic {
        /// Canonical name of the tool.
        name: String,
        /// Raw textual content.
        content: String,
        /// `true` if the adapter truncated the output.
        truncated: bool,
    },
    /// The dispatch policy declined the call — a small delegated task kept
    /// inline, an implementation model reserved for orchestration. `reason`
    /// is the instruction the model reads; the screen heads it as a verdict,
    /// not a failure.
    Declined {
        /// What to do instead, in the policy's words.
        reason: String,
    },
}

/// One row of a [`ToolResultBody::Todos`] checklist. Mirrors the writer's
/// persisted `{content, activeForm, status}` shape (`tools::task_tools`), kept
/// provider-neutral so the TUI never parses tool JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoResultItem {
    /// Imperative form shown when the item is pending or completed.
    pub content: String,
    /// Present-progressive form shown while the item is in progress.
    pub active_form: String,
    /// Lifecycle state of this item.
    pub status: TodoResultStatus,
}

/// Lifecycle state of a [`TodoResultItem`] — same three states as the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoResultStatus {
    /// Not started yet.
    Pending,
    /// Actively being worked on.
    InProgress,
    /// Finished.
    Completed,
}

// ============================================================================
// Bash result
// ============================================================================

/// Output of a bash / shell tool.
///
/// Renders as `tui/blocks/bash_result.rs` with an exit-code badge and
/// split stdout / stderr panes.
#[derive(Debug, Clone)]
pub struct BashResult {
    /// Process exit code (`0` on success).
    pub exit_code: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// `true` if the adapter truncated either stream.
    pub truncated: bool,
}

// ============================================================================
// Diff view
// ============================================================================

/// Structured unified diff.
///
/// Replaces the ad-hoc string output of `format_patch_preview` and
/// `format_structured_patch_preview` in `main.rs`.
#[derive(Debug, Clone)]
pub struct DiffView {
    /// Original path (`None` for new files).
    pub old_path: Option<String>,
    /// New path (`None` for deletions).
    pub new_path: Option<String>,
    /// Best-effort language identifier (e.g. `"rust"`).
    pub language: Option<String>,
    /// Hunks in file order.
    pub hunks: Vec<DiffHunk>,
}

/// A single hunk of a [`DiffView`].
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// 1-based starting line in the old file.
    pub old_start: u32,
    /// Number of lines the hunk covers in the old file.
    pub old_lines: u32,
    /// 1-based starting line in the new file.
    pub new_start: u32,
    /// Number of lines the hunk covers in the new file.
    pub new_lines: u32,
    /// Lines comprising the hunk.
    pub lines: Vec<DiffLine>,
}

/// A single line within a [`DiffHunk`].
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// Whether the line was added, removed, or unchanged.
    pub kind: DiffLineKind,
    /// Raw line text (no leading `+`/`-`/` ` marker).
    pub text: String,
}

/// Classification of a [`DiffLine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// Unchanged context line.
    Context,
    /// Line added in the new file.
    Added,
    /// Line removed from the old file.
    Removed,
}

// ============================================================================
// Permission prompt
// ============================================================================

/// A permission decision request.
///
/// See `.zo/contracts/render_block.rs` for the full lifecycle. This
/// is the only [`RenderBlock`] variant that is *not* `Clone` and *not*
/// serializable because it carries a live [`oneshot::Sender`] back to
/// the agent task.
#[derive(Debug)]
pub struct PermissionPrompt {
    /// Stable id of the modal block.
    pub id: BlockId,
    /// Tool call the prompt is gating.
    pub tool_call_id: ToolCallId,
    /// Canonical tool name, for display.
    pub tool_name: String,
    /// Human-readable justification shown to the user.
    pub reasoning: String,
    /// Short audit line explaining risk and the explicit unblock action.
    pub audit_hint: Option<String>,
    /// The available choices, in display order.
    pub choices: Vec<PermissionChoice>,
    /// Used by the TUI to resolve the user's decision.
    pub responder: oneshot::Sender<PermissionDecision>,
}

/// A single choice on a [`PermissionPrompt`].
#[derive(Debug, Clone)]
pub struct PermissionChoice {
    /// Single keyboard key (e.g. `'y'`, `'n'`, `'a'`).
    pub key: char,
    /// Human-readable label (e.g. `"Allow once"`).
    pub label: String,
    /// Decision this choice resolves to.
    pub decision: PermissionDecision,
}

/// Resolution of a [`PermissionPrompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow this invocation only.
    AllowOnce,
    /// Allow and remember for this session.
    AllowAlways,
    /// Deny this invocation only.
    Deny,
    /// Deny and remember for this session.
    DenyAlways,
}

/// One selectable choice of a [`UserQuestionPrompt`].
///
/// Deserializes from either a bare string (`"OAuth"`) or an object
/// (`{"label": "OAuth", "description": "browser login"}`), so the model can
/// send the legacy flat form or the rich form interchangeably.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestionOption {
    /// Short answer label — what is returned when the option is picked.
    pub label: String,
    /// Optional one-line explanation rendered dim under the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional monospace preview of the concrete artifact this option produces
    /// — an ASCII layout mockup, a code snippet, a config sample. When any
    /// option carries one, the modal switches to a side-by-side layout with the
    /// option list on the left and the focused option's preview on the right,
    /// so alternatives can be compared by looking rather than by reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

impl QuestionOption {
    /// A plain option with no description.
    #[must_use]
    pub fn plain(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
            preview: None,
        }
    }

    /// Attach a monospace preview block to this option.
    #[must_use]
    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }
}

impl<'de> Deserialize<'de> for QuestionOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Plain(String),
            Rich {
                label: String,
                #[serde(default)]
                description: Option<String>,
                #[serde(default)]
                preview: Option<String>,
            },
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Plain(label) => Self {
                label,
                description: None,
                preview: None,
            },
            Raw::Rich {
                label,
                description,
                preview,
            } => Self {
                label,
                description,
                // An empty preview is the same as none: it would otherwise flip
                // the whole modal into the side-by-side layout to show nothing.
                preview: preview.filter(|value| !value.trim().is_empty()),
            },
        })
    }
}

/// Blocking question prompt emitted by `AskUserQuestion`.
///
/// `options` is empty for a free-form text answer. Otherwise the TUI presents
/// a choice list (plus an always-available free-form row). When `multi_select`
/// is `false` (the default) it is a single-select radio and the responder
/// resolves to exactly one value; when `true` it renders `[x]`/`[ ]`
/// checkboxes and the responder carries every checked label (plus any typed
/// free-form text).
#[derive(Debug)]
pub struct UserQuestionPrompt {
    /// Stable id of the modal block.
    pub id: BlockId,
    /// Human-readable question text.
    pub question: String,
    /// Short topic chip (e.g. `Auth method`) shown beside the modal title.
    pub header: Option<String>,
    /// Optional fixed choices in display order.
    pub options: Vec<QuestionOption>,
    /// When `true`, the user may check several options; single-select (the
    /// default) resolves to exactly one value.
    pub multi_select: bool,
    /// Used by the TUI to resolve the user's answer(s). One element for a
    /// single-select prompt; one per checked option (plus any free-form text)
    /// for a multi-select prompt.
    pub responder: oneshot::Sender<Vec<String>>,
}

// ============================================================================
// System messages
// ============================================================================

/// Completion status of a re-injected [`RenderBlock::AgentResult`].
///
/// Drives the header glyph (`✓` / `✕`) and tint. Deliberately coarse — the card
/// only distinguishes "the agent produced a usable result" from "it did not";
/// richer per-status wording stays in the `System` notice line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentResultStatus {
    /// The sub-agent finished and returned a result.
    Completed,
    /// The sub-agent failed, was stopped, or gave up.
    Failed,
}

/// Severity tint for [`RenderBlock::System`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLevel {
    /// Slash command echo, `/clear` divider, banners.
    Info,
    /// A completed action worth confirming (login, connect). Distinct from
    /// [`Self::Info`] so a success reads as one at a glance instead of blending
    /// into the dim command echo above it.
    Success,
    /// Cost / token budget warnings.
    Warn,
    /// Stream pipeline errors surfaced to the user.
    Error,
    /// The runtime's own housekeeping — a context trim, the heads-up that
    /// auto-compaction is near. A fact about the session's plumbing, not
    /// about the work: an attended surface shows it beside its status line
    /// for a moment and keeps it out of the transcript (codex leaves no
    /// housekeeping line in history; the footer's context figure is the
    /// standing word). A real compaction is not housekeeping — it changes
    /// what the model can see, and stays a transcript line (t-3177).
    Housekeeping,
}

/// Which road a `PushNotification` took — the word its receipt says and the
/// `notify` frame carries, spelled once here because the block that carries
/// it (`RenderBlock::Notification`) and the tool that judges it (`tools`)
/// both depend on this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationRoad {
    /// Handed to the `ZeroCode` window over the events channel.
    Window,
    /// Rung in the bare terminal: BEL plus OSC 9 / OSC 777.
    Terminal,
    /// A person pressed a key at this zo recently; nothing was sent.
    SkippedAttended,
    /// No surface can carry a notification here; nothing was sent.
    SkippedNowhere,
}

impl NotificationRoad {
    /// The receipt's and the frame's word for this road.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Terminal => "terminal",
            Self::SkippedAttended => "skipped: attended",
            Self::SkippedNowhere => "skipped: nowhere",
        }
    }

    /// Whether something actually went out to the person.
    #[must_use]
    pub const fn delivered(self) -> bool {
        matches!(self, Self::Window | Self::Terminal)
    }

    /// The road a wire word names, for a consumer on the other end.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [
            Self::Window,
            Self::Terminal,
            Self::SkippedAttended,
            Self::SkippedNowhere,
        ]
        .into_iter()
        .find(|road| road.as_str() == word)
    }
}

// ============================================================================
// Id generator
// ============================================================================

/// Centralised id generator.
///
/// The agent task owns one of these and hands out fresh ids as it
/// pushes blocks. Wrapped in `Arc<AtomicU64>` so the parser and the
/// slash command handlers can share it.
#[derive(Debug, Clone, Default)]
pub struct BlockIdGen(pub Arc<AtomicU64>);

impl BlockIdGen {
    /// Allocate a fresh id.
    #[must_use]
    pub fn next(&self) -> BlockId {
        BlockId(self.0.fetch_add(1, Ordering::Relaxed))
    }
}
