//! Shared domain vocabulary for ZeroCode.
//!
//! Every other crate — the hook bridge, the PTY host, the harness client, the
//! orchestrator, the Tauri shell — speaks in these types, and the web view
//! mirrors their serde field names. That makes this crate the single place a
//! wire-visible name is allowed to change.
//!
//! The serialization rule is uniform: **snake_case, everywhere**. No per-struct
//! exceptions, so a field is spelled the same in Rust, in JSON, in a hook form
//! body, and in the TypeScript mirror.

pub mod account;
pub mod account_autoswitch;
pub mod advertised_url;
pub mod agent;
pub mod agent_browser;
pub mod agent_emulator;
pub mod agent_exit;
pub mod agent_lineage;
pub mod agent_teams;
pub mod app;
pub mod artifact;
pub mod artifact_publish;
pub mod artifact_transcript;
pub mod ask;
pub mod automation;
pub mod board;
pub mod branching;
pub mod browser_read;
pub mod capabilities;
pub mod checks;
pub mod civil;
pub mod cli_login_files;
pub mod clone;
pub mod codex_account;
pub mod codex_delta;
pub mod commit_failure;
pub mod commit_message;
pub mod compact_diff;
pub mod computer_flow;
pub mod computer_recipe;
pub mod computer_use;
pub mod computer_use_protocol;
pub mod conflict;
pub mod credential;
pub mod delegation;
pub mod git_config;
pub mod git_dir;
pub mod git_graph;
pub mod git_prompt;
pub mod guarded;
pub mod guide;
pub mod hook;
pub mod hook_continuation;
pub mod host;
pub mod interrupt;
pub mod jev;
pub mod jira;
pub mod lane;
pub mod launch;
pub mod linear;
pub mod localhost_label;
pub mod mermaid;
pub mod notify;
pub mod notify_call;
pub mod onboarding;
pub mod orchestration;
pub mod pane;
pub mod pane_claim;
pub mod payload;
pub mod pick;
pub mod project;
pub mod provider_session;
pub mod readiness;
pub mod reap;
pub mod repo_mark;
pub mod repo_trust;
pub mod review;
pub mod scm_observer;
pub mod scm_tree;
pub mod screen_action;
pub mod second_brain;
pub mod second_brain_code;
pub mod second_brain_export;
pub mod second_brain_graph;
pub mod second_brain_lint;
pub mod second_brain_live;
pub mod second_brain_pairs;
pub mod second_brain_paths;
pub mod second_brain_relate;
pub mod second_brain_related;
pub mod session;
pub mod shell_history;
pub mod skill;
pub mod skill_install;
pub mod source_control_ai;
pub mod stall_cause;
pub mod stats_events;
pub mod step_effort;
pub mod summon_choice;
pub mod supply_chain;
pub mod task;
pub mod transcript;
pub mod type_value;
pub mod untrusted;
pub mod usage_ledger;
pub mod usage_limit;
pub mod usage_report;
pub mod usage_stats;
pub mod usage_stats_codex;
pub mod usage_stats_opencode;
pub mod vault;
pub mod vault_opencode;
pub mod worker_placement;
pub mod worker_transcript;
pub mod workitem;
pub mod workspace_cleanup;
pub mod workspace_space;
pub mod worktree_ownership;

