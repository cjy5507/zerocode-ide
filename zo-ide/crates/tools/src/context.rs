//! `ToolContext` — shared registries threaded through tool dispatch.
//!
//! Everything a tool handler needs at runtime (LSP, MCP, task/team/cron
//! registries, the optional user-question channel, the active model) is
//! bundled in one struct so each `_tools.rs` submodule can take a
//! single `&ToolContext` parameter instead of an ever-growing list.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use zerocode_core::computer_use_protocol::frame::ShotFrame;

use crate::aliases::canonical_tool_name;
use crate::error::ToolError;
use crate::gateway::{AuditSummary, RouteDecisionRecord, ToolInvocation, summarize_invocations};
use crate::workspace_checkpoint::{
    WorkspaceCheckpoint, WorkspaceCheckpointStore, WorkspaceRestorePlan, WorkspaceRestoreSummary,
};

use runtime::{
    lsp_client::LspRegistry,
    task_registry::TaskRegistry,
    team_cron_registry::{CronRegistry, TeamRegistry},
    worker_boot::WorkerRegistry,
};

pub(crate) const TOOL_TOGGLE_DENIAL_REASON: &str = "tool disabled by .zo/tool-toggles.json";

/// Co-locate turn evidence behind the existing handle: `ToolContext` is carried
/// inside futures whose stack-size budget leaves no room for another handle.
#[derive(Default)]
struct ToolTurnEvidence {
    id: Option<String>,
    artifact_skill_reads: BTreeSet<String>,
}

/// Upper bound on each shadow audit ledger ([`ToolContext::tool_invocations`]
/// and [`ToolContext::route_decisions`]). The ledgers are append-only and the
/// `ToolContext` lives for the whole session, so without a bound a long-running
/// interactive session or `serve` daemon executing many thousands of tool calls
/// would grow them without limit (each entry is a metadata-heavy envelope).
///
/// The cap is deliberately generous: interactive sessions and ordinary workflow
/// runs stay far below it, so [`ToolContext::audit_summary`] reflects every call
/// in practice. Only a pathological long-lived session is windowed, in which
/// case the audit reflects the most recent `MAX_AUDIT_LEDGER_ENTRIES` calls —
/// the same recent-window tradeoff the task registry and LSP diagnostics use.
pub(crate) const MAX_AUDIT_LEDGER_ENTRIES: usize = 10_000;

/// Channel for delivering user questions from tool execution back to the UI.
///
/// Implementations live in the CLI (terminal I/O) or parent agent context (message pipe).
/// When no channel is available, `AskUserQuestion` falls back to direct stdin/stdout
/// only for interactive terminals; headless runs return an unanswered payload.
pub trait UserQuestionChannel: Send + Sync {
    /// Present a question to the user and return their response(s).
    ///
    /// `header` is an optional short topic chip; `options` is non-empty when
    /// the question offers a fixed set of choices (the user may still answer
    /// free-form, so the results are not guaranteed to be among the labels).
    /// When `multi_select` is `true` the user may pick several options, so the
    /// return carries one entry per selection; a single-select prompt returns
    /// exactly one entry.
    fn ask(
        &self,
        question: &str,
        header: Option<&str>,
        options: &[runtime::message_stream::QuestionOption],
        multi_select: bool,
    ) -> Result<Vec<String>, ToolError>;

    /// Push a verbatim message to the user mid-run (the `send_to_user` tool)
    /// without ending the turn or blocking for a reply.
    ///
    /// Unlike [`Self::ask`] this is fire-and-forget: it returns as soon as the
    /// notice is handed to the UI. The default errors so channels that only
    /// answer questions stay source-compatible; the TUI channel overrides it to
    /// emit a `UserNotice` render block. Callers treat an `Err` (or the absence
    /// of a channel entirely) as "no interactive surface" and echo the content
    /// inline instead.
    fn send_to_user(&self, _message: &str) -> Result<(), ToolError> {
        Err(ToolError::Execution(
            "this channel does not support send_to_user".to_string(),
        ))
    }

    /// Call the person's attention away from the screen (the
    /// `PushNotification` tool), and say which road it took.
    ///
    /// The surface owns the facts the judgement needs — whether a key was
    /// pressed recently, whether a window is subscribed, whether stdout is a
    /// terminal — so it judges (`push_notification::push_road`) and delivers.
    /// The default is honest for a channel with no surface: nothing was sent,
    /// nowhere could carry it. Callers treat `Err` the same way.
    fn push_notification(
        &self,
        _notice: &crate::misc_tools::push_notification::PushNotice,
    ) -> Result<crate::misc_tools::push_notification::PushRoad, ToolError> {
        Ok(crate::misc_tools::push_notification::PushRoad::SkippedNowhere)
    }
}

/// A single instrumentation probe recorded by `InstrumentLog` (debug mode).
///
/// `snippet` is the exact text inserted into `path` (a `/*ZO_PROBE:<id>*/`
/// marker plus the caller's statement, on its own line). Recording it verbatim
/// lets [`ToolContext::revert_probes`] strip it with a literal replace,
/// restoring the file byte-for-byte so debug instrumentation never leaks into
/// the final diff.
#[derive(Clone, Debug)]
pub struct Probe {
    pub path: PathBuf,
    pub snippet: String,
}

/// Verdict for a [`DebugHypothesis`] in debug mode.
///
/// A debugger sub-agent records a root-cause guess as `Open`, then flips it to
/// `Confirmed` or `Refuted` as evidence arrives. The wire form is the lowercase
/// variant name, matching the `DebugHypothesis` tool's input-schema enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HypothesisStatus {
    Open,
    Confirmed,
    Refuted,
}

impl HypothesisStatus {
    /// Lowercase label used both on the wire and in the rendered ledger.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Confirmed => "confirmed",
            Self::Refuted => "refuted",
        }
    }
}

/// One root-cause hypothesis tracked by `DebugHypothesis` (debug mode).
///
/// A debugger sub-agent records guesses with a stable `id` and updates their
/// `status`/`evidence` as it gathers proof. The whole ledger is rendered back
/// into the agent's context on every call (so its reasoning persists across
/// iterations) and mirrored to a scratch file for post-run inspection.
#[derive(Clone, Debug)]
pub struct DebugHypothesis {
    pub id: String,
    pub statement: String,
    pub status: HypothesisStatus,
    pub evidence: Option<String>,
}

/// Per-turn agent-delegation policy signals, computed from the USER's turn by
/// the host's SSOT classifiers and threaded to tool dispatch so the CC-style
/// same-model spawn guard can fold a wasteful simple-implementation `Agent`
/// spawn to inline. Set fresh at the start of every model-led turn (interactive
/// and headless); `None` on the shared cell means "unknown", and the guard
/// fails open (allows the spawn) — e.g. background / non-turn dispatch.
#[derive(Clone, Copy, Debug)]
pub struct TurnAgentPolicy {
    /// Whole-turn task complexity (`tools::assess_turn_complexity`).
    pub user_complexity: runtime::RouteTaskComplexity,
    /// Whole-turn orchestration shape (`tools::assess_turn_orchestration`).
    pub user_shape: runtime::RouteShapeKind,
    /// Distinct planned agent needs the router found for the turn (`0` → none).
    pub user_need_count: usize,
    /// The user EXPLICITLY requested a delegation shape in the turn text
    /// (`TurnOrchestrationHint::user_requested_delegation`). Honored as "the
    /// user asked to delegate", so the guard never folds it.
    pub user_requested_delegation: bool,
    /// The one model the user named in the turn text, canonical
    /// (`TurnOrchestrationHint::user_named_model`). A person pin: the spawn
    /// runs on it whatever `model` the assistant's call asked for.
    pub user_named_model: Option<&'static str>,
}

