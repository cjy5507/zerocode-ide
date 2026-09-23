//! 얇은 세션 — `LiveCli`(5.7k줄) 대신 엔진 경계 위에 새로 쓴 20%.
//!
//! 가진 것: 런타임 빌드(MCP·LSP·플러그인 포함), 스트리밍 턴 구동, 턴별 영속,
//! `--resume`, 모델/권한/effort 세터, 압축, 한 줄 상태. 없는 것: 164개 슬래시
//! 핸들러, goal/loop 컨트롤러, undo 스냅샷, 원격 승인, 자동화 게이트 —
//! zerocode-IDE 가 담당하거나 이 제품에서 뺀 기능들이다.
//!
//! 턴 구동 순서는 zo-cli `LiveCli::run_turn_streaming_to_channel_with_prompter`
//! (serve 가 쓰던 장수 런타임 경로)를 그대로 따른다 — 그 경로가 이미 프로덕션
//! 에서 검증된 배선이기 때문이다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core_types::Session;
use runtime::message_stream::{AgentResultStatus, RenderBlock};
use runtime::{ConversationRuntime, HookAbortSignal, HookEvent, PermissionMode};

use super::agent_completion_pump::{
    register_for_surface, AgentCompletionPump,
};
use super::mcp_runtime::discover_pending_mcp_tools_in_background;
use super::stream::drive_render_stream;
use super::{BuiltRuntime, SessionHandle, TurnHarness};
use crate::cli_args::{AllowedToolSet, DisallowedToolSet};
use crate::effort::Effort;
use crate::autonomy::crons::CronTurn;
use crate::autonomy::loops::{LoopTurn, LoopTurnKind};
use crate::autonomy::runner::TurnReceipt;
use crate::autonomy::scheduler::now_unix_ms;
use crate::autonomy::Autonomy;
use crate::goal::{GoalCommandResult, GoalPhase, GoalTurn, GoalTurnReport};
use crate::runtime_support::{
    build_runtime_with_thinking,
    install_cli_runtime_overrides, CliRuntimeOverrides,
};
use crate::session_registry::{
    create_managed_session_handle_at, resolve_session_reference,
};

/// 시작 effort — zo-cli 와 동일(`High`).
pub const DEFAULT_EFFORT: Effort = Effort::High;

/// 내부 병렬 에이전트 툴. zerocode-IDE 의 오케스트레이션(원장·워커 패인)이 그
/// 역할을 맡으므로 이 프런트엔드에서는 기본으로 끈다.
pub const SPAWN_FAMILY_TOOLS: &[&str] = &["Agent", "SpawnMultiAgent", "Workflow", "Task"];

#[must_use]
pub fn is_spawn_family_tool(name: &str) -> bool {
    SPAWN_FAMILY_TOOLS.contains(&name)
}

const PERSISTENT_GOAL_REMINDER_PREFIX: &str = "[zo:persistent-goal]";
const DREAMER_LAST_PASS_REMINDER_PREFIX: &str = "[zo:dreamer-last-pass]";

/// `PlainSession::open` 입력.
pub struct OpenOptions {
    /// Accepted root --model selection; a teammate cannot create this authority.
    pub person_model_pin: Option<String>,
    pub cwd: PathBuf,
    pub model: String,
    pub permission_mode: PermissionMode,
    pub allowed_tools: Option<AllowedToolSet>,
    /// `true` 면 [`SPAWN_FAMILY_TOOLS`] 를 `--disallowedTools` 로 설치한다.
    pub disable_spawn_family: bool,
    /// 세션 id · `latest` · 트랜스크립트 경로. `None` = 새 세션.
    pub resume: Option<String>,
    pub mcp_config: Option<PathBuf>,
    pub effort: Option<Effort>,
    /// Nobody is at the keyboard: a pipe, a redirect, `--plain`.
    ///
    /// Picks the turn-discipline the system prompt carries. The interactive
    /// contract tells the model to stop and ask when it needs a decision; the
    /// autonomous one tells it the opposite, because there is no one to answer
    /// and a question is just a stalled run. `PromptMode::Autonomous` existed
    /// and NOTHING ever selected it — every zo run, piped or not, was told a
    /// human was watching.
    ///
    /// Decided at session creation and then frozen: the prompt is persisted
    /// beside the transcript and replayed verbatim on resume, because a byte
    /// that moves there invalidates the whole provider cache prefix.
    pub headless: bool,
    /// An exact launch contract accepted `model`/`effort` as spelled: the
    /// explicit model wins over the saved preference even when it spells the
    /// default, and nothing below re-chooses it (`crate::launch_contract`).
    /// Off for every legacy launch, whose preference fallback is unchanged.
    pub exact_selection: bool,
    /// A teammate's carried harness (t-2513 §2.1): the role prompt and rules
    /// its parent resolved, run as written instead of this process's own root
    /// prompt. `None` for every session a person opens.
    pub teammate_harness: Option<TeammateHarness>,
    /// The parent session's registry to open INSTEAD of one rooted here
    /// (t-2513 §2.5): stamps, reaping and lookups then land in the parent's
    /// root, as an inline child's do. `None` for a root session.
    pub parent_registry: Option<ParentRegistry>,
    /// The parent session's MCP tools and the channel that answers them
    /// (t-2513 §2.1): advertised as this session's runtime tools, dispatched
    /// over the parent's channel. `None` for a root session.
    pub remote_mcp: Option<runtime::subagent_panes::McpRoute>,
}

/// The half of a carried harness the session itself applies — the executor
/// takes the tool list through `allowed_tools`, the mode through
/// `permission_mode`; these two have no other door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeammateHarness {
    /// The role's system prompt, section by section, in place of the root
    /// prompt this process would build.
    pub system_prompt: Vec<String>,
    /// The carried local inspection shell policy.
    pub inspection_shell: bool,
    /// A custom agent's allow/deny/ask rules, added to the permission policy.
    pub permission_rules: Option<runtime::RuntimePermissionRuleConfig>,
}

/// Which registry a teammate opens: its parent's, found through the record
/// the brief named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentRegistry {
    pub session_id: String,
    pub locator: PathBuf,
}

/// 한 줄 상태(`/status`·IDE status 프레임 재료).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub model: String,
    pub permission_mode: &'static str,
    pub effort: Option<&'static str>,
    pub session_id: String,
    pub context_tokens: usize,
    pub goal: String,
    pub autonomous: String,
    pub loops: String,
    pub autonomy: crate::autonomy::AutonomyStatus,
}

pub struct PlainSession {
    selection_origins: (&'static str, &'static str),
    pub cwd: PathBuf,
    pub model: String,
    pub permission_mode: PermissionMode,
    allowed_tools: Option<AllowedToolSet>,
    system_prompt: Vec<String>,
    pub(crate) runtime: BuiltRuntime,
    tasks: Arc<runtime::task_registry::TaskRegistry>,
    pub handle: SessionHandle,
    /// WHERE this session's helper manifests live (t-2511): the store root
    /// fixed from the cwd the session was born in, the legacy mirrors adopted
    /// for it, and its roster generation. Shared with the tool context, every
    /// child job, the roster watcher and the events relay — the one handle
    /// they all take, so no reader re-derives a store from the process cwd.
    registry: Arc<tools::AgentRegistry>,
    effort: Option<Effort>,
    mcp_config: Option<PathBuf>,
    /// A teammate's road to its parent's MCP runtime, kept for the rebuilds a
    /// model or account change makes (t-2513 §2.1).
    remote_mcp: Option<runtime::subagent_panes::McpRoute>,
    /// 런치 플래그 그대로 — TUI 가 세션을 갈아 끼울 때(`/resume`) 다시 쓴다.
    disable_spawn_family: bool,
    resumed: bool,
    /// 트랜스크립트 옆 sidecar에 사는 host 진행 원장. 대화 압축과 수명이
    /// 분리돼 있고 `/resume`은 같은 session path로 이것까지 되찾는다.
    autonomy: Autonomy,
    /// 도는 도구를 끊는 신호. 호스트가 쥐고 있고 **매 턴** 런타임에 다시
    /// 심는다 — 런타임을 다시 세우면(모델·권한 전환) 심어둔 것이 함께
    /// 사라져서, 한 번만 심으면 Esc 가 조용히 다시 죽는다.
    tool_cancel: runtime::ToolCancelSignal,
    /// Registered while the session is built (outside Tokio), then moved into
    /// the interactive frontend's one consumer task once the runtime exists.
    /// Headless sessions keep this `None`, and therefore keep Agent blocking.
    agent_completion_receiver:
        Option<tokio::sync::mpsc::UnboundedReceiver<tools::AgentCompletion>>,
    /// Nobody at the keyboard — the launch's `headless` flag. Read on the way
    /// into every turn to decide who stops a turn that does not stop itself
    /// ([`runtime::Attendance`]): a headless run keeps the clock and token
    /// caps, a person's turn has none unless the env names one.
    headless: bool,
    /// What the scheduler did between turns — a wakeup adopted, a cron
    /// budget refused — for the frontend to print the next time it asks.
    /// The dispatch calls return the turn; this carries the words.
    autonomy_notices: Vec<String>,
    /// The turn's route facts for the interactive front (t-5872): what the
    /// routing judgment and the step governor decided, as they decide it.
    route_fact: super::route_fact::RouteFactSender,
}

/// `PlainSession::open` 에 들어간 런치 플래그의 되읽기 — `/resume` 으로 세션을
/// 교체할 때 argv 를 다시 파싱하지 않고 같은 조건으로 새 세션을 연다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFlags {
    pub allowed_tools: Option<AllowedToolSet>,
    pub disable_spawn_family: bool,
    pub mcp_config: Option<PathBuf>,
}

/// 재개 재생 재료 — 이전 대화를 화면 셀로 되돌리기 위한 최소 어휘.
/// codex 는 `replay_thread_turns` 로 이전 턴을 다시 그린다; 그 입력이 이것이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayItem {
    /// 사람이 보낸 프롬프트 원문.
    User(String),
    /// 어시스턴트 본문(한 메시지의 Text 블록을 이어 붙인 것).
    Assistant(String),
    /// 영속된 모델용 완료 알림을 다시 사람용 결과 카드로 투영한 것.
    AgentResult {
        label: String,
        status: AgentResultStatus,
        /// `12 tool uses · 45.3k tokens · 1m 20s`, recovered from the stored
        /// notification header so a resumed card is not poorer than the live
        /// one. `None` for a header written before the cost was carried.
        summary: Option<String>,
        body: String,
    },
    /// 도구 호출 — 결과가 이어졌으면 `output`/`is_error` 가 채워진다.
    ToolCall {
        name: String,
        input: String,
        output: Option<String>,
        is_error: bool,
    },
}