pub use account::{
    AccountSelection, CONFIG_DIR_VAR, ClaudeAccount, ClaudeIdentity, OVERRIDING_AUTH_VARS,
    Provider, SECURE_STORAGE_CONFIG_DIR_VAR, active_account, duplicate_of, launch_env,
    providers_for,
};
pub use agent::{
    AGENT_SPECS, ALL_AGENTS, AgentKind, AgentPresence, AgentSpec, ComposerClear,
    DefaultAgentPreference, HookAdditionalContext, Injection, NudgeRoad, ReadyMark, agent_presence,
    agent_spec,
};
pub use agent_exit::{
    ForegroundWatch, LOOK_EVERY_MS, LOOKS_BEFORE_GONE, child_is_the_agent, claims_running,
};
pub use agent_lineage::{Lineage, ROOT_LINEAGE};
pub use automation::{
    Automation, AutomationFailureRecord, AutomationFailureStage, AutomationRun, Cadence, Cron,
    EvidencePolicy, LocalMinute, PrecheckOutcome, PrecheckRecord, RUNS_KEPT_PER_AUTOMATION,
    RunTrigger, Schedule, WorkspaceMode, agent_may_be_saved, born_worktrees, close_unwitnessed,
    failed_occurrence_recorded, leaves_evidence, mark_completed, mark_ended, occurrence_recorded,
    prune_final_runs, record_run, reuse_normalized, should_run_precheck,
};
pub use capabilities::{AgentCapabilities, agent_capabilities};
pub use checks::{
    CheckAnnotation, CheckConclusion, CheckDetails, CheckJob, CheckRun, CheckStatus, CheckStep,
    CheckTally, actions_run_id, broken_checks, log_tail_for_prompt, sort_for_display,
};
pub use guide::{Facts as GuideFacts, Step as GuideStep};
pub use hook::HookEnvelope;
// 호스트 경계의 이름들. `About`/`DirEntry`/`ReadDir`는 이 크레이트에 흔한
// 낱말이라 모듈 이름과 함께 불린다 — 창은 `host::DirEntry`로 쓴다.
pub use host::{
    ExecutionHostId, Fs, Host, HostError, PinnedHostKey, PtyCwd, PtySpec, RemotePath,
    RemotePathError, RemoteWorkspace, SSH_DEFAULT_PORT, SshAuthentication, SshEndpoint,
    SshHostDraft, SshHostError, SshHostRecord, Vcs,
};
pub use lane::{Lane, LaneId, LaneState};
pub use launch::{
    AgentPermissionMode, LaunchOverride, LaunchPlan, PermissionMode, apply_agent_permission_mode,
    default_launch_args, default_launch_env, has_permission_switch, join_command_line,
    launch_args_line, launch_env_line, launch_plan, sanitize_launch_args, split_command_line,
};
pub use onboarding::{Checklist as OnboardingChecklist, Onboarding};
pub use orchestration::{DurableRetryIdentity, PreparedWorkerStart};
pub use pane::{FIRST_PANE_HANDLE, PaneHandle, PaneKey, PaneKeyParseError};
pub use project::{
    DefaultTab, PROJECT_FILE, ProjectFile, ProjectScript, SCRIPT_TIMEOUT, ScriptSource,
    SetupRunPolicy, effective_script, parse_project_file, runs_setup_on_create, script_env,
};
pub use provider_session::{
    ConversationKey, ProviderSession, SessionKey, claude_args_without_selectors, conversation_key,
    conversation_never_written, publishes_session, resume_argv, resume_argv_continuing,
    session_in_payload, session_key_of, session_will_come,
};
pub use repo_mark::{REPO_MARK_PALETTE, normalize_repo_mark};
pub use repo_trust::{TrustStanding, content_hash, standing, trust_content};
pub use review::DiffNote;
pub use session::SessionLabel;
pub use task::{DISPLAY_NAME_MAX_CHARS, TASK_TITLE_MAX_CHARS, WorktreeTask};
// 스마트 입력의 판정기. `NameSeed`/`WorkItem`은 이 크레이트에 흔한 낱말이 아니라
// 그대로 올라오고, 종류 열거형만 모듈 이름과 함께 불린다.
pub use workitem::{
    JiraSyncActionState, JiraSyncEvent, JiraSyncPolicy, JiraSyncTrigger, LinkedOrchestration,
    LinkedWorkItem, LinkedWorkItemSource, NameSeed, WorkItem, WorkItemKind, jira_link_id,
    jira_sync_event_id, seed_from_text,
};
// `workspace_space`의 이름 셋만 올라온다. `Entry`/`Kind`/`Status`/`Rect`는
// 이 크레이트에 이미 흔한 낱말이라 모듈 이름과 함께 불린다 — 창은
// `workspace_space::Entry`로 쓴다.
pub use workspace_space::{
    MAX_SCAN_ENTRIES, MAX_TOP_LEVEL_ENTRIES, cap_entries, format_bytes, layout_treemap,
    ready_to_delete, split_balanced,
};
pub use worktree_ownership::{
    AGENT_SCRATCH_DIRS, Facts as WorktreeFacts, Ours as WorktreeOurs, Ownership,
    Stored as ExternalVisibility, VISIBILITY_ROLLOUT_MS, Visibility, classify,
    effective_visibility, import_clears_suppression, inbox_paths, is_legacy, is_user_facing,
    merge_baseline, set_visibility, should_offer_inbox, should_offer_prompt, should_show,
};