impl TurnAgentPolicy {
    /// `true` when the user's turn is a simple Solo ask with no delegation
    /// value and no explicit delegation request — the only case where folding a
    /// same-model simple-implementation spawn to inline is correct. A non-Solo
    /// natural shape, any planned agent need, or an explicit user delegation
    /// request means the turn genuinely warrants delegation, so the guard steps
    /// aside.
    #[must_use]
    pub fn user_turn_is_solo_simple(self) -> bool {
        !self.user_requested_delegation
            && matches!(
                self.user_complexity,
                runtime::RouteTaskComplexity::Trivial | runtime::RouteTaskComplexity::Small
            )
            && matches!(self.user_shape, runtime::RouteShapeKind::Solo)
            && self.user_need_count == 0
    }
}

/// Who a sub-agent tool call is coming from. Recorded on the sub-agent's
/// [`ToolContext`] at spawn so tools that address the PARENT conversation
/// (today: `SendMessage(to: "main")`) can both authorize the caller and
/// attribute the message without a process-global lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentIdentity {
    /// Durable agent id (`a…-…`), the address a reply is sent back to.
    pub agent_id: String,
    /// Display label shown to the parent model and the user; may be empty.
    pub name: String,
}

impl SubagentIdentity {
    /// Best display handle: the label when it has one, else the raw id — the
    /// same "label wins over name" rule the HUD and the agent tree use.
    #[must_use]
    pub fn display(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.agent_id
        } else {
            self.name.trim()
        }
    }
}

/// The conversation a `fork` sub-agent inherits (t-2875): where the owning
/// session's transcript lives on disk, and the system prompt its runtime shows
/// the model. The host that owns a session publishes it after every runtime
/// build ([`ToolContext::install_fork_source`]); the `Agent` tool reads it when
/// a call names `subagent_type: "fork"`. The transcript is current at every
/// tool boundary because the session appends each message before the tools
/// of a batch run — so a fork taken mid-turn sees the turn so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkSource {
    /// The session's persisted transcript.
    pub transcript: PathBuf,
    /// The system prompt as the runtime holds it — CLI overrides and model
    /// retargets included, which the stored sidecar beside the transcript
    /// does not carry.
    pub system_prompt: Vec<String>,
}

/// The target, coordinates and mark numbering of a staged Computer picture.
#[derive(Debug, PartialEq)]
pub(crate) struct ComputerObservation {
    pub app: Option<String>,
    pub window_id: Option<u64>,
    pub frame: ShotFrame,
    pub marked_look: Option<String>,
}

/// One model's desktop frame and observation, shared by context clones. Its
/// viewer keeps provider diffs separate from every other model's looks.
struct ComputerSight {
    frame: Mutex<Option<ShotFrame>>,
    /// A diff can omit a picture only when the last staged picture was of
    /// this target, frame and numbering, not merely a cached provider look.
    observation: Mutex<Option<ComputerObservation>>,
    /// The last marked look's id: a click by a mark's number names it.
    marked_look: Mutex<Option<String>>,
    viewer: String,
}

impl ComputerSight {
    /// A viewer name no other context in this process holds.
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { frame: Mutex::new(None), observation: Mutex::new(None), marked_look: Mutex::new(None), viewer: format!("zo-{}-{n}", std::process::id()) }
    }
}