/// 트랜스크립트 메시지 → 재생 항목. System 메시지와 thinking 은 화면에 없던
/// 것이므로 뺀다. `ToolResult` 는 `tool_use_id` 로 앞선 호출에 붙인다.
#[must_use]
pub fn replay_items_from(messages: &[core_types::session::ConversationMessage]) -> Vec<ReplayItem> {
    use core_types::session::{ContentBlock, MessageRole};
    let mut items: Vec<ReplayItem> = Vec::new();
    let mut call_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for message in messages {
        match message.role {
            MessageRole::System => {}
            MessageRole::User | MessageRole::Tool => {
                let mut text = String::new();
                for block in &message.blocks {
                    match block {
                        ContentBlock::Text { text: t } => {
                            if let Some((label, status, summary, body)) =
                                crate::tui::tools::persisted_agent_result(t)
                            {
                                if !text.trim().is_empty() {
                                    items.push(ReplayItem::User(std::mem::take(&mut text)));
                                }
                                items.push(ReplayItem::AgentResult {
                                    label,
                                    status,
                                    summary,
                                    body,
                                });
                            } else {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(t);
                            }
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            output,
                            is_error,
                            ..
                        } => {
                            if let Some(index) = call_index.get(tool_use_id) {
                                if let Some(ReplayItem::ToolCall {
                                    output: slot,
                                    is_error: error_slot,
                                    ..
                                }) = items.get_mut(*index)
                                {
                                    *slot = Some(output.clone());
                                    *error_slot = *is_error;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !text.trim().is_empty() {
                    items.push(ReplayItem::User(text));
                }
            }
            MessageRole::Assistant => {
                let mut text = String::new();
                for block in &message.blocks {
                    match block {
                        ContentBlock::Text { text: t }
                            if runtime::is_reasoning_passport_text(t) => {}
                        ContentBlock::Text { text: t } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            if !text.trim().is_empty() {
                                items.push(ReplayItem::Assistant(std::mem::take(&mut text)));
                            }
                            call_index.insert(id.clone(), items.len());
                            items.push(ReplayItem::ToolCall {
                                name: name.clone(),
                                input: input.clone(),
                                output: None,
                                is_error: false,
                            });
                        }
                        _ => {}
                    }
                }
                if !text.trim().is_empty() {
                    items.push(ReplayItem::Assistant(text));
                }
            }
        }
    }
    items
}

/// User-authored transcript text used to seed the interactive composer when a
/// session is resumed.
#[must_use]
pub fn user_input_history_from(
    messages: &[core_types::session::ConversationMessage],
) -> Vec<String> {
    use core_types::session::{ContentBlock, MessageRole};

    messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .filter_map(|message| {
            let text = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let text = text.trim();
            if text.is_empty()
                || text.starts_with(runtime::STEERING_ECHO_PREFIX)
                || crate::tui::tools::persisted_agent_result(text).is_some()
            {
                None
            } else {
                Some(text.to_string())
            }
        })
        .collect()
}

/// The project's home under `~/.zo/projects/<slug>` — the sessions directory's
/// parent, read off the handle so there is one source of that path.
fn project_home_of(session_path: &Path) -> Option<PathBuf> {
    session_path.parent()?.parent().map(Path::to_path_buf)
}

/// Artifacts and the team inbox land beside the session, not in the working
/// tree. The tools crates read these two variables; unset, they fell back to
/// `.zo/` under cwd and dirtied the person's `git status` on every run
/// (6/6 in the 2026-09-02 benchmark). An operator's explicit value wins.
fn keep_stores_out_of_the_tree(session_path: &Path) {
    let Some(home) = project_home_of(session_path) else {
        return;
    };
    for (key, dir) in [
        (tools::ARTIFACT_STORE_ENV, "artifacts"),
        (tools::TEAM_INBOX_STORE_ENV, "team_inbox"),
    ] {
        if std::env::var_os(key).is_some_and(|value| !value.is_empty()) {
            continue;
        }
        std::env::set_var(key, home.join(dir));
    }
}

/// The catalog first: the persisted model is an alias, and an alias resolves
/// against whatever the bridge has published — so discovery's rows must be
/// live before the alias is read.
fn preferences_behind_the_catalog() -> crate::preferences::Preferences {
    // A session start is a connection: the quiet sources are asked again.
    crate::runtime_support::connect_model_catalog();
    match crate::preferences::pin_persisted_model_alias() {
        Ok(Some(migration)) => crate::runtime_support::push_catalog_notice(format!(
            "settings: model {} → {} (pinned to the family alias; it now follows the family)",
            migration.from, migration.to
        )),
        Ok(None) => {}
        Err(error) => eprintln!("[zo] could not pin the persisted model alias: {error}"),
    }
    crate::preferences::load()
}

/// Over-cap tool results are folded by the provider's fast model unless the
/// `toolDigest` setting or `ZO_TOOL_DIGEST` says off (the env wins). The
/// process default is off, so only a session that booted here ever dials the
/// digester — never a unit test or a hermetic harness by accident.
fn configure_tool_digest_from(preferences: &crate::preferences::Preferences) {
    tools::configure_tool_digest(tools::ToolDigestMode::from_setting(
        std::env::var(tools::TOOL_DIGEST_ENV)
            .ok()
            .as_deref()
            .or(preferences.tool_digest.as_deref()),
    ));
}

/// `ide::args` supplies `DEFAULT_MODEL` when `--model` was absent. That sentinel
/// lets us honor a saved choice without touching the forbidden argument
/// parser; any non-default CLI model remains authoritative.
fn chosen_model(cli_model: &str, persisted: Option<String>) -> String {
    if cli_model == crate::DEFAULT_MODEL {
        persisted.unwrap_or_else(|| cli_model.to_string())
    } else {
        cli_model.to_string()
    }
}

impl PlainSession {
    /// 세션을 열고(새로 또는 `--resume`) 런타임을 빌드한다.
    #[expect(
        clippy::too_many_lines,
        reason = "session construction is one ordered lifecycle and only crossed the lint by loading autonomy sidecars"
    )]
    pub fn open(options: OpenOptions) -> Result<Self, Box<dyn std::error::Error>> {
        // 시작 비용은 `ZO_PROFILE_BOOT=1` 한 채널로 나간다 — 프로파일러가
        // 둘이면 어느 쪽이 진실인지 다시 재야 한다.
        let opened = std::time::Instant::now();
        let persisted_preferences = preferences_behind_the_catalog();
        configure_tool_digest_from(&persisted_preferences);
        crate::runtime_support::log_boot_stage("open/preferences", opened);
        let cwd = options.cwd;
        let (mut session, handle, resumed) = if let Some(reference) = options.resume.as_deref() {
            let handle = resolve_session_reference(reference)?;
            let mut session = Session::load_from_path(&handle.path)?;
            handle.id.clone_into(&mut session.session_id);
            (session, handle, true)
        } else {
            let session = Session::new();
            let handle = create_managed_session_handle_at(&session.session_id, &cwd)?;
            (session, handle, false)
        };
        // Process diagnostics are transcript records so a dead pane leaves its
        // cause beside the conversation. They are host metadata, not model
        // context: consume them before a resumed session builds its request.
        super::process_lifecycle::strip_process_events(&mut session);
        // Session transcripts do not carry their workspace path. Keep a tiny
        // sidecar for the public resume picker; old sessions simply render an
        // explicit "not recorded" location.
        let _ = crate::resume::write_session_cwd_if_missing(&handle.path, &cwd);
        keep_stores_out_of_the_tree(&handle.path);
        // The session's agent registry: rooted at the cwd it was BORN in — the
        // `.cwd` sidecar for a resumed session, this launch's cwd for a new
        // one — and found again through the locator its own sidecar names,
        // so a `/resume` from another directory keeps its children.
        let origin_cwd = if resumed {
            crate::resume::load_session_cwd(&handle.path).unwrap_or_else(|| cwd.clone())
        } else {
            cwd.clone()
        };
        let locator_hint = crate::resume::load_session_registry_locator(&handle.path);
        // A teammate opens its PARENT's registry (t-2513 §2.5) — the record
        // the brief named, under the parent's session id — so what it stamps
        // and reaps lands where the parent reads. A root session opens its own.
        let registry = match options.parent_registry.as_ref() {
            Some(parent) => tools::AgentRegistry::open_for_session(
                &parent.session_id,
                &origin_cwd,
                Some(&parent.locator),
                false,
            ),
            None => tools::AgentRegistry::open_for_session(
                &handle.id,
                &origin_cwd,
                locator_hint.as_deref(),
                resumed,
            ),
        }
        .map_err(|error| format!("startup stage agent-registry: {error}"))?;
        if let Some(locator) = registry.locator() {
            let _ = crate::resume::write_session_registry_locator(&handle.path, locator);
        }
        // From here the cwd fallback is a bug in this process (debug builds
        // assert it): every agent road below carries this handle.
        tools::mark_session_process();
        crate::runtime_support::spawn_orphaned_agent_reap(&registry);

        if options.disable_spawn_family {
            let disallowed: DisallowedToolSet = SPAWN_FAMILY_TOOLS
                .iter()
                .map(|name| (*name).to_string())
                .collect();
            install_cli_runtime_overrides(CliRuntimeOverrides {
                disallowed_tools: Some(disallowed),
                ..CliRuntimeOverrides::default()
            });
        }

        let preferences_selected = !options.exact_selection && options.model == crate::DEFAULT_MODEL && persisted_preferences.model.is_some();
        let selection_origins = (
            if preferences_selected { "preferences" } else { "launch" },
            if options.effort.is_some() { "launch" } else if persisted_preferences.effort.is_some() { "preferences" } else { "default" },
        );
        let model = if options.exact_selection {
            options.model.clone()
        } else {
            chosen_model(&options.model, persisted_preferences.model)
        };
        let prompt_started = std::time::Instant::now();
        crate::runtime_support::log_boot_stage("open/session-handle", opened);
        // A teammate runs the prompt its parent resolved, as written — never
        // this process's root prompt (t-2513 contract 1: re-derivation is
        // drift). A resumed teammate replays the stored one, like any session.
        let system_prompt = match options.teammate_harness.as_ref() {
            Some(harness) if !harness.system_prompt.is_empty() => {
                carried_system_prompt(&handle.path, resumed, &harness.system_prompt)
            }
            _ => session_system_prompt(
                &cwd,
                &model,
                &handle.path,
                resumed,
                if options.headless {
                    runtime::PromptMode::Autonomous
                } else {
                    runtime::PromptMode::Interactive
                },
            )?,
        };
        crate::runtime_support::log_boot_stage("open/system-prompt", prompt_started);
        let runtime_started = std::time::Instant::now();
        let effort = options
            .effort
            .or(persisted_preferences.effort)
            .or(Some(DEFAULT_EFFORT));
        let tasks = Arc::new(runtime::task_registry::TaskRegistry::for_session(&runtime::zo_project_state_dir(registry.origin_cwd()), &handle.id)?);

        let persisted_goal = session.session_goal.clone();
        let had_goal_reminder = session_has_goal_reminder(&session);
        let mut runtime = build_runtime(
            &cwd,
            options.mcp_config.as_ref(),
            session.with_persistence_path(handle.path.clone()),
            &handle.id,
            model.clone(),
            system_prompt.clone(),
            options.allowed_tools.clone(),
            options.permission_mode,
            thinking_config_for(effort),
            effort.and_then(Effort::level),
            effort.and_then(Effort::band_ceiling),
            tasks.as_ref().clone(),
            options.remote_mcp.clone(),
        )?;
        crate::runtime_support::log_boot_stage("open/runtime", runtime_started);
        install_agent_registry(&runtime, &registry);
        if let Some(inner) = runtime.runtime.as_ref() {
            inner.tool_executor().tool_registry().context().set_person_model_pin(options.person_model_pin.as_deref()
                .or_else(|| (preferences_selected && options.parent_registry.is_none() && options.teammate_harness.is_none()).then_some(model.as_str())));
        }
        if let Some(inner) = runtime.runtime.as_ref() {
            inner.tool_executor().tool_registry().context().set_inspection_shell(
                options.teammate_harness.as_ref().is_some_and(|harness| harness.inspection_shell));
        }
        // The carried rules join the policy the session was built with — the
        // same `with_permission_rules` an inline child's enforcer gets.
        if let Some(rules) = options
            .teammate_harness
            .as_ref()
            .and_then(|harness| harness.permission_rules.as_ref())
        {
            if let Some(inner) = runtime.try_runtime_mut() {
                inner.add_permission_rules(rules);
            }
        }
        install_headless_task_delivery(&runtime, &tasks, options.headless, &handle.id);
        runtime.set_autonomous_surface(false);
        apply_active_model(&mut runtime, &model);
        // The keep-selected rule: a refresh that no longer lists this model
        // keeps its row instead of erasing the session's choice (t-3054).
        runtime::model_discovery::note_selected(&model);
        install_fork_source(&runtime);
        let autonomy = Autonomy::load(&handle.path, persisted_goal.as_deref(), resumed);
        sync_goal_reminder(
            &mut runtime,
            autonomy.goal.reminder_state(),
            had_goal_reminder,
        );
        sync_dreamer_model_reminder(&mut runtime, crate::dream::last_pass_line().as_deref());
        let agent_completion_receiver = register_for_surface(options.headless);

        let session = Self {
            selection_origins,
            cwd,
            model,
            permission_mode: options.permission_mode,
            allowed_tools: options.allowed_tools,
            system_prompt,
            runtime,
            tasks,
            handle,
            registry,
            effort,
            mcp_config: options.mcp_config,
            remote_mcp: options.remote_mcp,
            disable_spawn_family: options.disable_spawn_family,
            resumed,
            autonomy,
            tool_cancel: runtime::ToolCancelSignal::new(),
            agent_completion_receiver,
            headless: options.headless,
            autonomy_notices: Vec::new(),
            route_fact: super::route_fact::channel(),
        };
        // A new session has no file yet, so this publishes its initial
        // snapshot. A resumed session is already persistence-bound and clean;
        // append-aware persistence therefore does no I/O. In particular, the
        // provider-only process-event filter above must not rewrite the source
        // transcript or contaminate its resume-list mtime.
        session
            .runtime
            .session()
            .persist_appended_state_to_path(&session.handle.path)?;
        super::process_lifecycle::track_session(&session.handle.id, &session.handle.path);
        crate::runtime_support::log_boot_stage("open/total", opened);
        Ok(session)
    }

    #[must_use]
    pub fn selection_origins(&self) -> (&'static str, &'static str) {
        self.selection_origins
    }

    #[must_use]
    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// The session's agent registry (see the field).
    #[must_use]
    pub(crate) fn registry(&self) -> Arc<tools::AgentRegistry> {
        Arc::clone(&self.registry)
    }

    #[must_use]
    pub fn effort(&self) -> Option<Effort> {
        self.effort
    }

    /// The front's end of this session's route facts (t-5872).
    #[must_use]
    pub(crate) fn route_fact_receiver(&self) -> super::route_fact::RouteFactReceiver {
        self.route_fact.subscribe()
    }

    /// 도는 도구를 끊는 신호의 사본. 스캐폴드가 이걸 쥐고 있다가 Esc 에 쏜다.
    ///
    /// 스티어 큐와 달리 런타임이 없어도 `None` 이 아니다 — 신호는 호스트가
    /// 소유하고 런타임은 매 턴 사본을 받아 가는 쪽이라, 런타임이 아직
    /// 없더라도 손잡이는 이미 진짜다.
    #[must_use]
    pub fn tool_cancel_handle(&self) -> runtime::ToolCancelSignal {
        self.tool_cancel.clone()
    }

    /// 턴 중 스티어 큐(사용자가 입력한 줄을 다음 툴 경계에서 모델에 전달).
    #[must_use]
    pub fn steering_handle(&self) -> Option<runtime::SteeringQueue> {
        self.runtime
            .runtime
            .as_ref()
            .map(ConversationRuntime::steering_handle)
    }

    /// Start the one interactive completion consumer after the Tokio runtime
    /// is live. Registration itself happened in [`Self::open`], so completions
    /// that race frontend startup wait in the unbounded channel rather than
    /// falling through a channel-less host.
    pub(crate) fn start_agent_completion_pump(&mut self) -> Option<AgentCompletionPump> {
        self.agent_completion_receiver
            .take()
            .map(|receiver| AgentCompletionPump::spawn(receiver, self.handle.id.clone()))
    }

    /// Current runtime's mid-turn completion inbox. Model/permission rebuilds
    /// replace the runtime, so the frontend reads this afresh at every turn.
    pub(crate) fn agent_notification_inbox(&self) -> Option<runtime::AgentNotificationInbox> {
        self.runtime
            .runtime
            .as_ref()
            .map(ConversationRuntime::agent_notification_inbox)
    }

    /// 스트리밍 턴 하나. 블록은 `block_tx` 로 흘러가고, 권한 프롬프트는
    /// `prompter` 가 답한다. Ctrl-C 는 호출자가 `user_cancel_requested` 를
    /// 세우고 `hook_abort_signal.abort()` 를 부르는 것으로 전달한다.
    pub(crate) async fn run_turn(
        &mut self,
        input: &str,
        block_tx: tokio::sync::mpsc::Sender<RenderBlock>,
        prompter: std::sync::Arc<dyn runtime::permission::PermissionPrompter>,
        hook_abort_signal: HookAbortSignal,
        user_cancel_requested: Arc<AtomicBool>,
    ) -> Result<runtime::TurnSummary, String> {
        self.run_turn_with_images(
            input,
            Vec::new(),
            block_tx,
            prompter,
            hook_abort_signal,
            user_cancel_requested,
        )
        .await
    }

    /// 턴 앞의 자격 손질([`crate::runtime_support::refresh_oauth_if_near_expiry`]).
    /// 로그인 없이 뜬 프로세스는 여기서 로그인을 다시 찾고, 찾았는지를 상태 줄
    /// 한 줄로 말한다(t-6248 C3) — 한 번의 무조건 찾기는 사람의 턴이 쓴다.
    async fn refresh_credentials_for_turn(&mut self, block_tx: &tokio::sync::mpsc::Sender<RenderBlock>) {
        let Some(inner) = self.runtime.try_runtime_mut() else {
            return;
        };
        let persons_turn = !inner.is_autonomous_surface() && !crate::autonomy::wakeup::scope_active();
        let Some(said) =
            crate::runtime_support::refresh_oauth_if_near_expiry(inner.api_client_mut(), persons_turn).await
        else {
            return;
        };
        let _ = block_tx
            .send(RenderBlock::System {
                id: runtime::message_stream::BlockIdGen::default().next(),
                level: runtime::message_stream::SystemLevel::Housekeeping,
                text: said,
            })
            .await;
    }

    /// 스트리밍 턴 하나를 이미지 첨부와 함께 실행한다. 이미지 쌍은 엔진의
    /// `push_user_with_images` 입력과 같은 `(media_type, base64)` 모양이다.
    pub(crate) async fn run_turn_with_images(
        &mut self,
        input: &str,
        images: Vec<(String, String)>,
        block_tx: tokio::sync::mpsc::Sender<RenderBlock>,
        prompter: std::sync::Arc<dyn runtime::permission::PermissionPrompter>,
        hook_abort_signal: HookAbortSignal,
        user_cancel_requested: Arc<AtomicBool>,
    ) -> Result<runtime::TurnSummary, String> {
        self.runtime
            .set_hook_abort_signal(hook_abort_signal.clone());
        if let Some(mcp_state) = self.runtime.mcp_state.as_ref() {
            discover_pending_mcp_tools_in_background(
                mcp_state,
                self.runtime.api_client().tool_registry(),
            );
        }
        // 저장된 OAuth 토큰이 만료 버퍼 안이면 여기서 갱신한다. `build_live_client`
        // 가 bearer 를 복제해 가므로 그 **전**이어야 이번 턴이 새 토큰을 쓴다 —
        // 이 자리를 잃으면 긴 세션이 턴 도중 401 로 죽는다.
        self.refresh_credentials_for_turn(&block_tx).await;
        let turn_setup = TurnHarness::setup_model_led_turn(&mut self.runtime, input, true);
        let named_effort = self.effort.and_then(Effort::level);
        let effort_band_ceiling = self.effort.and_then(Effort::band_ceiling);
        let live_client = TurnHarness::build_live_client(
            &self.runtime,
            self.allowed_tools.clone(),
            thinking_config_for(self.effort),
            named_effort,
            effort_band_ceiling,
        );
        let restore_reactive_gate =
            TurnHarness::install_reactive_verify_gate_if_coding(input, &mut self.runtime);
        // Where this turn's messages begin, for the labels written at its end
        // (t-5806): everything the turn appends stands after this index.
        let turn_from = self
            .runtime
            .try_runtime()
            .map_or(0, |inner| inner.session().messages.len());
        let installed = super::smart_runtime::install_smart_turn(
            &mut self.runtime,
            &self.cwd,
            &self.handle.id,
            self.allowed_tools.as_ref(),
            super::smart_runtime::SmartTurnInput {
                input,
                assessment: turn_setup.assessment,
                turn_effort: (named_effort, effort_band_ceiling),
                route_fact: &self.route_fact,
            },
        );
        self.arm_turn_limits();
        // 난이도가 넓다고 하면 호스트가 먼저 갈라 읽는다(`orchestration`): 결과는
        // 이 턴의 문맥에 앉고, 모델은 그 위에서 시작한다. 예산·출석 선언 뒤라
        // 헬퍼도 같은 한도를 받는다.
        let input = self
            .input_after_host_prelude(installed.host_turn(), &turn_setup, input, &block_tx, || {
                user_cancel_requested.load(Ordering::SeqCst) || hook_abort_signal.is_aborted()
            })
            .await;
        let model = self.model.clone();
        let result = match self.runtime.runtime.as_mut() {
            Some(rt) => {
                let completed = {
                    let turn = drive_render_stream(
                        rt,
                        live_client,
                        input,
                        images,
                        &model,
                        block_tx,
                        prompter,
                    );
                    tokio::pin!(turn);
                    tokio::select! {
                        biased;
                        result = &mut turn => Some(result),
                        () = async {
                            while !hook_abort_signal.is_aborted() {
                                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                            }
                        } => None,
                    }
                };
                if user_cancel_requested.load(Ordering::SeqCst) {
                    Err(rt
                        .cancel_streaming_turn_by_user("turn cancelled by user")
                        .to_string())
                } else if hook_abort_signal.is_aborted() {
                    Err(rt
                        .cancel_streaming_turn_by_host("turn aborted by host")
                        .to_string())
                } else {
                    completed.expect("turn completed when no stop signal is set")
                }
            }
            None => Err("runtime not available".to_string()),
        };
        TurnHarness::restore_deep_gate(&mut self.runtime, restore_reactive_gate);
        // The Jev seats' labels for this turn (t-5806): whether the route the
        // routing seat took part in stood, and whether the note the recall
        // seat put first was read. Judged on what is already in memory — the
        // switch the observer saw, and the messages this turn appended — and
        // not on a cancelled turn, which says nothing about either.
        self.label_jev_seats(
            installed.route_watch.taken(), turn_from,
            user_cancel_requested.load(Ordering::SeqCst) || hook_abort_signal.is_aborted(),
        );
        let summary = result?;
        self.persist().map_err(|error| error.to_string())?;
        Ok(summary)
    }

    /// Who stops this turn if it does not stop itself, and what cuts a tool
    /// that will not end — armed every turn, because the runtime is rebuilt
    /// on a model or permission switch and takes the last turn's copies with
    /// it. Its own method so the turn's body stays readable (clippy's line
    /// budget), not because any of it is reused.
    fn arm_turn_limits(&mut self) {
    if let Some(inner) = self.runtime.try_runtime_mut() {
        // Who stops this turn if it does not stop itself. A person at the
        // keyboard has Esc — the one breaker Claude Code and Codex run
        // with — so their turn sets no clock and no token cap unless the
        // env names one. A headless run, or an autonomous turn the goal
        // controller or the window drives, has nobody to press it and
        // keeps the safety net.
        let attendance = if self.headless || inner.is_autonomous_surface() {
            runtime::Attendance::Unattended
        } else {
            runtime::Attendance::Attended
        };
        inner.set_attendance(attendance);
        // And for the parts of the process with no runtime to ask — the
        // caps a spawned helper is given on its own thread.
        runtime::declare_attendance(attendance);
        let (deadline, output_budget, input_budget) = runtime::env_turn_budgets(attendance);
        match deadline {
            Some(budget) => inner.set_deadline(std::time::Instant::now() + budget),
            None => inner.clear_deadline(),
        }
        inner.set_turn_output_token_budget(output_budget);
        inner.set_turn_input_token_budget(input_budget);
        // Arm the progress-gated deadline extension.
        //
        // The policy, its defaults (2 pushes of 30 minutes) and its env
        // overrides all existed, and the turn loop's consumer is careful —
        // it extends only on FRESH progress (reads and probes do not
        // count), caps the count, and says so on screen. Nothing ever
        // armed it, so `deadline_extension` stayed `None` and the blunt
        // 60-minute cut its own comment blames for "interrupting the
        // legitimate long audit or deploy pipeline" was still the whole
        // policy.
        //
        // Here, beside the deadline it extends, and re-read every turn for
        // the same reason `set_deadline` is: an env change takes effect on
        // the next turn rather than at next launch.
        //
        // Only this host. A spawned sub-agent keeps its deadline as a hard
        // straggler bound — nobody is watching it to notice a grind.
        inner.set_deadline_extension(runtime::env_deadline_extension());
        // 도는 도구를 끊는 신호를 심는다.
        //
        // 런타임은 이걸 `await_cancellable_tool_dispatch` 의 `select!` 두
        // 팔 중 하나로 지켜보고 있었다: 도구 완료 아니면 이 신호. 그런데
        // 아무도 신호를 쥔 적이 없어서, 도구가 떠 있는 동안 Esc 가 세우는
        // 깃발 둘(`hook_abort`·`cancel`)을 그 select 는 읽지 않았다 —
        // 화면엔 "interrupted" 가 찍히고 턴은 계속 돌았다. 물린 MCP·bash
        // 호출이면 영영.
        //
        // 예산 옆에서 매 턴 다시 심는 이유는 `set_deadline` 과 같다: 이
        // 런타임은 모델·권한 전환에 다시 세워지고, 그때 심어둔 사본은
        // 함께 사라진다.
        inner.set_tool_cancel_signal(self.tool_cancel.clone());
    }    }

    /// Write the routing and recall seats' `agreed` marks for the turn that
    /// just ended: the route stood unless `unseated` says which door moved
    /// the wire, and the recall's first note was read or cited in the
    /// messages from `turn_from` on. A session compaction may have shrunk the
    /// transcript under that index; the turn is then read from its own user
    /// message, the last one the session holds.
    fn label_jev_seats(&self, unseated: Option<runtime::SwitchTrigger>, turn_from: usize, cancelled: bool) {
        let Some(inner) = self.runtime.try_runtime() else {
            return;
        };
        let attempt = inner.attempt().to_string();
        if cancelled {
            let _ = tools::note_recall_read(&self.cwd, &attempt, None);
            let _ = tools::note_compaction_reread(&self.cwd, None);
            let _ = tools::note_patch_review_turn(&self.cwd, None);
            return;
        }
        let messages = Arc::clone(&inner.session().messages);
        let from = if turn_from <= messages.len() {
            turn_from
        } else {
            messages
                .iter()
                .rposition(|message| message.role == core_types::MessageRole::User)
                .unwrap_or(0)
        };
        // Whether a row was written is the ledger's business, not the turn's.
        let _ = tools::note_route_followed(&self.cwd, &attempt, unseated);
        let _ = tools::note_recall_read(&self.cwd, &attempt, Some(&messages[from..]));
        // And the compaction seat's: whether a block a compaction dropped was
        // read again inside its window (t-6039).
        let _ = tools::note_compaction_reread(&self.cwd, Some(&messages[from..]));
        // And the patch review seat's: whether a patch's lines were edited
        // again, or a check ran green after the turn's last edit (t-6203).
        let _ = tools::note_patch_review_turn(&self.cwd, Some(&messages[from..]));
    }

    /// 턴 후 영속 — 메시지는 이미 append 됐고, 헤더/압축 변경만 스냅샷.
    pub fn persist(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime
            .session()
            .persist_appended_state_to_path(&self.handle.path)?;
        Ok(())
    }

    /// Give a freshly opened chat the optional display name accepted by
    /// Codex's `/new <name>` and `/clear <name>` forms.
    pub fn set_name(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let name = name.trim();
        if name.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "session name cannot be empty",
            )
            .into());
        }
        let runtime = self
            .runtime
            .try_runtime_mut()
            .ok_or_else(|| std::io::Error::other("runtime not available"))?;
        runtime.session_mut().name = Some(name.to_string());
        self.persist()
    }

    pub fn fire_session_start(&self) {
        // 사용량 프로브를 한 번 깨운다. 배경으로 돌고 결과는 엔진 슬롯에 앉으므로
        // 부팅은 이것을 기다리지 않는다 — 첫 `/status` 가 빈 카드 대신 측정값을
        // 만나는 것이 목적이다.
        //
        // `open` 이 아니라 여기인 이유: `main` 은 tokio 런타임을 **세션을 연
        // 뒤에** 세운다(런타임 빌드가 ambient 런타임 안에서 패닉하기 때문). 프로브
        // 예약은 현재 런타임 핸들이 없으면 조용히 자기 요청을 버리므로, `open`
        // 에서 부른 것은 한 번도 나가지 않았을 것이다. 두 프런트엔드가 다 런타임
        // 안에서 부르는 지점이 이 훅이다.
        crate::usage::refresh_soon();
        let payload = serde_json::json!({
            "source": if self.resumed { "resume" } else { "startup" },
            "cwd": self.cwd.to_string_lossy(),
            "session_id": self.handle.id,
        });
        self.runtime
            .fire_lifecycle_hook(HookEvent::SessionStart, &payload);
    }

    pub fn fire_session_end(&mut self, reason: &str) {
        // The process-global completion producer may be re-registered by a
        // `/resume` in this same process. Retire only this conversation's
        // delivery claims before that next consumer can interpret a late
        // worker stop as its own follow-up turn; workers/store entries remain.
        let _ = tools::clear_background_completion_marks_for_session(&self.registry, &self.handle.id);
        let granted_rules = self
            .runtime
            .runtime
            .as_mut()
            .map(ConversationRuntime::take_granted_permission_rules)
            .unwrap_or_default();
        let _ = runtime::persist_allow_always_rules(&self.cwd, &granted_rules);

        if let Some(runtime) = self.runtime.try_runtime_mut() {
            if let Err(error) = super::process_lifecycle::record_session_end(
                runtime.session_mut(),
                &self.handle.id,
                reason,
            ) {
                eprintln!("zo: could not record session exit reason={reason}: {error}");
            }
        }

        let payload = serde_json::json!({
            "session_id": self.handle.id,
            "reason": reason,
        });
        self.runtime
            .fire_lifecycle_hook(HookEvent::SessionEnd, &payload);
        let _ = super::dreamer_hook::run_on_session_end(&self.cwd, &self.handle.id, &self.model);
    }

    /// Persist a recoverable host failure without turning it into model
    /// context on a later resume.
    pub(crate) fn record_process_event(&mut self, kind: &str, reason: &str) {
        let Some(runtime) = self.runtime.try_runtime_mut() else {
            eprintln!("zo: process event kind={kind} could not be recorded (runtime unavailable)");
            return;
        };
        if let Err(error) = super::process_lifecycle::record_event(
            runtime.session_mut(),
            &self.handle.id,
            kind,
            reason,
        ) {
            eprintln!("zo: could not record process event kind={kind}: {error}");
        }
    }

    /// `/model` — 같은 세션을 들고 런타임을 재빌드한다(제공자 클라이언트가
    /// 모델에 묶여 있어 라이브 교체가 불가능하다).
    pub fn set_model(&mut self, model: &str) -> Result<(), Box<dyn std::error::Error>> {
        let model = crate::cli_args::resolve_model_alias(model);
        runtime::model_discovery::note_selected(&model);
        // The person's switch, scored the way any other switch is: what the
        // scorer would have done from here and what the new model rewrites.
        // Log-only, and before the runtime it reads is rebuilt.
        super::smart_runtime::record_person_model_switch(
            &self.runtime,
            &self.cwd,
            &self.handle.id,
            &model,
            (
                self.effort.and_then(Effort::level),
                self.effort.and_then(Effort::band_ceiling),
            ),
        );
        let mut system_prompt = self.system_prompt.clone();
        runtime::retarget_prompt_model(&mut system_prompt, &model);
        let mut session = self.runtime.session().clone();
        // Mark the seam in the transcript BEFORE the new runtime takes over, so
        // the note lands where the switch actually happened. Append-only, so
        // the cached prefix behind it is untouched, and it rides a resume like
        // any other message.
        if let Some(note) = runtime::model_handoff_notice(&self.model, &model) {
            if let Err(error) = session.push_message(runtime::ConversationMessage {
                role: runtime::MessageRole::System,
                blocks: vec![runtime::ContentBlock::Text { text: note }],
                usage: None,
                thought_signature: None,
                reasoning_replay: None,
                model: None,
            }) {
                eprintln!("zo: could not record the model handoff ({error})");
            }
        }
        let had_goal_reminder = session_has_goal_reminder(&session);
        let mut new_runtime = build_runtime(
            &self.cwd,
            self.mcp_config.as_ref(),
            session,
            &self.handle.id,
            model.clone(),
            system_prompt.clone(),
            self.allowed_tools.clone(),
            self.permission_mode,
            thinking_config_for(self.effort),
            self.effort.and_then(Effort::level),
            self.effort.and_then(Effort::band_ceiling),
            self.tasks.as_ref().clone(),
            self.remote_mcp.clone(),
        )?;
        install_agent_registry(&new_runtime, &self.registry);
        new_runtime.set_autonomous_surface(false);
        apply_active_model(&mut new_runtime, &model);
        install_fork_source(&new_runtime);
        sync_goal_reminder(
            &mut new_runtime,
            self.autonomy.goal.reminder_state(),
            had_goal_reminder,
        );
        self.replace_runtime(new_runtime)?;
        self.model = model;
        self.system_prompt = system_prompt;
        self.publish_selection();
        self.persist_preferences()?;
        self.persist()
    }

    /// The shared cell the tool registry reads for the session's permission
    /// mode.
    ///
    /// The screen keeps a clone so a mode change reaches a **running** turn.
    /// The turn owns the session (and with it the approval policy, which needs
    /// `&mut`), but this cell lives behind an `Arc` that every registry clone
    /// shares — so widening the mode mid-turn immediately lifts the file-tool
    /// workspace boundary even though the approval policy itself only swaps
    /// when the turn ends. Two seams, and only one of them is live; the screen
    /// says so rather than implying the whole switch landed.
    #[must_use]
    pub fn permission_cell(&self) -> std::sync::Arc<std::sync::Mutex<Option<PermissionMode>>> {
        std::sync::Arc::clone(
            &self
                .runtime
                .api_client()
                .tool_registry()
                .context()
                .session_permission_mode,
        )
    }

    /// `/permissions` — 라이브 전환(재빌드 없음).
    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        if let Some(inner) = self.runtime.try_runtime_mut() {
            inner.set_active_permission_mode(mode);
        }
        self.permission_mode = mode;
    }

    /// `/goal` command surface. Goal text is mirrored into the runtime session
    /// header after every mutation; the controller sidecar owns only progress.
    pub fn goal_command(&mut self, raw: &str) -> Result<GoalCommandResult, String> {
        let first = raw.split_whitespace().next().unwrap_or_default();
        let fingerprint = matches!(first, "verify" | "check")
            .then(|| crate::goal::workspace_fingerprint(&self.cwd).ok())
            .flatten();
        let result = self.autonomy.goal.apply(raw, fingerprint.as_deref())?;
        if let Some(inner) = self.runtime.try_runtime_mut() {
            inner.set_session_goal(self.autonomy.goal.objective().map(str::to_string));
        }
        self.sync_goal_reminder();
        self.persist().map_err(|error| error.to_string())?;
        Ok(result)
    }

    #[must_use]
    pub fn goal_status_report(&self) -> String {
        self.autonomy.goal.status_report()
    }

    #[must_use]
    pub fn goal_running(&self) -> bool {
        self.autonomy.goal.is_running()
    }

    pub fn dispatch_goal_turn(&mut self) -> Result<Option<GoalTurn>, String> {
        if let Some(reason) = self.autonomy.exhaustion() {
            self.autonomy.goal.pause_for_session_limit(&reason)?;
            self.autonomy.loops.pause_for_session_limit(&reason)?;
            self.sync_goal_reminder();
            return Ok(None);
        }
        let result = self.autonomy.goal.dispatch_next();
        if result.is_ok() {
            self.sync_goal_reminder();
        }
        result
    }

    #[must_use]
    pub fn expected_goal_gate<'a>(&'a self, turn: &'a GoalTurn) -> Option<&'a str> {
        self.autonomy.goal.expected_gate(turn)
    }

    pub fn record_goal_turn(
        &mut self,
        turn: &GoalTurn,
        report: GoalTurnReport,
    ) -> Result<String, String> {
        let usage = (report.output_tokens, report.active_millis);
        let result = self.autonomy.goal.record_turn(turn, report)?;
        self.autonomy
            .record_usage(usage.0, usage.1)
            .map_err(|error| error.to_string())?;
        if let Some(reason) = self.autonomy.exhaustion() {
            self.autonomy.goal.pause_for_session_limit(&reason)?;
            self.autonomy.loops.pause_for_session_limit(&reason)?;
        }
        self.sync_goal_reminder();
        Ok(result)
    }

    pub fn loop_command(&mut self, raw: &str) -> Result<String, String> {
        let input = if raw.trim().is_empty() {
            "/loop".to_string()
        } else {
            format!("/loop {}", raw.trim())
        };
        let parsed = commands::SlashCommand::parse(&input).map_err(|error| error.to_string())?;
        let Some(commands::SlashCommand::Loop { command }) = parsed else {
            return Err("loop command did not parse as /loop".to_string());
        };
        self.autonomy
            .loops
            .apply(command, now_unix_ms(), &self.cwd)
    }

    pub fn start_headless_loop(
        &mut self,
        trigger: crate::autonomy::scheduler::Trigger,
        prompt: String,
        max_runs: Option<u32>,
    ) -> Result<String, String> {
        self.autonomy
            .loops
            .start_headless(trigger, prompt, max_runs, now_unix_ms())
    }

    #[must_use]
    pub fn headless_loop_state(
        &self,
        id: &str,
    ) -> Option<crate::autonomy::loops::HeadlessLoopState> {
        self.autonomy.loops.headless_state(id)
    }

    #[must_use]
    pub fn loop_iteration_status(
        &self,
        id: &str,
    ) -> Option<crate::autonomy::loops::LoopIterationStatus> {
        self.autonomy.loops.iteration_status(id)
    }

    #[must_use]
    pub fn loop_quiet_streak(&self, id: &str) -> Option<u32> {
        self.autonomy.loops.quiet_streak(id)
    }

    #[must_use]
    pub fn loop_status_report(&self) -> String {
        self.autonomy.loops.status_report(None)
    }

    pub fn dispatch_loop_turn(&mut self) -> Result<Option<LoopTurn>, String> {
        self.adopt_wakeups()?;
        if let Some(reason) = self.autonomy.exhaustion() {
            self.autonomy.goal.pause_for_session_limit(&reason)?;
            self.autonomy.loops.pause_for_session_limit(&reason)?;
            return Ok(None);
        }
        self.autonomy.loops.dispatch_due(now_unix_ms(), &self.cwd)
    }

    /// The `ScheduleWakeup` records this session's tools wrote since the last
    /// sweep, consumed. One reader: the store's files are gone once returned.
    fn take_wakeup_records(&self) -> Vec<tools::wakeup_store::WakeupRecord> {
        tools::wakeup_store::take_for_session(&self.cwd, &self.handle.id)
    }

    /// Fold every pending wakeup record into the loop registry (arm, re-arm
    /// or stop the session's wakeup loop) and queue the notices.
    fn adopt_wakeups(&mut self) -> Result<(), String> {
        let records = self.take_wakeup_records();
        if records.is_empty() {
            return Ok(());
        }
        let notices = self.autonomy.loops.adopt_wakeups(&records, now_unix_ms())?;
        self.autonomy_notices.extend(notices);
        Ok(())
    }

    /// Words the scheduler queued since the last call — printed by the driver
    /// right after the dispatch or receipt that produced them.
    #[must_use]
    pub fn take_autonomy_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.autonomy_notices)
    }

    #[must_use]
    pub fn expected_loop_gate<'a>(&'a self, turn: &'a LoopTurn) -> Option<&'a str> {
        self.autonomy.loops.expected_gate(turn)
    }

    pub fn record_loop_turn(
        &mut self,
        turn: &LoopTurn,
        mut receipt: TurnReceipt,
    ) -> Result<String, String> {
        // A wakeup loop's own iteration schedules itself through the records
        // it wrote; any other turn's records are adopted after it is recorded.
        let records = self.take_wakeup_records();
        let folded = self
            .autonomy
            .loops
            .fold_wakeups_into_receipt(turn, &mut receipt, &records);
        let usage = (receipt.output_tokens, receipt.active_millis);
        let result = self
            .autonomy
            .loops
            .record_turn(turn, receipt, now_unix_ms())?;
        if !folded && !records.is_empty() {
            let notices = self.autonomy.loops.adopt_wakeups(&records, now_unix_ms())?;
            self.autonomy_notices.extend(notices);
        }
        self.autonomy
            .record_usage(usage.0, usage.1)
            .map_err(|error| error.to_string())?;
        if let Some(reason) = self.autonomy.exhaustion() {
            self.autonomy.goal.pause_for_session_limit(&reason)?;
            self.autonomy.loops.pause_for_session_limit(&reason)?;
        }
        Ok(result)
    }

    /// The earliest moment any armed autonomous work is due: the goal, a
    /// loop, or a cron — the one clock both frontends sleep on.
    #[must_use]
    pub fn next_autonomy_wakeup(&self) -> Option<u64> {
        // An exhausted session budget refuses every cron turn, so a due cron
        // must not keep waking the loop to be refused again; the refusal is
        // said once, at the next dispatch a human turn triggers.
        let crons = self
            .autonomy
            .exhaustion()
            .is_none()
            .then(|| {
                self.cron_registry()
                    .and_then(|registry| crate::autonomy::crons::next_wakeup(&registry, now_unix_ms()))
            })
            .flatten();
        [
            self.autonomy.goal.next_wakeup(),
            self.autonomy.loops.next_wakeup(),
            crons,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// The cron registry the session's tools write — fetched from the live
    /// runtime each time, so a rebuild (`/model`, an account switch) cannot
    /// leave the scheduler reading a registry the tools no longer touch.
    fn cron_registry(&self) -> Option<runtime::team_cron_registry::CronRegistry> {
        self.runtime
            .runtime
            .as_ref()
            .map(|runtime| runtime.tool_executor().tool_registry().context().crons.clone())
    }

    /// The due cron whose run this session just recorded, as the turn to run.
    /// `Err` names a due cron the exhausted session budget refuses to run.
    pub fn dispatch_cron_turn(&mut self) -> Result<Option<CronTurn>, String> {
        let Some(registry) = self.cron_registry() else {
            return Ok(None);
        };
        let now = now_unix_ms();
        if let Some(reason) = self.autonomy.exhaustion() {
            self.autonomy.goal.pause_for_session_limit(&reason)?;
            self.autonomy.loops.pause_for_session_limit(&reason)?;
            return match crate::autonomy::crons::first_due(&registry, now) {
                Some(id) => Err(format!(
                    "{} {id} is due but {reason}; CronRunDue still enqueues it by hand",
                    crate::autonomy::crons::TURN_LABEL
                )),
                None => Ok(None),
            };
        }
        Ok(crate::autonomy::crons::dispatch_due(&registry, now))
    }

    /// Charge a cron turn to the session budget and word its receipt.
    pub fn record_cron_turn(
        &mut self,
        turn: &CronTurn,
        receipt: &TurnReceipt,
    ) -> Result<String, String> {
        self.autonomy
            .record_usage(receipt.output_tokens, receipt.active_millis)
            .map_err(|error| error.to_string())?;
        if let Some(reason) = self.autonomy.exhaustion() {
            self.autonomy.goal.pause_for_session_limit(&reason)?;
            self.autonomy.loops.pause_for_session_limit(&reason)?;
        }
        Ok(format!(
            "{} {} · {}",
            crate::autonomy::crons::TURN_LABEL,
            turn.id,
            receipt.summary()
        ))
    }

    #[must_use]
    pub fn loop_turn_is_iteration(turn: &LoopTurn) -> bool {
        turn.kind == LoopTurnKind::Iteration
    }

    /// Unattended turns use the same live permission policy as human turns.
    /// Without `--allow-writes` it is temporarily clamped to read-only; with
    /// the opt-in it inherits the session mode/rules. No prompter shortcut is
    /// installed — a real approval request is denied by the frontend and pauses
    /// the controller.
    pub fn begin_autonomous_turn(&mut self, allow_writes: bool) -> PermissionMode {
        let mut saved = self.permission_mode;
        if let Some(inner) = self.runtime.try_runtime_mut() {
            inner.set_autonomous_surface(true);
            if !allow_writes {
                saved = inner.set_active_permission_mode(PermissionMode::ReadOnly);
            }
        }
        saved
    }

    pub fn finish_autonomous_turn(&mut self, saved_permission: PermissionMode) {
        if let Some(inner) = self.runtime.try_runtime_mut() {
            inner.set_active_permission_mode(saved_permission);
            inner.set_autonomous_surface(false);
        }
    }

    fn sync_goal_reminder(&mut self) {
        let had_reminder = session_has_goal_reminder(self.runtime.session());
        sync_goal_reminder(
            &mut self.runtime,
            self.autonomy.goal.reminder_state(),
            had_reminder,
        );
    }

    /// `/effort` — 다음 턴의 라이브 클라이언트부터 적용된다.
    /// effort 를 세우고 다음 세션 기본값으로 저장한다.
    ///
    /// 인메모리 선택은 저장 실패와 무관하게 선다 — 사람이 방금 고른 값을
    /// 디스크 사정으로 되돌리지 않는다. 대신 실패 사유를 **돌려준다**:
    /// 예전에는 `let _ =` 로 삼켜서, 다음 세션이 조용히 옛 값으로 돌아가도
    /// 사람이 원인을 알 길이 없었다.
    pub fn set_effort(&mut self, effort: Effort) -> Result<(), Box<dyn std::error::Error>> {
        self.effort = Some(effort);
        self.publish_selection();
        self.persist_preferences()
    }

    // Both frontends mutate selection through this seam. Publishing here also
    // covers idle picker changes, where no turn/status update will follow.
    fn publish_selection(&self) {
        if let Some(channel) = crate::ide::events::channel() {
            channel.publish_status(&self.status(), &self.cwd);
        }
    }

    /// Persist the current model and effort as the next-session defaults.
    pub fn persist_preferences(&self) -> Result<(), Box<dyn std::error::Error>> {
        let effort = self.effort.unwrap_or(DEFAULT_EFFORT);
        crate::preferences::save(&self.model, effort)?;
        Ok(())
    }

    /// `/compact [focus]` — 라이브 런타임에 제자리 적용. `(removed, kept)`.
    pub fn compact(
        &mut self,
        focus: Option<&str>,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let result = self
            .runtime
            .compact(runtime::CompactionConfig::default(), focus);
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        self.runtime.apply_manual_compaction(result);
        self.persist()?;
        Ok((removed, kept))
    }

    #[must_use]
    /// 이 세션을 연 런치 플래그(`/resume` 세션 교체가 같은 조건으로 다시 연다).
    pub fn launch_flags(&self) -> LaunchFlags {
        LaunchFlags {
            allowed_tools: self.allowed_tools.clone(),
            disable_spawn_family: self.disable_spawn_family,
            mcp_config: self.mcp_config.clone(),
        }
    }

    /// 재개된 세션의 이전 대화를 화면 셀 재료로 — 새 세션이면 빈 목록.
    #[must_use]
    pub fn replay_items(&self) -> Vec<ReplayItem> {
        replay_items_from(&self.runtime.session().messages)
    }

    /// Composer history contributed by the resumed transcript. The global
    /// history file is loaded by the composer itself; this slice puts the
    /// current session's real user turns at the newest end of that history.
    #[must_use]
    pub fn composer_history_seed(&self) -> Vec<String> {
        if self.resumed {
            user_input_history_from(&self.runtime.session().messages)
        } else {
            Vec::new()
        }
    }

    /// 한 줄 상태 스냅샷. **부수효과 하나**: 사용량 프로브를 예약한다.
    ///
    /// 이 함수는 `/status` 를 그리려는 모든 경로가 반드시 지나는 자리다(카드·
    /// 배너·IDE status 프레임). 프로브 예약을 여기 두면 "사람이 지금 사용량을
    /// 보려 한다"는 신호가 한 곳에서 나가고, 엔진의 케이던스(1분)가 그것을
    /// 실제 요청 한 번으로 줄인다 — 그리는 쪽마다 예약을 흩뿌리면 언젠가 한
    /// 경로가 빠진다.
    pub fn status(&self) -> StatusSnapshot {
        crate::usage::refresh_soon();
        let (goal, autonomous) = self.autonomy.goal.status_fields();
        let loops = self.autonomy.loops.footer_text().unwrap_or_else(|| "none".to_string());
        StatusSnapshot {
            model: self.model.clone(),
            permission_mode: permission_label(self.permission_mode),
            effort: self.effort.map(Effort::canonical),
            session_id: self.handle.id.clone(),
            // The number the FOOTER shows, not a second opinion. The estimate
            // alone runs ~30% low against the provider's own count, so a card
            // built on it disagreed with the footer on the same screen.
            context_tokens: usize::try_from(self.runtime.effective_context_tokens())
                .unwrap_or(usize::MAX),
            goal,
            autonomous,
            loops,
            autonomy: self.autonomy.status(),
        }
    }

    /// Host default for an `Agent` call that omits `background`.
    ///
    /// See [`tools::ToolContext::background_agent_default`]: only a surface
    /// whose REPL consumes agent completions may say `true`, because a
    /// detached child's report is delivered by that REPL and nowhere else.
    pub fn set_background_agent_default(&self, background: bool) {
        if let Some(runtime) = self.runtime.runtime.as_ref() {
            runtime
                .tool_executor()
                .tool_registry()
                .context()
                .set_background_agent_default(background);
        }
    }

    /// Where a pane child's `mcp.call` is answered: this session's MCP
    /// runtime, through the very dispatch an inline child's passthrough uses
    /// (t-2513 §2.1). `None` when the session has no MCP servers.
    #[must_use]
    pub fn mcp_channel_bridge(&self) -> Option<crate::ide::channel::state::McpBridge> {
        let dispatch = self
            .runtime
            .runtime
            .as_ref()?
            .tool_executor()
            .tool_registry()
            .subagent_mcp_dispatch()?;
        Some(Arc::new(move |name: &str, input: &serde_json::Value| dispatch(name, input)))
    }

    /// Fan one `auth.reload` out to this session's live pane children
    /// (t-2513 §2.6), each over its own channel. Failures are the child's
    /// to report on its next turn; the parent's answer to the window does not
    /// wait on them.
    #[must_use]
    pub fn child_auth_reload_fanout(&self) -> crate::ide::channel::state::ChildFanout {
        let registry = self.registry();
        Arc::new(move |params: &serde_json::Value| {
            for child in tools::live_pane_children(&registry) {
                if let Err(error) = child.call(
                    runtime::subagent_panes::channel_method::AUTH_RELOAD,
                    params.clone(),
                ) {
                    eprintln!(
                        "[zo] auth.reload did not reach teammate {}: {error}",
                        child.agent_id
                    );
                }
            }
        })
    }

    /// The cold fixed harness this session would put in front of the model.
    ///
    /// Read from the live session rather than rebuilt, so the number is the
    /// one that actually ships: the preloaded-tool profile, MCP activation and
    /// a resumed session's compaction reminders all move it, and a
    /// reconstruction from defaults would report a harness nobody is billed
    /// for.
    #[must_use]
    pub fn prompt_input(&self) -> crate::prompt_input::PromptInput {
        let (tools, reminders) = self.runtime.runtime.as_ref().map_or_else(
            || (Vec::new(), Vec::new()),
            |runtime| {
                (
                    crate::filter_tool_specs(
                        runtime.tool_executor().tool_registry(),
                        self.allowed_tools.as_ref(),
                    ),
                    runtime.transient_reminders().to_vec(),
                )
            },
        );
        crate::prompt_input::PromptInput::measure(
            &self.model,
            &self.cwd.display().to_string(),
            &self.system_prompt,
            &tools,
            &reminders,
        )
    }

    /// The exact input-token cost of this session's cold harness, from the
    /// provider.
    ///
    /// Shaped as `architecture-r15.md` §2.4 specifies the release gate: the
    /// system blocks, the wire tools, and a first user message of `x`. Same
    /// protocol for every product being compared, so the numbers are
    /// comparable rather than merely present.
    ///
    /// `None` when there is nothing to ask (a non-Anthropic session — only the
    /// Anthropic surface exposes a counting endpoint) or when the ask failed.
    /// A diagnostic never fails a run because the network did.
    pub async fn provider_input_tokens(&self) -> Option<crate::prompt_input::ProviderCount> {
        if api::detect_provider_kind(&self.model) != api::ProviderKind::Anthropic {
            return None;
        }
        let runtime = self.runtime.runtime.as_ref()?;
        let tools = crate::filter_tool_specs(
            runtime.tool_executor().tool_registry(),
            self.allowed_tools.as_ref(),
        );
        let request = |tools: Option<Vec<api::ToolDefinition>>| api::MessageRequest {
            model: self.model.clone(),
            max_tokens: 1,
            messages: vec![api::InputMessage::user_text("x")],
            system: Some(
                self.system_prompt
                    .iter()
                    .map(|section| api::system_from_string(section.clone()))
                    .collect::<Vec<_>>()
                    .concat(),
            ),
            tools,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        };
        let client = api::AnthropicClient::from_env().ok()?;
        let total = match client.count_input_tokens(&request(Some(tools))).await {
            Ok(tokens) => tokens,
            Err(error) => {
                eprintln!(
                    "zo: could not reach the token counter ({error}); showing the estimate only"
                );
                return None;
            }
        };
        // Second count with the tools removed. The difference is the exact cost
        // of the wire tool block — the bucket the estimate is least equipped to
        // guess, because a JSON schema tokenizes far denser than prose and
        // `chars / 4` treats them alike. Without this split the calibration is a
        // single blended number that hides which budget is actually wrong.
        let without_tools = client.count_input_tokens(&request(None)).await.ok();
        Some(crate::prompt_input::ProviderCount {
            total,
            without_tools,
        })
    }

    /// zo-cli `LiveCli::replace_runtime` 의 핵심: 이전 세션의 writer 리스 반납,
    /// 체크포인트 상태 이관, 서브시스템 종료, 내부 런타임은 leak(비동기 문맥에서
    /// tokio 리소스 drop 금지), 셸은 blocking 허용 문맥에서 drop.
    fn replace_runtime(
        &mut self,
        mut new_runtime: BuiltRuntime,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(runtime) = self.runtime.runtime.as_ref() {
            runtime.session().release_writer_lease();
        }
        let previous_context = self.runtime.runtime.as_mut().map(|runtime| {
            runtime
                .tool_executor_mut()
                .tool_registry_mut()
                .context()
                .clone()
        });
        if let (Some(previous_context), Some(runtime)) =
            (previous_context.as_ref(), new_runtime.try_runtime_mut())
        {
            let context = runtime.tool_executor_mut().tool_registry_mut().context();
            context.copy_workspace_checkpoint_state_from(previous_context);
            context.set_inspection_shell(previous_context.inspection_shell());
            context.set_person_model_pin(previous_context.person_model_pin().as_deref());
        }
        // A gateway that refused zo's recalled memory refuses it under the next
        // model too, so this session keeps asking it without.
        let refusing_recall = self
            .runtime
            .runtime
            .as_ref()
            .map(runtime::ConversationRuntime::gateways_refusing_recall)
            .unwrap_or_default();
        if let Some(runtime) = new_runtime.try_runtime_mut() {
            runtime.withhold_recall_from(refusing_recall);
        }
        install_headless_task_delivery(&new_runtime, &self.tasks, self.headless, &self.handle.id);
        self.runtime.shutdown_lsp()?;
        self.runtime.shutdown_mcp()?;
        self.runtime.shutdown_plugins()?;
        if let Some(old_inner) = self.runtime.runtime.take() {
            std::mem::forget(old_inner);
        }
        let old_shell = std::mem::replace(&mut self.runtime, new_runtime);
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| drop(old_shell));
        } else {
            drop(old_shell);
        }
        Ok(())
    }
}