/// Shared registries constructed at startup and threaded through tool dispatch.
#[derive(Clone)]
pub struct ToolContext {
    /// The conversation a `fork` inherits, published by the session host
    /// (shared cell: every registry clone sees the latest build's value).
    fork_source: Arc<Mutex<Option<ForkSource>>>,
    pub lsp: LspRegistry,
    pub teams: TeamRegistry,
    pub crons: CronRegistry,
    pub tasks: TaskRegistry,
    inspection_shell: Arc<Mutex<bool>>,
    person_model_pin: Arc<Mutex<Option<String>>>,
    /// What the `Computer` tool keeps for its model: one cell, so the
    /// context — held inline by the TUI's largest futures — grows by a
    /// pointer, not by each thing kept.
    computer: Arc<ComputerSight>,
    pub workers: WorkerRegistry,
    pub hook_config: runtime::RuntimeHookConfig,
    /// Interactive user-question / mid-run push channel (`AskUserQuestion`,
    /// `send_to_user`).
    ///
    /// `Arc<Mutex<…>>` (like [`Self::active_model`]) so the TUI's per-turn
    /// install propagates to **every** registry clone — notably the
    /// concurrent-dispatch closure cloned at session boot that actually serves
    /// live tool calls (`runtime_support.rs`). As a plain `Option` the install
    /// only reached the executor's clone, so `send_to_user` never saw a channel
    /// and degraded to its inline echo even in a live TUI.
    user_question_channel: Arc<Mutex<Option<Arc<dyn UserQuestionChannel>>>>,
    /// Active parent model selected by the foreground runtime.
    ///
    /// Background `Agent` / `SpawnMultiAgent` calls inherit this when
    /// no per-agent model or `ZO_AGENT_MODEL` override is supplied.
    ///
    /// `Arc<Mutex<…>>` (like [`Self::disabled_tools`]) so a `/model` switch
    /// propagates to **every** registry clone — the api client, the executor,
    /// and the concurrent-dispatch closure that actually serves live tool calls
    /// (`runtime_support.rs`). A plain `Option<String>` would freeze each clone
    /// at the build-time model, so post-switch sub-agents would keep spawning on
    /// the startup model instead of the one the user just selected.
    pub active_model: Arc<Mutex<Option<String>>>,
    /// `true` when [`Self::active_model`] was chosen explicitly *for this
    /// agent* — i.e. a sub-agent spawned with a concrete `model` field rather
    /// than a Smart route (see `agent_tools::runtime::subagent_model_pinned`).
    /// Spawn-time smart routing consults this and inherits that model verbatim
    /// so an explicitly-modeled agent's own nested spawns stay in its context.
    ///
    /// The FOREGROUND session deliberately never sets this: a `/model` pick,
    /// `--model`, or a `/fast` flip pins the main turn only, and delegation
    /// keeps Smart-routing onto the connected inventory. Conflating the two
    /// meant one `/model opus` silently ran every Explore/verify/reviewer
    /// spawn on Opus for the rest of the session, with `/smart off` as the
    /// only (undiscoverable) escape. Opting delegation out is now explicit:
    /// `/smart off`, or `/smart pin <role|subagent> <model>`.
    pub active_model_pinned: Arc<Mutex<bool>>,
    /// Foreground session id that owns spawned sub-agent manifests.
    ///
    /// `.zo/agents` is workspace-global, so display surfaces must distinguish
    /// agents spawned by this chat from agents spawned by another model/session
    /// in the same directory. Shared across registry clones like the model
    /// fields so live runtime rebuilds and concurrent dispatch stay in sync.
    pub session_id: Arc<Mutex<Option<String>>>,
    /// The session-owned agent registry (t-2511): WHERE this session's helper
    /// manifests live, fixed once from the cwd the session was born in and
    /// never re-derived from the process cwd. Every spawn, stamp, stop, resume
    /// and roster scan takes this handle; `None` means no session installed
    /// one — unit tests, hermetic harnesses — and the documented cwd fallback
    /// ([`crate::AgentRegistry::unowned_from_cwd`]) applies.
    ///
    /// `Arc<Mutex<…>>` like [`Self::session_id`]: the host installs it once
    /// after the runtime is built and every registry clone — including the
    /// concurrent-dispatch closure that serves live spawns — sees the same
    /// handle. A sub-agent's fresh context inherits its parent's through
    /// [`Self::with_registry`].
    agent_registry: Arc<Mutex<Option<Arc<crate::AgentRegistry>>>>,
    /// Active foreground permission mode, used **only** to relax the file-tool
    /// workspace boundary (read/write/edit) for full-access sessions.
    ///
    /// The foreground `tool_registry` carries no `PermissionEnforcer` — tool
    /// *gating* is done at the runtime layer (`ConversationRuntime`'s
    /// `permission_policy` + interactive prompter), not in the registry. So
    /// `enforce_read_boundary`/`enforce_workspace_boundary` cannot learn the
    /// session mode from a registry enforcer (it is `None`): without this cell a
    /// danger-full-access user is wrongly denied an outside `read_file` with
    /// "escapes workspace boundary", even though `bash cat` / `read_image` reach
    /// the same path. The boundary relaxation now also consults this mode.
    ///
    /// `Arc<Mutex<…>>` (like [`Self::active_model`]) so a live Shift+Tab /
    /// `/permission` switch propagates to **every** registry clone — including
    /// the concurrent-dispatch closure that serves live `read_file` calls — with
    /// no runtime rebuild. `None` leaves the boundary keyed off the enforcer
    /// alone (tests, harness runs, sub-agents that carry an explicit enforcer).
    pub session_permission_mode: Arc<Mutex<Option<runtime::PermissionMode>>>,
    /// `true` when the user has explicitly selected Plan (Shift+Tab plan stop
    /// or `/plan on`). Plan enforces runtime [`runtime::PermissionMode::ReadOnly`],
    /// so `session_permission_mode` alone cannot tell user-selected Plan apart
    /// from plain read-only — this authoritative flag carries the distinction
    /// into the registry so the tool surface can be mode-driven, not
    /// prompt-inferred: while set, [`crate::registry::GlobalToolRegistry`] drops
    /// the write-gated `EnterPlanMode` / legacy `ExitPlanMode` from the wire
    /// advertisement and handles any stale in-flight call to them idempotently,
    /// instead of letting the generic `WorkspaceWrite` denial recur.
    ///
    /// `Arc<Mutex<…>>` (like [`Self::session_permission_mode`]) so a live
    /// Shift+Tab / `/plan` toggle propagates to **every** registry clone —
    /// including the concurrent-dispatch closure and the request builder — with
    /// no runtime rebuild. Set only by the user-driven seam
    /// (`LiveCli::set_plan_selected`); the model never toggles it and can never
    /// use it to restore write access.
    plan_selected: Arc<Mutex<bool>>,
    /// Canonical workspace root used to validate file-tool paths.
    ///
    /// `None` disables boundary enforcement (e.g. unit tests, harness
    /// runs). Wired by the CLI runtime builder to the user's cwd so
    /// `write_file` / `edit_file` cannot escape via `../` or symlinks
    /// when `PermissionMode::WorkspaceWrite` is active.
    pub workspace_root: Option<PathBuf>,
    /// Working directory for tool execution. `None` (the default) uses the
    /// live process cwd, preserving historical behavior for every tool.
    ///
    /// When set, `bash` runs in this directory and relative file-tool paths
    /// resolve against it instead of the process cwd. Per-agent execution
    /// wires it so `isolation:"worktree"` runs each sub-agent inside its own
    /// worktree without mutating the shared, process-global cwd.
    pub cwd: Option<PathBuf>,
    /// When this context runs a worktree-isolated sub-agent, the absolute
    /// worktree root its shell commands are confined to. `Some` marks the agent
    /// as isolated (mirrors the `cwd.is_some()` signal used elsewhere) and drives
    /// the git-redirection guard that refuses `git -C` / `--git-dir` / `GIT_DIR=`
    /// targets escaping the worktree into the shared checkout. `None` (the
    /// default) leaves shells unconfined.
    pub worktree_confinement: Option<PathBuf>,
    /// Logical owner id for the cross-process file write-lease (track 4-2).
    ///
    /// `None` (the solo default) disables write-lease coordination entirely, so
    /// single-agent / single-process workflows are unaffected. When set — a
    /// foreground session id, or a sub-agent id for a spawned agent — a
    /// `write_file` / `edit_file` first acquires a lease on the target path so a
    /// *second* concurrent agent (even in a separate `zo` process sharing the
    /// tree) cannot clobber an in-flight edit. Plain `String`, not a shared cell:
    /// it is fixed at context-build time per agent and never mutated live.
    pub lease_owner: Option<String>,
    /// Identity of the spawned sub-agent this context serves, or `None` in a
    /// main session. Plain `Option`, not a shared cell: it is fixed at
    /// context-build time per agent and never mutated live (same lifecycle as
    /// [`Self::lease_owner`]). See [`Self::with_subagent_identity`].
    subagent_identity: Option<SubagentIdentity>,
    /// Session-local disabled tool names loaded from `.zo/tool-toggles.json`
    /// and shared across registry clones. Names are stored canonicalized, so
    /// aliases like `web_search` disable the same handler as `WebSearch`.
    pub disabled_tools: Arc<Mutex<BTreeSet<String>>>,
    /// Phase-1 `ToolGateway` shadow ledger. Each dispatched tool appends one
    /// normalized invocation envelope with policy decision and result metadata.
    /// Shared across registry/context clones so the foreground session can audit
    /// calls made through cloned executors without changing tool outputs.
    tool_invocations: Arc<Mutex<Vec<ToolInvocation>>>,
    /// Active workflow run id, set by the engine at run start and cleared at the
    /// end. `record_tool_invocation` stamps it onto each invocation so the audit
    /// can join tool calls to the run's event stream (WI-C). `Arc<Mutex<…>>` so
    /// every cloned context observes the same active run.
    active_run_id: Arc<Mutex<Option<String>>>,
    /// Active foreground turn id, set by the turn loop at turn start. Stamped onto
    /// each invocation alongside `active_run_id`.
    turn_evidence: Arc<Mutex<ToolTurnEvidence>>,
    /// Per-turn agent-delegation policy ([`TurnAgentPolicy`]), set fresh at the
    /// start of every model-led turn (like [`Self::active_turn_id`]) and read by
    /// the `Agent` dispatch guard. `Arc<Mutex<…>>` so the value reaches every
    /// registry clone, including the concurrent-dispatch closure. `None` =
    /// unknown → the guard fails open. Overwritten each turn (never accumulates).
    turn_agent_policy: Arc<Mutex<Option<TurnAgentPolicy>>>,
    /// Structured route-decision ledger (WI-C), shared across clones like
    /// `tool_invocations`. The turn controller records one per routing decision,
    /// replacing the prior `ZO_ROUTE_DEBUG` eprintln; `audit_summary` folds it in.
    route_decisions: Arc<Mutex<Vec<RouteDecisionRecord>>>,
    /// Out-of-band image staging for multimodal tool results. `read_image`
    /// pushes `(media_type, base64)` here; the conversation loop drains it right
    /// after the tool runs (via `ToolExecutor::take_pending_images`) and attaches
    /// the images to that tool's result. Shared (`Arc`) so a cloned context drains
    /// the same sink; single-threaded in practice — image tools stay off the
    /// concurrency-safe path.
    pub image_sink: Arc<Mutex<Vec<(String, String)>>>,
    /// Out-of-band ledger of instrumentation probes staged by `InstrumentLog`
    /// (debug mode). Each entry records the file and the exact snippet inserted;
    /// a debugger sub-agent's run drains this via [`Self::revert_probes`] at
    /// completion so markers never survive into the working tree. Shared
    /// (`Arc`) so a cloned context drains the same ledger; single-threaded in
    /// practice — `InstrumentLog` stays off the concurrency-safe path.
    pub probe_sink: Arc<Mutex<Vec<Probe>>>,
    /// Out-of-band ledger of debugging hypotheses recorded by `DebugHypothesis`
    /// (debug mode). Each entry is a root-cause guess with a status
    /// (open/confirmed/refuted) and optional evidence; the tool upserts by id
    /// and renders the whole ledger back into the sub-agent's context so its
    /// reasoning persists across iterations. Shared (`Arc`) so a cloned context
    /// reads the same ledger; single-threaded in practice — like `probe_sink`,
    /// debug tools stay off the concurrency-safe path. Unlike probes there is
    /// nothing to revert: the ledger is pure bookkeeping and never touches the
    /// working tree (it mirrors to an OS-temp scratch file instead).
    pub hypothesis_sink: Arc<Mutex<Vec<DebugHypothesis>>>,
    /// Parent-session MCP passthrough for spawned sub-agents (see
    /// `crate::registry::McpPassthrough`). Shared `Arc<Mutex<…>>` like the
    /// model fields so the session host's one-time install (after MCP
    /// discovery wiring) reaches every registry/context clone, including the
    /// concurrent-dispatch closure that serves live spawns. `None` until the
    /// host installs it (headless runs without MCP stay `None`).
    pub mcp_passthrough: Arc<Mutex<Option<crate::registry::McpPassthrough>>>,
    /// Host default for an `Agent` call that omits `background`.
    ///
    /// `true` only in the interactive main session, whose REPL consumes the
    /// agent-completion channel and re-injects a detached agent's result as a
    /// fresh turn. Sub-agent executors (fresh [`ToolContext::new`]) and
    /// headless runs have no such consumer — a detached agent's result would
    /// be silently lost there — so they keep the blocking default.
    /// `Arc<Mutex<…>>` like the other cells so every registry clone (including
    /// the concurrent-dispatch closure) sees the value the CLI set at boot.
    pub background_agent_default: Arc<Mutex<bool>>,
    /// 대화(세션) 스코프 read-before-edit 레지스트리 (CC 패리티).
    ///
    /// `read_file`/`write_file`/`edit_file` 성공 시 `path → (mtime, hash)`
    /// 스냅샷을 기록하고, `edit_file`·기존 파일을 덮어쓰는 `write_file`은
    /// 실행 전에 "이 대화에서 읽었고, 마지막 읽기 이후 디스크가 변하지
    /// 않았음"을 강제한다 — 사용자/외부 편집의 조용한 clobber 차단.
    ///
    /// **스코프 주의**: 반드시 이 컨텍스트(=대화)에 소속된다. 프로세스
    /// 전역으로 승격하면 서브에이전트와 공유되어 다른 대화의 읽기 기록이
    /// 가드를 오염시킨다(과거 전역 reasoning-replay 캐시 사고와 같은
    /// 클래스). 서브에이전트는 fresh [`ToolContext::new`]를 받으므로
    /// 자동으로 자기만의 레지스트리를 가진다. `Arc<Mutex<…>>`는 같은
    /// 대화의 registry/context 클론(동시 디스패치 클로저 포함)끼리만
    /// 상태를 공유하기 위함이다.
    pub file_reads: Arc<Mutex<runtime::FileReadRegistry>>,
    workspace_checkpoints: Arc<Mutex<WorkspaceCheckpointStore>>,
    workspace_hunk_attribution: Arc<Mutex<crate::HunkAttributionLedger>>,
    /// Lazily initialized code-symbol graph for this session's workspace.
    ///
    /// The cell is shared across registry clones, while a fresh
    /// [`ToolContext::new`] gives a sub-agent its own index handle.
    pub(crate) codegraph: Arc<Mutex<Option<codegraph::CodeGraph>>>,
}