fn thinking_config_for(effort: Option<Effort>) -> Option<api::ThinkingConfig> {
    effort
        .map(Effort::budget)
        .filter(|budget| *budget > 0)
        .map(api::ThinkingConfig::enabled)
}

fn apply_active_model(runtime: &mut BuiltRuntime, model: &str) {
    if let Some(inner) = runtime.try_runtime_mut() {
        inner
            .tool_executor_mut()
            .tool_registry_mut()
            .context()
            .set_active_model(model);
    }
}

fn session_has_goal_reminder(session: &Session) -> bool {
    session.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, core_types::session::ContentBlock::Text { text }
                if text.contains(PERSISTENT_GOAL_REMINDER_PREFIX))
        })
    })
}

/// Keep the standing objective on the runtime's reminder seam rather than in
/// the base system prompt (which is cache-stable). Reminder persistence makes
/// it cross turns; the session header lets a fresh runtime re-install it after
/// `/resume` or compaction. A clear marker overrides any older persisted goal
/// reminder without injecting noise into sessions that never had a goal.
fn sync_goal_reminder(
    runtime: &mut BuiltRuntime,
    goal: Option<(&str, GoalPhase)>,
    had_reminder: bool,
) {
    let reminder = goal_reminder(goal, had_reminder);
    if let Some(inner) = runtime.try_runtime_mut() {
        inner.replace_transient_system_reminder_by_prefix(
            PERSISTENT_GOAL_REMINDER_PREFIX,
            reminder.as_deref(),
        );
    }
}

/// Surface the most recent completed Dreamer pass through the runtime's
/// once-until-persisted reminder seam. The first model request records the
/// notice as stable transcript context; a resume sees the marker and does not
/// append a second copy for the same session.
fn sync_dreamer_model_reminder(runtime: &mut BuiltRuntime, line: Option<&str>) {
    let reminder = dreamer_model_reminder(line);
    if let Some(inner) = runtime.try_runtime_mut() {
        inner.install_reminder_until_persisted(
            DREAMER_LAST_PASS_REMINDER_PREFIX,
            reminder.as_deref(),
        );
    }
}

fn dreamer_model_reminder(line: Option<&str>) -> Option<String> {
    line.map(|line| {
        format!(
            "{DREAMER_LAST_PASS_REMINDER_PREFIX}\n<system-reminder>\nDreamer learning from a \
             previous completed session is available as status, not a new task:\n{line}\n\
             </system-reminder>"
        )
    })
}

fn goal_reminder(goal: Option<(&str, GoalPhase)>, had_reminder: bool) -> Option<String> {
    goal.map_or_else(
        || {
            had_reminder.then(|| {
                format!(
                    "{PERSISTENT_GOAL_REMINDER_PREFIX} <system-reminder>No persistent session \
                     goal is active. Ignore any earlier reminder with this prefix.</system-reminder>"
                )
            })
        },
        |(goal, phase)| {
            let quoted = serde_json::to_string(goal)
                .unwrap_or_else(|_| "\"goal\"".to_string())
                .replace('&', "\\u0026")
                .replace('<', "\\u003c")
                .replace('>', "\\u003e");
            let state = match phase {
                GoalPhase::Completed => {
                    "It is recorded as completed; do not continue work solely because an older reminder called it active."
                }
                GoalPhase::Running => {
                    "It is active and its bounded autonomous controller is running."
                }
                GoalPhase::Saved | GoalPhase::Paused => {
                    "It remains the standing objective, while autonomous continuation is off."
                }
            };
            Some(format!(
                "{PERSISTENT_GOAL_REMINDER_PREFIX} <system-reminder>The user explicitly set \
                 this persistent session goal: {quoted}. {state} Keep the recorded state in \
                 view across turns and compaction. A current user instruction may refine the \
                 next action, but only an explicit /goal complete or a green controller gate \
                 completes it, and only /goal clear forgets it.</system-reminder>"
            ))
        },
    )
}