impl fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolContext")
            .field("lsp", &self.lsp)
            .field("teams", &self.teams)
            .field("crons", &self.crons)
            .field("tasks", &self.tasks)
            .field("workers", &self.workers)
            .field("hook_config", &"..")
            .field(
                "user_question_channel",
                &self.user_question_channel().map(|_| ".."),
            )
            .field("active_model", &self.active_model())
            .field("session_id", &self.session_id())
            .field(
                "agent_registry",
                &self.registry_handle().map(|registry| registry.root().to_path_buf()),
            )
            .field("session_permission_mode", &self.session_permission_mode())
            .field("workspace_root", &self.workspace_root)
            .field("cwd", &self.cwd)
            .field("worktree_confinement", &self.worktree_confinement)
            .field("lease_owner", &self.lease_owner)
            .field("subagent_identity", &self.subagent_identity)
            .field("disabled_tools", &self.disabled_tools())
            .field(
                "background_agent_default",
                &self.background_agent_default(),
            )
            .field("image_sink", &"..")
            .field("probe_sink", &"..")
            .field("hypothesis_sink", &"..")
            .field("file_reads", &"..")
            .field("workspace_checkpoints", &"..")
            .field("codegraph", &"..")
            .finish_non_exhaustive()
    }
}

impl ToolContext {
    /// The person's launch model, never a model-authored tool argument.
    pub fn set_person_model_pin(&self, model: Option<&str>) {
        *self.person_model_pin.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = model.map(str::to_string);
    }