/// CLI 라벨(zo-cli `permission_mode.rs` 와 동일한 3분류).
#[must_use]
pub fn permission_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "read-only",
        PermissionMode::WorkspaceWrite | PermissionMode::Prompt => "workspace-write",
        PermissionMode::Allow | PermissionMode::DangerFullAccess => "danger-full-access",
    }
}

/// `<id>.system-prompt.json` — 세션이 만들어질 때의 프롬프트를 보존해 재개 시
/// 그대로 재사용한다(바이트가 달라지면 제공자 캐시 프리픽스가 전부 무효).
fn session_system_prompt_path(session_path: &Path) -> PathBuf {
    session_path.with_extension("system-prompt.json")
}

/// A teammate's prompt: the carried sections, stored beside the transcript
/// exactly like a built one so a resume of this session replays it verbatim
/// (the provider cache prefix depends on the bytes not moving).
fn carried_system_prompt(session_path: &Path, resumed: bool, carried: &[String]) -> Vec<String> {
    if resumed {
        if let Some(stored) = std::fs::read(session_system_prompt_path(session_path))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Vec<String>>(&raw).ok())
            .filter(|sections| !sections.is_empty())
        {
            return stored;
        }
    }
    if let Ok(payload) = serde_json::to_vec(carried) {
        let _ = core_types::paths::write_private_file(
            &session_system_prompt_path(session_path),
            &payload,
            &core_types::paths::ParentDirPolicy::LeaveParent,
        );
    }
    carried.to_vec()
}

fn session_system_prompt(
    cwd: &Path,
    model: &str,
    session_path: &Path,
    resumed: bool,
    prompt_mode: runtime::PromptMode,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if resumed {
        if let Some(stored) = std::fs::read(session_system_prompt_path(session_path))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Vec<String>>(&raw).ok())
            .filter(|sections| !sections.is_empty())
        {
            return Ok(stored);
        }
    }
    let built = crate::conversation_support::build_system_prompt_for_mode(
        cwd,
        prompt_mode,
        model,
    )?;
    if let Ok(payload) = serde_json::to_vec(&built) {
        let _ = core_types::paths::write_private_file(
            &session_system_prompt_path(session_path),
            &payload,
            &core_types::paths::ParentDirPolicy::LeaveParent,
        );
    }
    Ok(built)
}

/// Headless runs share the runtime's boundary inbox; interactive runs keep
/// their existing pump, which also owns idle follow-up delivery.
fn install_headless_task_delivery(runtime: &BuiltRuntime, tasks: &runtime::task_registry::TaskRegistry, headless: bool, session_id: &str) {
    if headless {
            if let Some(inner) = runtime.runtime.as_ref() {
                let inbox = inner.agent_notification_inbox();
                let owner = session_id.to_string();
                let callback: runtime::task_registry::TaskCompletionCallback = Box::new(move |task_id, status, output, session_id| {
                    if session_id.as_deref() != Some(owner.as_str()) { return; }
                    if let Some(notification) = super::agent_completion_pump::background_task_notification(task_id, status, output) {
                        inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(notification);
                    }
                });
                tasks.set_completion_callback(Some(Arc::new(callback)));
            }
        }
 }