    #[must_use]
    pub fn person_model_pin(&self) -> Option<String> {
        self.person_model_pin.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Install the host-owned shell constraint (shared by every dispatch clone).
    pub fn set_inspection_shell(&self, enabled: bool) {
        *self.inspection_shell.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = enabled;
    }

    #[must_use]
    pub fn inspection_shell(&self) -> bool {
        *self.inspection_shell.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The frame of the last desktop picture the model was shown.
    #[must_use]
    pub fn computer_frame(&self) -> Option<ShotFrame> {
        *self.computer.frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Remember the frame of a desktop picture the model was just shown.
    pub fn set_computer_frame(&self, frame: ShotFrame) {
        *self.computer.frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(frame);
        self.set_computer_observation(None);
    }

    pub(crate) fn has_computer_observation(&self, observation: &ComputerObservation) -> bool {
        self.computer.observation.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() == Some(observation)
    }

    pub(crate) fn set_computer_observation(&self, observation: Option<ComputerObservation>) {
        *self.computer.observation.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = observation;
    }

    /// This model's name in the window's last-look table.
    #[must_use]
    pub fn computer_viewer(&self) -> &str {
        &self.computer.viewer
    }

    /// The id of the last marked look the model was shown.
    #[must_use]
    pub fn computer_marked_look(&self) -> Option<String> {
        self.computer.marked_look.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Remember the marked look the model was just shown — or that the
    /// picture it was just shown carried no numbers.
    pub fn set_computer_marked_look(&self, look: Option<&str>) {
        *self.computer.marked_look.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = look.map(str::to_string);
    }

    /// Create a new context with default (empty) registries.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fork_source: Arc::new(Mutex::new(None)),
            lsp: LspRegistry::new(),
            teams: TeamRegistry::new(),
            crons: CronRegistry::new(),
            tasks: TaskRegistry::new(),
            inspection_shell: Arc::default(),
            person_model_pin: Arc::default(),
            computer: Arc::new(ComputerSight::new()),
            workers: WorkerRegistry::new(),
            hook_config: runtime::RuntimeHookConfig::default(),
            user_question_channel: Arc::new(Mutex::new(None)),
            active_model: Arc::new(Mutex::new(None)),
            active_model_pinned: Arc::new(Mutex::new(false)),
            session_id: Arc::new(Mutex::new(None)),
            agent_registry: Arc::new(Mutex::new(None)),
            session_permission_mode: Arc::new(Mutex::new(None)),
            plan_selected: Arc::new(Mutex::new(false)),
            workspace_root: None,
            cwd: None,
            worktree_confinement: None,
            lease_owner: None,
            subagent_identity: None,
            disabled_tools: Arc::new(Mutex::new(BTreeSet::new())),
            tool_invocations: Arc::new(Mutex::new(Vec::new())),
            active_run_id: Arc::new(Mutex::new(None)),
            turn_evidence: Arc::default(),
            turn_agent_policy: Arc::new(Mutex::new(None)),
            route_decisions: Arc::new(Mutex::new(Vec::new())),
            image_sink: Arc::new(Mutex::new(Vec::new())),
            probe_sink: Arc::new(Mutex::new(Vec::new())),
            hypothesis_sink: Arc::new(Mutex::new(Vec::new())),
            mcp_passthrough: Arc::new(Mutex::new(None)),
            background_agent_default: Arc::new(Mutex::new(false)),
            file_reads: Arc::new(Mutex::new(runtime::FileReadRegistry::new())),
            workspace_checkpoints: Arc::new(Mutex::new(WorkspaceCheckpointStore::default())),
            workspace_hunk_attribution: Arc::new(Mutex::new(
                crate::HunkAttributionLedger::default(),
            )),
            codegraph: Arc::new(Mutex::new(None)),
        }
    }

    /// Start capturing successful guarded file writes for one foreground turn.
    pub fn begin_workspace_checkpoint(&self, suggested_turn_index: usize) -> usize {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .begin_turn(suggested_turn_index)
    }

    /// Finalize the active turn, capturing every touched file's final bytes.
    pub fn finish_workspace_checkpoint(&self) -> std::io::Result<Option<WorkspaceCheckpoint>> {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish_turn()
    }

    /// Mark the active checkpoint incomplete because a shell command may write.
    pub fn mark_workspace_checkpoint_incomplete(&self) {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .mark_incomplete();
    }

    #[must_use]
    pub fn workspace_checkpoints(&self) -> Vec<WorkspaceCheckpoint> {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .checkpoints()
    }

    /// Rebuild the session's hunk attribution view from checkpoints and the
    /// live worktree while preserving prior accept/reject/stale decisions.
    pub fn workspace_hunk_attribution(&self) -> std::io::Result<crate::HunkAttributionLedger> {
        let mut rebuilt = crate::HunkAttributionLedger::build(&self.workspace_checkpoints())?;
        let mut ledger = self
            .workspace_hunk_attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rebuilt.reconcile(&ledger);
        *ledger = rebuilt.clone();
        Ok(rebuilt)
    }

    pub fn accept_workspace_hunk(
        &self,
        index: usize,
    ) -> Result<crate::HunkAttributionLedger, crate::ReviewHunkError> {
        let mut ledger = self
            .workspace_hunk_attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.accept(index)?;
        Ok(ledger.clone())
    }

    #[must_use]
    pub fn current_workspace_hunk_attribution(&self) -> crate::HunkAttributionLedger {
        self.workspace_hunk_attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn accept_workspace_file_hunks(
        &self,
        path: &std::path::Path,
    ) -> crate::HunkAttributionLedger {
        let mut ledger = self
            .workspace_hunk_attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.accept_file(path);
        ledger.clone()
    }

    pub fn reject_workspace_hunk(
        &self,
        index: usize,
    ) -> Result<crate::HunkAttributionLedger, crate::ReviewHunkError> {
        let path = {
            let ledger = self
                .workspace_hunk_attribution
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger
                .hunks
                .get(index)
                .ok_or(crate::ReviewHunkError::NotFound(index))?
                .path
                .clone()
        };
        // This is normally outside an agent turn (so these are no-ops), but
        // using the same checkpoint hooks keeps an in-flight host integration
        // from silently bypassing the write ledger.
        let _ = self.record_workspace_checkpoint_before(&path);
        let result = {
            let mut ledger = self
                .workspace_hunk_attribution
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.reject(index).map(|()| ledger.clone())
        };
        if result.is_ok() {
            self.record_workspace_checkpoint_write(&path);
            self.file_reads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record_from_disk(&path);
        }
        result
    }

    pub fn restore_workspace_to_before(
        &self,
        target_turn_index: usize,
        force: bool,
    ) -> Result<WorkspaceRestoreSummary, crate::ToolError> {
        crate::file_tools::restore_workspace_checkpoint(self, None, target_turn_index, force)
    }

    /// Rebind the read-before-edit registry to a session's sidecar
    /// (`<session>.file-reads.json`) and restore whatever it recorded —
    /// [`runtime::FileReadRegistry::rebind_sidecar`]. Every session
    /// create/swap/resume must pass through here (mirror of
    /// [`Self::reset_workspace_checkpoint_session`]): a resumed conversation
    /// keeps the freshness baseline it actually observed, while a swap can
    /// never leak the previous conversation's read history into the new
    /// guard. `None` unbinds (in-memory only — subagent contexts).
    pub fn reset_file_reads_session(&self, sidecar: Option<std::path::PathBuf>) {
        self.file_reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .rebind_sidecar(sidecar);
    }

    pub fn reset_workspace_checkpoint_session(
        &self,
        durable_dir: Option<std::path::PathBuf>,
    ) -> std::io::Result<()> {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reset_session(durable_dir)?;
        *self
            .workspace_hunk_attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            crate::HunkAttributionLedger::default();
        Ok(())
    }

    pub fn copy_workspace_checkpoint_state_from(&self, source: &Self) {
        let state = source
            .workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        *self
            .workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
        let attribution = source
            .workspace_hunk_attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        *self
            .workspace_hunk_attribution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = attribution;
    }

    pub(crate) fn workspace_restore_plan(
        &self,
        target_turn_index: usize,
    ) -> Result<WorkspaceRestorePlan, String> {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .compose_restore_plan(target_turn_index)
    }

    pub(crate) fn record_workspace_checkpoint_before(&self, path: &std::path::Path) -> std::io::Result<()> {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_before(path)
    }

    pub(crate) fn record_workspace_checkpoint_write(&self, path: &std::path::Path) {
        self.workspace_checkpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_write_success(path);
    }

    /// Reuse a task registry owned by a longer-lived session harness.
    #[must_use]
    pub fn with_tasks(mut self, tasks: TaskRegistry) -> Self {
        self.tasks = tasks;
        self
    }

    /// Install the sub-agent MCP passthrough (see
    /// [`crate::registry::McpPassthrough`]). Writes through the shared cell so
    /// every existing context/registry clone observes it.
    pub(crate) fn install_mcp_passthrough(&self, passthrough: crate::registry::McpPassthrough) {
        *self
            .mcp_passthrough
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(passthrough);
    }

    /// Snapshot of the installed sub-agent MCP passthrough, if any.
    #[must_use]
    pub(crate) fn mcp_passthrough(&self) -> Option<crate::registry::McpPassthrough> {
        self.mcp_passthrough
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Publish the conversation a `fork` inherits (see [`ForkSource`]). The
    /// session host calls this after every runtime build and rebuild; it
    /// writes through the shared cell, so every registry clone reads the
    /// latest value.
    pub fn install_fork_source(&self, source: ForkSource) {
        *self
            .fork_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
    }

    /// The conversation a `fork` would inherit, if a session host published
    /// one. `None` where nobody owns a transcript to fork — a fan-out member,
    /// a bare test harness — and the `Agent` tool refuses `fork` there.
    #[must_use]
    pub fn fork_source(&self) -> Option<ForkSource> {
        self.fork_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Drain every staged instrumentation probe and remove its marker snippet
    /// from the file it was inserted into, restoring the file byte-for-byte
    /// (the snippet was recorded verbatim, so a literal strip is exact and
    /// idempotent — the unique `/*ZO_PROBE:<id>*/` marker means it matches
    /// nothing else). Any unrelated edits the agent made around the probe are
    /// preserved; only the probe line vanishes. Called when a debugger
    /// sub-agent's run ends so instrumentation never leaks into the diff.
    /// Best-effort per file (a read/write error skips that probe); returns the
    /// number of probes actually reverted.
    #[must_use]
    pub fn revert_probes(&self) -> usize {
        revert_probe_sink(&self.probe_sink)
    }

    /// Bind the workspace root used for file-tool boundary enforcement.
    ///
    /// Wired by the CLI when constructing the runtime so `write_file` /
    /// `edit_file` paths can be checked against the user's cwd.
    #[must_use]
    pub fn with_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(root.into());
        self
    }

    /// Attach a user-question channel for `AskUserQuestion` tool support.
    #[must_use]
    pub fn with_user_question_channel(self, channel: Arc<dyn UserQuestionChannel>) -> Self {
        self.set_user_question_channel(Some(channel));
        self
    }

    /// Install (or clear) the interactive user channel. Writes through the
    /// shared cell so every existing context/registry clone observes it —
    /// notably the concurrent-dispatch closure cloned at session boot.
    pub fn set_user_question_channel(&self, channel: Option<Arc<dyn UserQuestionChannel>>) {
        *self
            .user_question_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = channel;
    }

    /// Snapshot of the installed interactive user channel, if any.
    #[must_use]
    pub fn user_question_channel(&self) -> Option<Arc<dyn UserQuestionChannel>> {
        self.user_question_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn with_hook_config(mut self, hook_config: runtime::RuntimeHookConfig) -> Self {
        self.hook_config = hook_config;
        self
    }

    #[must_use]
    pub fn hook_config(&self) -> &runtime::RuntimeHookConfig {
        &self.hook_config
    }

    /// Replace the active foreground model at runtime (e.g. after `/model`),
    /// preserving the shared cell identity so every registry clone — including
    /// the concurrent-dispatch closure that serves live `SpawnMultiAgent` /
    /// `Agent` calls — observes the new model without a runtime rebuild. Blank
    /// input clears it (mirrors [`Self::set_disabled_tools`]).
    pub fn set_active_model(&self, model: &str) {
        *self
            .active_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            (!model.trim().is_empty()).then(|| model.trim().to_string());
    }

    /// Record whether this agent's active model was chosen explicitly for it
    /// (see the field doc on [`Self::active_model_pinned`]). Kept separate
    /// from [`Self::set_active_model`] so router-driven model swaps never
    /// accidentally mark themselves as explicit intent. The foreground session
    /// never calls this — only the sub-agent executor does, for its own
    /// nested-spawn cascade.
    pub fn set_active_model_pinned(&self, pinned: bool) {
        *self
            .active_model_pinned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pinned;
    }

    /// `true` when this agent's model was chosen explicitly for it (never set
    /// by a foreground `/model` pin — see the field doc).
    #[must_use]
    pub fn active_model_pinned(&self) -> bool {
        *self
            .active_model_pinned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Snapshot the active foreground model.
    #[must_use]
    pub fn active_model(&self) -> Option<String> {
        self.active_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replace the foreground session id used to stamp spawned agent manifests.
    /// Blank input clears it; older/headless contexts can leave it unset and
    /// the display layer falls back to the existing time-scope behavior.
    pub fn set_session_id(&self, session_id: &str) {
        let session_id = session_id.trim();
        *self
            .session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            (!session_id.is_empty()).then(|| session_id.to_string());
    }

    /// Snapshot the foreground session id that owns spawned agent manifests.
    #[must_use]
    pub fn session_id(&self) -> Option<String> {
        self.session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Install the session-owned agent registry (see `Self::agent_registry`).
    /// Shared-cell semantics: every clone of this context sees it.
    pub fn install_registry(&self, registry: Arc<crate::AgentRegistry>) {
        *self
            .agent_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(registry);
    }

    /// Builder form of [`Self::install_registry`], for a fresh sub-agent
    /// context inheriting its parent's registry. `None` leaves the cell empty.
    #[must_use]
    pub fn with_registry(self, registry: Option<Arc<crate::AgentRegistry>>) -> Self {
        if let Some(registry) = registry {
            self.install_registry(registry);
        }
        self
    }

    /// The installed registry, if a session installed one.
    #[must_use]
    pub fn registry_handle(&self) -> Option<Arc<crate::AgentRegistry>> {
        self.agent_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The registry every agent road in this context reads and writes —
    /// the installed one, else the documented cwd fallback.
    #[must_use]
    pub fn registry(&self) -> Arc<crate::AgentRegistry> {
        self.registry_handle()
            .unwrap_or_else(crate::AgentRegistry::unowned_from_cwd)
    }

    /// Declare whether an `Agent` call that omits `background` detaches by
    /// default. Set `true` only by a host that consumes the agent-completion
    /// channel and re-injects results (the interactive REPL); see
    /// [`Self::background_agent_default`].
    pub fn set_background_agent_default(&self, background: bool) {
        *self
            .background_agent_default
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = background;
    }

    /// Host default applied when an `Agent` call omits `background`.
    #[must_use]
    pub fn background_agent_default(&self) -> bool {
        *self
            .background_agent_default
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record the active foreground permission mode (shared cell, like
    /// [`Self::set_active_model`]) so the file-tool boundary relaxation can see
    /// the session mode even though the foreground registry carries no
    /// `PermissionEnforcer`. Propagates to every registry clone, so a live
    /// Shift+Tab / `/permission` switch takes effect without a runtime rebuild.
    pub fn set_permission_mode(&self, mode: runtime::PermissionMode) {
        *self
            .session_permission_mode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(mode);
    }

    /// Snapshot the active foreground permission mode, if one was recorded.
    #[must_use]
    pub fn session_permission_mode(&self) -> Option<runtime::PermissionMode> {
        *self
            .session_permission_mode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record whether the user has explicitly selected Plan (shared cell, like
    /// [`Self::set_permission_mode`]) so the registry can drive its plan-mode
    /// tool surface from authoritative state rather than prompt inference.
    /// Propagates to every registry clone, so a live Shift+Tab / `/plan` toggle
    /// takes effect without a runtime rebuild. Set only by the user-driven seam.
    pub fn set_plan_selected(&self, selected: bool) {
        *self
            .plan_selected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = selected;
    }

    /// `true` when the user has explicitly selected Plan (see the field doc on
    /// [`Self::plan_selected`]).
    #[must_use]
    pub fn plan_selected(&self) -> bool {
        *self
            .plan_selected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The model a freshly spawned `Agent` / `SpawnMultiAgent` should inherit as
    /// its `parent_model`. This is intentionally only the active foreground
    /// model: role-specific or cross-provider selection belongs to `/smart`
    /// routing, while `ZO_AGENT_MODEL` and explicit per-agent metadata are
    /// layered on top by `resolve_agent_model`.
    #[must_use]
    pub fn spawn_parent_model(&self) -> Option<String> {
        self.active_model()
    }

    /// Pin the working directory for tool execution (worktree isolation).
    ///
    /// Sets both the execution `cwd` (so `bash` and relative file paths
    /// resolve there) and the `workspace_root` boundary, confining writes to
    /// the directory. Leaving it unset preserves the process-cwd default.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        self.workspace_root = Some(cwd.clone());
        self.cwd = Some(cwd);
        self
    }

    /// Pin the working directory *and* confine shell git operations to it, for a
    /// worktree-isolated sub-agent. Like [`Self::with_cwd`], but additionally
    /// records the worktree root so the bash guard refuses `git -C` /
    /// `--git-dir` / `GIT_DIR=` targets that escape into the shared checkout.
    #[must_use]
    pub fn with_worktree_confinement(mut self, cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        self.worktree_confinement = Some(cwd.clone());
        self.with_cwd(cwd)
    }

    /// Set the write-lease owner id for this context (track 4-2). A blank id
    /// clears it (lease coordination off — the solo default). Foreground wires
    /// its session id; a spawned sub-agent wires its agent id, so two concurrent
    /// writers are distinguishable and the same actor never conflicts with
    /// itself across calls.
    #[must_use]
    pub fn with_lease_owner(mut self, owner: impl Into<String>) -> Self {
        let owner = owner.into();
        self.lease_owner = (!owner.trim().is_empty()).then(|| owner.trim().to_string());
        self
    }

    /// Mark this context as belonging to a spawned SUB-AGENT and record who it
    /// is. Only `agent_tools::runtime::build_subagent_tool_executor` sets it —
    /// a foreground/headless main session leaves it `None` — so `Some` is the
    /// authoritative "the caller of this tool is a sub-agent" answer, and the
    /// pair is the identity a mid-run agent→main message is attributed to.
    ///
    /// Deliberately NOT inferred from [`Self::lease_owner`]: that field is
    /// documented to hold a *foreground session id* just as legitimately as an
    /// agent id, so keying caller identity off it would misclassify the main
    /// session the moment lease coordination is wired there.
    #[must_use]
    pub fn with_subagent_identity(mut self, agent_id: impl Into<String>, name: impl Into<String>) -> Self {
        let agent_id = agent_id.into();
        let name = name.into();
        if !agent_id.trim().is_empty() {
            self.subagent_identity = Some(SubagentIdentity {
                agent_id: agent_id.trim().to_string(),
                name: name.trim().to_string(),
            });
        }
        self
    }

    /// The spawned sub-agent this context runs tools for, or `None` in a main
    /// (foreground/headless) session. See [`Self::with_subagent_identity`].
    #[must_use]
    pub fn subagent_identity(&self) -> Option<&SubagentIdentity> {
        self.subagent_identity.as_ref()
    }

    /// Replace the disabled-tool set while preserving the shared sink identity
    /// for existing context clones.
    pub fn set_disabled_tools(&self, tools: BTreeSet<String>) {
        *self
            .disabled_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = canonical_disabled_tools(tools);
    }

    /// Attach disabled-tool names while building a fresh context.
    #[must_use]
    pub fn with_disabled_tools(self, tools: BTreeSet<String>) -> Self {
        self.set_disabled_tools(tools);
        self
    }

    /// Snapshot the currently disabled canonical tool names.
    #[must_use]
    pub fn disabled_tools(&self) -> BTreeSet<String> {
        self.disabled_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Snapshot the phase-1 `ToolGateway` shadow ledger.
    #[must_use]
    pub fn tool_invocations(&self) -> Vec<ToolInvocation> {
        self.tool_invocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Roll the shadow ledger up into an [`AuditSummary`] — the read consumer
    /// of the otherwise write-only ledger (WI-E2). Backs the `Audit` tool and
    /// any headless / slash audit surface. The structured route decisions
    /// (WI-C) are folded in here since they live beside the invocation ledger.
    #[must_use]
    pub fn audit_summary(&self) -> AuditSummary {
        let mut summary = summarize_invocations(&self.tool_invocations());
        summary.route_decisions = self.route_decisions();
        summary
    }

    /// Clone the value behind a poison-tolerant id slot. Centralizes the stamp
    /// read so callers assign an owned value instead of `place = guard.clone()`.
    fn snapshot_slot(slot: &Mutex<Option<String>>) -> Option<String> {
        slot.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn record_tool_invocation(&self, mut invocation: ToolInvocation) {
        // Stamp the active run/turn so the audit can join this call to the
        // workflow run and the foreground turn that issued it (WI-C). Only set
        // when actually active, so non-workflow / pre-turn calls stay unstamped.
        invocation.run_id = Self::snapshot_slot(&self.active_run_id);
        invocation.turn_id = self.active_turn_id();
        let mut ledger = self
            .tool_invocations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.push(invocation);
        Self::trim_ledger(&mut ledger);
    }

    /// Drop the oldest entries once a shadow audit ledger exceeds
    /// [`MAX_AUDIT_LEDGER_ENTRIES`], keeping the bound on this append-only,
    /// session-lifetime collection. Shared by both `record_*` paths so the two
    /// ledgers can never diverge on the eviction rule.
    fn trim_ledger<T>(ledger: &mut Vec<T>) {
        if ledger.len() > MAX_AUDIT_LEDGER_ENTRIES {
            let excess = ledger.len() - MAX_AUDIT_LEDGER_ENTRIES;
            ledger.drain(0..excess);
        }
    }

    /// Set (or clear, with `None`) the active workflow run id stamped onto
    /// subsequent tool invocations. Shared across clones (WI-C).
    pub fn set_active_run_id(&self, run_id: Option<String>) {
        *self
            .active_run_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = run_id;
    }

    /// A new user turn needs fresh design reading, including in headless sessions.
    pub fn begin_skill_turn(&self) {
        self.turn_evidence.lock().unwrap_or_else(std::sync::PoisonError::into_inner).artifact_skill_reads.clear();
    }

    pub(crate) fn note_artifact_skill_read(&self, path: &std::path::Path) {
        if let Some(name) = path.parent().and_then(std::path::Path::file_name).and_then(std::ffi::OsStr::to_str) {
            if crate::artifact_tools::DESIGN_SKILLS.contains(&name) {
                self.turn_evidence.lock().unwrap_or_else(std::sync::PoisonError::into_inner).artifact_skill_reads.insert(name.to_string());
            }
        }
    }

    pub(crate) fn artifact_design_read(&self) -> bool {
        !self.turn_evidence.lock().unwrap_or_else(std::sync::PoisonError::into_inner).artifact_skill_reads.is_empty()
    }

    /// Set (or clear, with `None`) the active foreground turn id stamped onto
    /// subsequent tool invocations.
    pub fn set_active_turn_id(&self, turn_id: Option<String>) {
        let mut evidence = self.turn_evidence.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if evidence.id != turn_id { evidence.artifact_skill_reads.clear(); }
        evidence.id = turn_id;
    }

    fn active_turn_id(&self) -> Option<String> {
        self.turn_evidence.lock().unwrap_or_else(std::sync::PoisonError::into_inner).id.clone()
    }

    /// Install this turn's [`TurnAgentPolicy`] (or clear it with `None`). Set by
    /// the host at every model-led turn start; the `Agent` dispatch guard reads
    /// it via [`Self::turn_agent_policy`]. Overwritten per turn, like
    /// [`Self::set_active_turn_id`].
    pub fn set_turn_agent_policy(&self, policy: Option<TurnAgentPolicy>) {
        *self
            .turn_agent_policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
    }

    /// This turn's agent-delegation policy, or `None` when unknown (the guard
    /// then fails open). See [`TurnAgentPolicy`].
    #[must_use]
    pub fn turn_agent_policy(&self) -> Option<TurnAgentPolicy> {
        *self
            .turn_agent_policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record one structured route decision (WI-C). Stamps the active turn id so
    /// the decision joins the turn it routed. Mirrors `record_tool_invocation`.
    pub fn record_route_decision(&self, mut decision: RouteDecisionRecord) {
        if decision.turn_id.is_none() {
            decision.turn_id = self.active_turn_id();
        }
        let mut ledger = self
            .route_decisions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.push(decision);
        Self::trim_ledger(&mut ledger);
    }

    /// Snapshot the structured route-decision ledger.
    #[must_use]
    pub fn route_decisions(&self) -> Vec<RouteDecisionRecord> {
        self.route_decisions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// True when `name` maps to a disabled canonical tool.
    #[must_use]
    pub fn is_tool_disabled(&self, name: &str) -> bool {
        let canonical = canonical_tool_name(name.trim());
        self.disabled_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(canonical.as_str())
    }

    /// Share an LSP registry into this context (a cheap `Arc`-clone of the
    /// parent's live servers).
    ///
    /// Wired so a non-isolated sub-agent inherits the parent session's LSP
    /// servers, letting the edit/write diagnostics enrichment
    /// (`crate::dispatch`) surface diagnostics to e.g. a debugger sub-agent.
    /// An empty registry is harmless — the enrich path is gated on
    /// `!lsp.is_empty()`.
    #[must_use]
    pub fn with_lsp(mut self, lsp: LspRegistry) -> Self {
        self.lsp = lsp;
        self
    }
}

pub(crate) fn disabled_tool_error(tool: &str) -> ToolError {
    ToolError::PermissionDenied {
        tool: tool.to_string(),
        reason: TOOL_TOGGLE_DENIAL_REASON.to_string(),
    }
}

fn canonical_disabled_tools(tools: BTreeSet<String>) -> BTreeSet<String> {
    tools
        .into_iter()
        .filter_map(|tool| {
            let trimmed = tool.trim();
            (!trimmed.is_empty()).then(|| canonical_tool_name(trimmed))
        })
        .collect()
}

impl Default for ToolContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Drain `sink` and strip each probe's marker snippet from the file it was
/// inserted into, restoring it byte-for-byte. Shared by
/// [`ToolContext::revert_probes`] and [`ProbeRevertGuard`] so cleanup is
/// identical whether it runs explicitly at run end or via the guard's `Drop`
/// during a panic unwind. Best-effort per file; returns the count reverted.
pub fn revert_probe_sink(sink: &Mutex<Vec<Probe>>) -> usize {
    let probes = std::mem::take(
        &mut *sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let mut reverted = 0;
    for probe in probes {
        let Ok(content) = std::fs::read_to_string(&probe.path) else {
            continue;
        };
        if !content.contains(&probe.snippet) {
            continue;
        }
        let restored = content.replace(&probe.snippet, "");
        if std::fs::write(&probe.path, restored).is_ok() {
            reverted += 1;
        }
    }
    reverted
}

/// RAII guard that reverts staged instrumentation probes when dropped.
///
/// Holds a clone of the shared probe-sink `Arc`, so it reverts the same ledger
/// the tools wrote to. Its `Drop` runs on every exit path — normal return, an
/// error `?`, and a **panic unwind** — which a plain `revert_probes()` call does
/// not cover: a panic inside the turn jumps past that call, and the runtime
/// (owning the sink) is then dropped with the markers still on disk. On the
/// normal path the explicit `revert_probes()` drains the sink first, so the
/// guard's `Drop` is a harmless no-op (the ledger is already empty).
pub struct ProbeRevertGuard {
    sink: Arc<Mutex<Vec<Probe>>>,
}

impl ProbeRevertGuard {
    /// Build a guard from a clone of the executor's probe-sink `Arc`.
    #[must_use]
    pub fn new(sink: Arc<Mutex<Vec<Probe>>>) -> Self {
        Self { sink }
    }
}

impl Drop for ProbeRevertGuard {
    fn drop(&mut self) {
        revert_probe_sink(&self.sink);
    }
}

#[cfg(test)]
mod probe_guard_tests {
    use super::{Probe, ProbeRevertGuard, revert_probe_sink};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_with_probe(body: &str, snippet: &str) -> (std::path::PathBuf, Arc<Mutex<Vec<Probe>>>) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("zo-g23-guard-{unique}-{counter}.rs"));
        std::fs::write(&path, format!("{body}{snippet}")).expect("write fixture");
        let sink = Arc::new(Mutex::new(vec![Probe {
            path: path.clone(),
            snippet: snippet.to_string(),
        }]));
        (path, sink)
    }

    #[test]
    fn guard_reverts_probes_on_panic_unwind() {
        // The exact gap a bare `revert_probes()` call misses: a panic while the
        // guard is alive must still strip the probe during the unwind, so a
        // sub-agent that panics mid-turn never leaks a marker into the tree.
        let original = "fn f() {}\n";
        let snippet = "\n/*ZO_PROBE:1*/ eprintln!(\"x\");";
        let (path, sink) = temp_with_probe(original, snippet);
        assert!(
            std::fs::read_to_string(&path)
                .expect("read")
                .contains("ZO_PROBE")
        );

        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = ProbeRevertGuard::new(sink.clone());
            panic!("simulated turn panic");
        }));
        assert!(result.is_err(), "panic propagated past the guard");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            original,
            "probe reverted during the unwind"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn explicit_revert_then_guard_drop_is_idempotent() {
        // Normal path: the explicit revert drains the ledger first, so the
        // guard's later drop is a harmless no-op (no error, no double-strip).
        let original = "let a = 1;\n";
        let snippet = "\n/*ZO_PROBE:2*/ // trace";
        let (path, sink) = temp_with_probe(original, snippet);

        let guard = ProbeRevertGuard::new(sink.clone());
        assert_eq!(revert_probe_sink(&sink), 1, "explicit revert strips it");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), original);
        drop(guard); // ledger already drained → no-op
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            original,
            "still restored after the guard drops"
        );
        assert!(
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod audit_context_tests {
    use super::ToolContext;
    use crate::gateway::{
        RouteDecisionRecord, ToolResultMetadata, begin_tool_invocation, epoch_millis_now,
        successful_result,
    };
    use serde_json::json;

    fn ok_invocation(name: &str) -> crate::gateway::ToolInvocation {
        begin_tool_invocation(name, name, &json!({}), None).finish(
            successful_result(ToolResultMetadata {
                output_chars: 1,
                returned_chars: 1,
                truncated: false,
                artifact: None,
            }),
            epoch_millis_now(),
        )
    }

    #[test]
    fn tool_invocation_carries_run_and_turn_id() {
        let ctx = ToolContext::new();
        ctx.set_active_run_id(Some("run-7".to_string()));
        ctx.set_active_turn_id(Some("turn-3".to_string()));

        ctx.record_tool_invocation(ok_invocation("bash"));

        let recorded = ctx.tool_invocations();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].run_id.as_deref(), Some("run-7"));
        assert_eq!(recorded[0].turn_id.as_deref(), Some("turn-3"));

        // Clearing the active ids leaves later calls unstamped — so a run's
        // calls can be filtered out by joining on `run_id`.
        ctx.set_active_run_id(None);
        ctx.set_active_turn_id(None);
        ctx.record_tool_invocation(ok_invocation("read_file"));
        let recorded = ctx.tool_invocations();
        assert_eq!(recorded[1].run_id, None);
        assert_eq!(recorded[1].turn_id, None);
    }

    #[test]
    fn audit_ledgers_are_bounded_to_the_cap() {
        use super::MAX_AUDIT_LEDGER_ENTRIES;

        let ctx = ToolContext::new();
        let overflow = MAX_AUDIT_LEDGER_ENTRIES + 25;
        for _ in 0..overflow {
            ctx.record_tool_invocation(ok_invocation("bash"));
            ctx.record_route_decision(RouteDecisionRecord {
                shape: "solo".to_string(),
                canonical_shape: "solo".to_string(),
                intent: "other".to_string(),
                confidence: 1.0,
                host_prespawn: false,
                semantic_triage: false,
                reasons: Vec::new(),
                turn_id: None,
            });
        }

        // Both append-only, session-lifetime ledgers stay capped rather than
        // growing without bound for a long-running session.
        assert_eq!(ctx.tool_invocations().len(), MAX_AUDIT_LEDGER_ENTRIES);
        assert_eq!(ctx.route_decisions().len(), MAX_AUDIT_LEDGER_ENTRIES);
        // The audit reflects the retained recent window once capped.
        assert_eq!(ctx.audit_summary().total, MAX_AUDIT_LEDGER_ENTRIES);
    }


    #[test]
    fn route_decision_defaults_semantic_triage_for_old_records() {
        let record: RouteDecisionRecord = serde_json::from_value(json!({
            "shape": "pipeline",
            "confidence": 0.7,
            "host_prespawn": false
        }))
        .expect("old route decision record should deserialize");

        assert!(!record.host_prespawn);
        assert!(!record.semantic_triage);
        assert!(record.intent.is_empty());
    }

    #[test]
    fn route_decision_is_recorded_in_audit() {
        let ctx = ToolContext::new();
        ctx.set_active_turn_id(Some("turn-9".to_string()));
        ctx.record_route_decision(RouteDecisionRecord {
            shape: "delegate_one".to_string(),
            canonical_shape: "one-specialist".to_string(),
            intent: "analysis".to_string(),
            confidence: 0.8,
            host_prespawn: false,
            semantic_triage: true,
            reasons: vec!["red test escalated".to_string()],
            turn_id: None,
        });

        let summary = ctx.audit_summary();
        assert_eq!(summary.route_decisions.len(), 1);
        assert_eq!(summary.route_decisions[0].shape, "delegate_one");
        // The active turn id is stamped when the record omitted it.
        assert_eq!(
            summary.route_decisions[0].turn_id.as_deref(),
            Some("turn-9")
        );
        // Structured, not an eprintln: it round-trips through the audit JSON.
        let value = serde_json::to_value(&summary).expect("audit summary serializes");
        assert_eq!(value["route_decisions"][0]["shape"], "delegate_one");
        assert_eq!(value["route_decisions"][0]["intent"], "analysis");
        assert_eq!(value["route_decisions"][0]["host_prespawn"], false);
        assert_eq!(value["route_decisions"][0]["semantic_triage"], true);
    }
}