/// Put the session's registry on the context shared by every tool clone.
fn install_agent_registry(runtime: &BuiltRuntime, registry: &Arc<tools::AgentRegistry>) {
    if let Some(runtime) = runtime.runtime.as_ref() {
        runtime
            .tool_executor()
            .tool_registry()
            .context()
            .install_registry(Arc::clone(registry));
    }
}

/// Publish the conversation a `fork` sub-agent inherits (t-2875): this
/// session's transcript — appended before every tool of a batch runs, so it
/// is current at the boundary a fork is taken on — and the system prompt as
/// the runtime holds it (CLI overrides and a `/model` retarget included,
/// which the stored sidecar does not carry). After every build and rebuild,
/// like the registry, since a rebuilt runtime is a fresh context.
fn install_fork_source(runtime: &BuiltRuntime) {
    let Some(inner) = runtime.runtime.as_ref() else {
        return;
    };
    let Some(transcript) = inner.session().persistence_path() else {
        return;
    };
    inner
        .tool_executor()
        .tool_registry()
        .context()
        .install_fork_source(tools::ForkSource {
            transcript: transcript.to_path_buf(),
            system_prompt: inner.system_prompt().to_vec(),
        });
}

/// 핵심: 레거시 stdout 직배출을 끄고 모든 출력을 `RenderBlock` 으로 통일한다.
#[allow(clippy::too_many_arguments)]
fn build_runtime(
    cwd: &Path,
    mcp_config: Option<&PathBuf>,
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
    tasks: runtime::task_registry::TaskRegistry,
    remote_mcp: Option<runtime::subagent_panes::McpRoute>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    const ENABLE_TOOLS: bool = true;
    const EMIT_OUTPUT: bool = false;
    // 한 길만 있다. `--mcp-config` 도 정본 빌더가 받는다 — 예전에는 이 자리에서
    // 빌더 앞부분을 복사해 분기했고, 그 복사본이 커스텀 프로바이더 env·모델
    // 와이어 env·세션 보존 청소·고아 수거와 단계 태그 에러를 빠뜨렸다.
    build_runtime_with_thinking(
        cwd,
        mcp_config.map(PathBuf::as_path),
        session,
        session_id,
        model,
        system_prompt,
        ENABLE_TOOLS,
        EMIT_OUTPUT,
        allowed_tools,
        permission_mode,
        thinking,
        named_effort,
        effort_band_ceiling,
        Some(tasks),
        remote_mcp,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        dreamer_model_reminder, goal_reminder, permission_label, thinking_config_for,
        DEFAULT_EFFORT,
    };
    use crate::effort::Effort;
    use crate::goal::GoalPhase;
    use runtime::PermissionMode;

    #[test]
    fn permission_labels_fold_to_three_cli_words() {
        assert_eq!(permission_label(PermissionMode::ReadOnly), "read-only");
        assert_eq!(permission_label(PermissionMode::Prompt), "workspace-write");
        assert_eq!(permission_label(PermissionMode::Allow), "danger-full-access");
    }

    #[test]
    fn effort_off_disables_thinking() {
        assert!(thinking_config_for(Some(Effort::Off)).is_none());
        assert!(thinking_config_for(Some(DEFAULT_EFFORT)).is_some());
        assert!(thinking_config_for(None).is_none());
    }

    #[test]
    fn persistent_goal_reminder_survives_turns_and_clear_overrides_old_context() {
        let active = goal_reminder(Some(("ship </system-reminder>", GoalPhase::Running)), false)
            .expect("active reminder");
        assert!(active.contains("[zo:persistent-goal]"));
        assert!(active.contains("bounded autonomous controller is running"));
        // User text is JSON-quoted, not interpolated as reminder markup.
        assert!(active.contains(r"ship \u003c/system-reminder\u003e"));
        assert!(!active.contains("ship </system-reminder>"));

        let completed = goal_reminder(Some(("ship", GoalPhase::Completed)), true)
            .expect("completed reminder");
        assert!(completed.contains("recorded as completed"));

        assert!(goal_reminder(None, false).is_none());
        let cleared = goal_reminder(None, true).expect("clear override");
        assert!(cleared.contains("No persistent session goal is active"));
    }

    #[test]
    fn the_last_dreamer_pass_uses_the_once_until_persisted_model_seam() {
        let line = "dreamer: promoted 2, skipped 0 (now): gate-order, pty-lane-flush";
        let reminder = dreamer_model_reminder(Some(line)).expect("a pass gets a reminder");
        assert!(reminder.starts_with("[zo:dreamer-last-pass]"));
        assert!(reminder.contains(line), "the model sees the exact status line");
        assert!(reminder.contains("previous completed session"));
        assert!(dreamer_model_reminder(None).is_none());

        let production = include_str!("plain_session.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes tests");
        assert!(
            production.contains("install_reminder_until_persisted")
                && production.contains("DREAMER_LAST_PASS_REMINDER_PREFIX"),
            "the host must use the runtime's once-per-session persisted-reminder seam"
        );
    }
}

#[cfg(test)]
mod replay_tests {
    use super::{replay_items_from, user_input_history_from, ReplayItem};
    use core_types::session::{ContentBlock, ConversationMessage};
    use runtime::message_stream::AgentResultStatus;

    #[test]
    fn replay_pairs_tool_results_with_their_calls_and_skips_thinking() {
        let mut result = ConversationMessage::user_text("");
        result.blocks = vec![ContentBlock::ToolResult {
            tool_use_id: "call-1".into(),
            tool_name: "bash".into(),
            output: "hello".into(),
            is_error: false,
            images: Vec::new(),
        }];
        let messages = vec![
            ConversationMessage::user_text("ls 해줘"),
            ConversationMessage::assistant(vec![
                ContentBlock::Thinking { thinking: "…".into(), signature: String::new() },
                ContentBlock::Text { text: "확인하겠습니다.".into() },
                ContentBlock::ToolUse { id: "call-1".into(), name: "bash".into(), input: "ls".into() },
            ]),
            result,
            ConversationMessage::assistant(vec![ContentBlock::Text { text: "hello 하나뿐입니다.".into() }]),
        ];
        assert_eq!(
            replay_items_from(&messages),
            vec![
                ReplayItem::User("ls 해줘".into()),
                ReplayItem::Assistant("확인하겠습니다.".into()),
                ReplayItem::ToolCall { name: "bash".into(), input: "ls".into(), output: Some("hello".into()), is_error: false },
                ReplayItem::Assistant("hello 하나뿐입니다.".into()),
            ]
        );
    }

    #[test]
    fn replay_hides_wire_reasoning_passports_from_assistant_cells() {
        let messages = vec![ConversationMessage::assistant(vec![
            ContentBlock::Text {
                text: "[earlier reasoning]\nprivate carried thought".into(),
            },
            ContentBlock::Text {
                text: "visible answer".into(),
            },
        ])];

        assert_eq!(
            replay_items_from(&messages),
            vec![ReplayItem::Assistant("visible answer".into())]
        );
    }

    #[test]
    fn replay_restores_agent_result_without_model_only_preambles() {
        let persisted = "[Task notification — a background agent you launched finished while this turn was still running. Its result follows. This is a host notification, not a user message: use the result now where it affects your current work, and account for it before ending the turn.]\n\n\
                         [task notification — background agent `runtime-scout` (id: agent-7) finished. To follow up without losing its context, use SendMessage with that id/name as `to` to continue this agent]\n\n\
                         found the bug\nverification passed";
        let message = ConversationMessage::user_text(persisted);

        assert_eq!(
            replay_items_from(std::slice::from_ref(&message)),
            vec![ReplayItem::AgentResult {
                label: "runtime-scout".into(),
                status: AgentResultStatus::Completed,
                summary: None,
                body: "found the bug\nverification passed".into(),
            }]
        );
        let ContentBlock::Text { text } = &message.blocks[0] else {
            panic!("fixture must remain text");
        };
        assert!(text.contains("use SendMessage"), "wire text was rewritten");
    }

    #[test]
    fn resume_composer_seed_keeps_only_real_user_inputs() {
        let notification = "[task notification — background agent `runtime-scout` (id: agent-7) \
                            finished. To follow up without losing its context, use SendMessage with \
                            that id/name as `to` to continue this agent]\n\nfound the bug";
        let mut tool_message = ConversationMessage::user_text("tool payload");
        tool_message.role = core_types::session::MessageRole::Tool;
        let messages = vec![
            ConversationMessage::user_text("first question"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "answer".into(),
            }]),
            ConversationMessage::user_text(notification),
            ConversationMessage::user_text(format!(
                "{}change course",
                runtime::STEERING_ECHO_PREFIX
            )),
            tool_message,
            ConversationMessage::user_text("literal mention of ⤷ steering: stays"),
            ConversationMessage::user_text("second\nquestion"),
        ];

        assert_eq!(
            user_input_history_from(&messages),
            vec![
                "first question".to_string(),
                "literal mention of ⤷ steering: stays".to_string(),
                "second\nquestion".to_string(),
            ]
        );
    }
}

/// What the host must ARM on the way into a turn.
///
/// Every entry here started the same way: a capability built on both ends,
/// documented, consumed by careful runtime code — and switched on by nobody.
/// The tests are source reads because a terminal and a provider are not
/// available in a unit test; the behaviour each one guards is pinned in the
/// runtime's own integration tests.
#[cfg(test)]
mod turn_arming_tests {
    /// The interactive host must ARM the progress-gated deadline extension.
    ///
    /// Everything else existed: the policy, its defaults (2 pushes of 30
    /// minutes), its env overrides, and a careful consumer in the turn loop
    /// that extends only on fresh progress, caps the count, and announces it.
    /// Nothing set it, so `deadline_extension` stayed `None` and the blunt
    /// 60-minute cut — the one that consumer's own comment blames for
    /// "interrupting the legitimate long audit or deploy pipeline" — was the
    /// entire policy.
    ///
    /// A terminal is not available here, so this reads the source the way the
    /// USAGE drift guard does, and pins the two things that make it work: it
    /// is armed BESIDE the deadline it extends (so both are re-read every
    /// turn), and it comes from the env reader rather than a literal.
    #[test]
    fn the_turn_arms_the_deadline_extension_beside_the_deadline() {
        let source = include_str!("plain_session.rs");
        let budgets = source
            .find("runtime::env_turn_budgets(attendance)")
            .expect("the per-turn budget block exists");
        let arm = source
            .find("set_deadline_extension(runtime::env_deadline_extension())")
            .expect("the extension is armed from the env policy, not a literal");
        assert!(
            arm > budgets,
            "the extension must be armed inside the per-turn budget block, so an \
             env change takes effect next turn like the deadline itself"
        );
        // Same block: no other turn-entry site may sit between them.
        let between = &source[budgets..arm];
        assert!(
            !between.contains("fn "),
            "the arming drifted out of the budget block: {:?}",
            &between[..between.len().min(200)]
        );
    }

    /// `/model` rebuilds the runtime around the same session, and what the old
    /// runtime learned about a gateway goes with it: the new runtime is handed
    /// the gateways that refused zo's recalled memory before the old one is
    /// dropped. The behaviour is pinned in the runtime's own
    /// `a_runtime_taking_over_the_session_keeps_recall_withheld_from_a_refusing_gateway`;
    /// what can only be checked here is that the one place that swaps
    /// runtimes hands the set over (09-12: after `/model` the memory rode
    /// again and the refusal cost one more request).
    #[test]
    fn a_rebuilt_runtime_inherits_the_gateways_that_refused_recall() {
        let whole = include_str!("plain_session.rs");
        let source = &whole[..whole.find("#[cfg(test)]").expect("tests live at the end")];
        let swap = source.find("fn replace_runtime(").expect("the one runtime swap");
        let body = &source[swap..];
        let body = &body[..body.find("\n    }\n").expect("its body ends")];
        let read = body
            .find("gateways_refusing_recall")
            .expect("the swap reads what the old runtime withheld");
        let handed = body
            .find(".withhold_recall_from(")
            .expect("the swap hands it to the new runtime");
        let dropped = body
            .find("std::mem::replace(&mut self.runtime, new_runtime)")
            .expect("the swap drops the old runtime");
        assert!(read < handed && handed < dropped, "read, hand over, then drop");
    }

    /// Esc while a tool runs. The runtime's dispatch `select!` has exactly two
    /// arms — the tool finishing, and a [`runtime::ToolCancelSignal`] — so the
    /// two flags `cancel_turn` raises are read by nobody while a tool is in
    /// flight. The behaviour is pinned end to end in the runtime's own
    /// `a_signal_installed_by_the_host_cancels_a_running_tool`; what can only
    /// be checked here is that the host installs *its own* handle, and installs
    /// it every turn.
    ///
    /// Every turn matters as much as at all: this runtime is rebuilt on a model
    /// or permission switch, and a signal installed once at construction goes
    /// with it — Esc would come back to life dead, silently, exactly the way it
    /// was before.
    #[test]
    fn the_turn_arms_the_hosts_own_tool_cancel_signal() {
        // Production half only: this file's own tests quote the strings below,
        // and `include_str!` reads the whole file.
        let whole = include_str!("plain_session.rs");
        let source = &whole[..whole.find("#[cfg(test)]").expect("tests live at the end")];
        let budgets = source
            .find("runtime::env_turn_budgets(attendance)")
            .expect("the per-turn budget block exists");
        let arm = source
            .find("set_tool_cancel_signal(self.tool_cancel.clone())")
            .expect("the signal installed is the host's own field, not a fresh one");
        assert!(
            arm > budgets,
            "the signal must be armed inside the per-turn budget block, so a runtime \
             rebuilt for a model or permission switch is re-armed on the next turn"
        );
        let between = &source[budgets..arm];
        assert!(
            !between.contains("fn "),
            "the arming drifted out of the budget block: {:?}",
            &between[..between.len().min(200)]
        );
        // A fresh signal here would be armed and instantly orphaned: the
        // runtime would watch an epoch no key press can reach.
        assert!(
            !source.contains("set_tool_cancel_signal(runtime::ToolCancelSignal::new())"),
            "the runtime must watch the epoch the host holds, not a new one"
        );
    }

    /// The other half of the same wire: the key press has to fire it, and fire
    /// it **last**. The freed turn walks straight into the next stream race,
    /// which reads the abort flag — so the flag must already be up when the
    /// tool lets go, or the turn resumes and keeps working.
    #[test]
    fn cancelling_a_turn_fires_the_signal_after_both_flags() {
        let source = include_str!("turn_scaffold.rs");
        let body = source
            .split("pub(crate) fn cancel_turn(&self)")
            .nth(1)
            .expect("cancel_turn exists");
        let body = &body[..body.find("\n    }").expect("its body ends")];
        let flag = body
            .find("self.abort.abort()")
            .expect("cancel_turn raises the hook abort flag");
        let fire = body
            .find("self.tool_cancel.cancel_running_tools()")
            .expect("cancel_turn releases a tool that is running right now");
        assert!(
            fire > flag,
            "the signal must fire after the flags are up, or the freed turn \
             starts another request before anything reads them"
        );
    }

    /// A person's turn runs until it ends, the way it does in Claude Code and
    /// Codex: the host asks for its budgets with the attendance of THIS turn,
    /// and attendance is read from both signals that mean "nobody can press
    /// Esc" — the launch's `headless` flag and the runtime's autonomous
    /// surface (a goal-controller or window-driven turn). Drop either and a
    /// class of turn silently gets the wrong default: a headless run with no
    /// net, or a person's long orchestration cut at sixty minutes again.
    #[test]
    fn the_turn_asks_for_budgets_with_its_own_attendance() {
        let whole = include_str!("plain_session.rs");
        let source = &whole[..whole.find("#[cfg(test)]").expect("tests live at the end")];
        let budgets = source
            .find("runtime::env_turn_budgets(attendance)")
            .expect("the budgets are asked for with the turn's attendance");
        let decided = source[..budgets]
            .rfind("let attendance = if self.headless || inner.is_autonomous_surface()")
            .expect("attendance is decided from the headless flag and the autonomous surface");
        assert!(
            !source[decided..budgets].contains("fn "),
            "attendance must be decided in the budget block itself, so it is \
             re-read every turn like the budgets"
        );
        assert!(
            source.contains("headless: options.headless,"),
            "the host keeps the launch's headless flag for its turns"
        );
        // The runtime's own guards (the repetition guard) read the same
        // answer: a person's turn is skipped-and-told, never ended, by them.
        assert!(
            source[decided..budgets].contains("inner.set_attendance(attendance);")
                && source[decided..budgets].contains("runtime::declare_attendance(attendance);"),
            "the runtime — and the process, for the helpers it spawns — is told who can \
             stop the turn, in the same block"
        );
        assert!(
            !source.contains("env_turn_budgets(runtime::Attendance::Attended)")
                && !source.contains("env_turn_budgets(runtime::Attendance::Unattended)"),
            "no turn site may hard-code its attendance"
        );
    }

    /// The policy the host arms is the documented one, and it is disableable.
    #[test]
    fn the_env_policy_is_two_half_hour_pushes_and_can_be_turned_off() {
        // Defaults, with nothing in the environment.
        let (count, step) = runtime::env_deadline_extension().expect("on by default");
        assert_eq!(count, 2, "two pushes");
        assert_eq!(step.as_secs(), 30 * 60, "half an hour each");
    }
}

#[cfg(test)]
mod prompt_mode_tests {
    /// A run with nobody at the keyboard must be TOLD so.
    ///
    /// `PromptMode::Autonomous` existed — "the user is not watching in real
    /// time and cannot answer questions mid-task" — and nothing ever selected
    /// it. Every zo run, piped or redirected or `--plain`, carried the
    /// interactive contract instead, which tells the model to stop and ask
    /// when it needs a decision. In a pipe that is not caution, it is a
    /// stalled run: the question reaches nobody.
    ///
    /// This is not a token win — the autonomous section is about 70 tokens
    /// LARGER. It is a correctness one.
    #[test]
    fn a_headless_surface_gets_the_autonomous_contract() {
        let dir = tempfile::tempdir().expect("temp dir");
        let build = |mode| {
            crate::conversation_support::build_system_prompt_for_mode(
                dir.path(),
                mode,
                "claude-opus-5",
            )
            .expect("prompt builds")
            .join("\n")
        };

        let headless = build(runtime::PromptMode::Autonomous);
        assert!(headless.contains("# Operating autonomously"), "headless");
        assert!(
            headless.contains("cannot answer questions mid-task"),
            "the reason must travel with the rule"
        );
        assert!(
            !headless.contains("# Finishing the turn"),
            "a pipe must not be told a human is waiting"
        );

        let interactive = build(runtime::PromptMode::Interactive);
        assert!(interactive.contains("# Finishing the turn"), "interactive");
        assert!(!interactive.contains("# Operating autonomously"));
    }

    /// And the surface must actually pick it.
    ///
    /// The parser only sees `--plain`; a pipe or a redirect is just as
    /// headless, so the tty check has to happen — and it has to happen BEFORE
    /// the session opens, because the prompt is frozen at creation and
    /// replayed verbatim on resume. Read from the source the way the USAGE
    /// drift guard is, because a terminal is not available in a unit test.
    #[test]
    fn main_decides_the_surface_before_opening_the_session() {
        let source = include_str!("../main.rs");
        let decide = source
            .find("launch.open.headless")
            .expect("main sets the surface");
        let open = source
            .find("PlainSession::open")
            .expect("main opens the session");
        assert!(
            decide < open,
            "the surface must be decided before the session freezes its prompt"
        );
        let head = &source[..decide];
        for signal in ["render.plain", "stdin().is_terminal()", "stdout().is_terminal()"] {
            assert!(
                head.contains(signal),
                "the surface check lost {signal} — a redirect would read as interactive"
            );
        }
    }
}

#[cfg(test)]
mod background_default_tests {
    /// The interactive TUI must actually OPT IN to the detached default.
    ///
    /// This is the half that was missing. The context cell existed and the
    /// dispatcher already read it (`inp.background = Some(ctx
    /// .background_agent_default())`), so the mechanism tested fine — but no
    /// host ever called the setter, the cell stayed `false`, and every `Agent`
    /// call that omitted `background` blocked the turn against a schema that
    /// promised the opposite. A blocked turn has no boundary to deliver a
    /// steer into, which is how "말을 걸어도 대답이 없다" happened.
    ///
    /// The call sits in `tui::run`, which needs a real terminal, so this reads
    /// the source the way the USAGE drift guard does — the same technique that
    /// caught `--continue` missing from the help text.
    #[test]
    fn the_interactive_tui_opts_into_the_detached_agent_default() {
        let source = include_str!("../tui/app.rs");
        let run = source
            .split_once("pub async fn run(")
            .expect("tui::run exists")
            .1;
        let body = run.split_once("enable_raw_mode()").expect("raw mode entry").0;
        assert!(
            body.contains("set_background_agent_default(true)"),
            "tui::run no longer opts into the detached default — an omitted \
             `background` will block the turn again, and a blocked turn cannot \
             answer a steer"
        );
    }

    #[test]
    fn the_project_home_is_the_sessions_directorys_parent() {
        let path = std::path::Path::new("/h/.zo/projects/repo-abc/sessions/session-1.jsonl");
        assert_eq!(
            super::project_home_of(path).as_deref(),
            Some(std::path::Path::new("/h/.zo/projects/repo-abc"))
        );
        assert_eq!(super::project_home_of(std::path::Path::new("session.jsonl")), None);
    }
}
