//! The window.
//!
//! Deliberately its own binary rather than a mode of `zerocode`. A webview is a
//! large dependency, and folding it into the terminal entry point would make
//! `zerocode status` — the command whose whole job is to answer instantly on a
//! machine that may not even have `zo` — carry a browser engine with it. The
//! headless test gate would build one too, on every run.
//!
//! What lives here is the **wiring**, nothing else: lanes, grids and the
//! session server stay in their crates, and this binary lifts the
//! [`LaneRegistry`] into Tauri state, drives it from one pump thread, and
//! forwards its events to the webview (ADR 0002 — the payloads are the crates'
//! own serde output, and the webview only paints them).

// The window is not a console app: on Windows a second black terminal appearing
// behind it is the giveaway of a port that was never finished.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use zerocode_core::conflict::{self, ConflictKind, ConflictOperation};
use zeroize::Zeroizing;
// 호스트 경계. 워크스페이스의 파일과 git이 어느 기계에 있는지는 `Host` 하나가
// 답하고, 이 파일은 그 답을 들고 다닐 뿐 어느 변종인지 묻지 않는다.
use zerocode_core::agent_teams::TeamsMode;
use zerocode_core::computer_use::RUN_EVIDENCE_DIR_ENV;
use zerocode_core::host::{self, Fs, Host, Within};
use zerocode_core::{
    AgentKind, Automation, AutomationFailureRecord, AutomationFailureStage, AutomationRun,
    CheckDetails, CheckRun, ComposerClear, Cron, DiffNote, Lane, LaneId, LocalMinute, PaneKey,
    RUNS_KEPT_PER_AUTOMATION, ReadyMark, RunTrigger, WorktreeTask, agent_may_be_saved,
    agent_presence, agent_spec, born_worktrees, close_unwitnessed, commit_failure, leaves_evidence,
    mark_completed, mark_ended, occurrence_recorded, prune_final_runs, record_run,
    reuse_normalized, should_run_precheck,
};
use zerocode_harness::{Client, method};
use zerocode_lane::{
    DEFAULT_PANE_CHANNEL_READY_TIMEOUT, LaneEvent, LaneRegistry, LaneSupervisor,
    LegacyAuthorityLease, LocalPty, PtyHandle, PtyTransport, PtyTransportError, ServeProbe,
    TOKEN_ENV, default_bind_for, project_root, project_token,
};
use zerocode_orchestrator::{
    Orchestrator, OrchestratorError, Removal, Worktree, same_worktree_path,
};
use zerocode_pty::{
    DeliveryOutcome, KeyPress, MouseEvent, MouseTracking, Observed, PromptDelivery, PtyLane,
    ReadySignal, TermHit, TermPoint, ZoBinary, encode_focus, encode_key, encode_mouse,
    encode_paste,
};
use zerocode_shell_state::AppState;

mod accounts;
mod agent_teams;
mod agent_tools_runtime;
mod agent_trust_presets;
pub(crate) mod api_routers;
mod app_paths;
mod artifact_runtime;
mod artifact_thumbs;
mod artifact_transcripts;
mod automation_runtime;
mod awake;
mod browser_cookie_import;
mod browser_cookies;
mod browser_diagnose;
mod browser_guest_runtime;
mod browser_nav_state;
mod browser_read;
mod browser_runtime;
mod browser_user_agent;
mod checks_runtime;
#[cfg(all(target_os = "macos", feature = "chromium-browser"))]
mod chromium_browser;
mod claude_tokens;
mod cli_login;
mod cmd;
mod codex_accounts;
mod codex_queue;
mod codex_tokens;
mod computer_use;
mod conversation_wake;
mod crash;
mod crash_triage;
mod credential_store;
mod crumbs;
mod developer_permissions;
mod durable_file;
mod durable_lifecycle;
mod durable_split;
mod emulator;
mod evidence_runtime;
mod explorer_policy;
mod explorer_runtime;
mod fetch_refusal;
mod file_tree_hooks;
mod file_tree_io;
mod file_tree_ops;
mod file_watch;
mod flow_console;
mod gh;
mod ghostty_import;
mod glab;
mod google_login;
mod hang_sample;
mod hang_watchdog;
mod hooks;
mod human_input;
mod icon;
mod jira_attachments;
mod jira_store;
mod keyboard_input_source;
mod last_status;
mod native_tray;
mod notify_call;
mod opencode_home;
mod orchestration;
mod orchestration_notify;
mod orchestration_pointer_mailbox;
mod pane_cwd_runtime;
mod pane_layout;
mod pane_runtime;
mod pick_runtime;
mod primary_selection;
mod proc;
mod project_runtime;
mod project_search;
mod prompt_transaction;
mod qa_triage;
mod quota_wall;
mod readiness_runtime;
mod remote_repo;
mod remote_servers;
mod remote_workspaces;
mod resource_usage;
mod restart_nudge_runtime;
mod resume_watch;
mod run_evidence;
mod save_intent;
mod scan_cell;
mod scm_observer;
mod scm_runtime;
mod scoreboard_inbox;
mod scrcpy;
mod scrcpy_video;
mod script;
mod seat_triage;
mod settings;
mod settings_runtime;
mod sftp_runtime;
mod sftp_sources;
mod shell_path;
mod shell_runtime;
mod simulator_window;
mod skills_runtime;
mod slash_catalog;
mod sqlite_read;
mod ssh_config;
mod ssh_hosts;
mod ssh_link;
mod ssh_probe;
mod ssh_prompt;
mod ssh_send_guard;
mod ssh_store;
mod stage_layout;
mod state_migration;
mod stats_events_store;
mod supply_chain;
mod system_fonts;
mod system_locale;
mod system_runtime;
mod systemone;
mod terminal_prefs_runtime;
mod terminal_registry;
mod terminal_theme_import;
mod token_scan;
mod typesafe_settings;
mod ui_source;
mod update_runtime;
mod update_store;
mod usage;
mod usage_antigravity;
mod usage_grok;
mod usage_http;
mod usage_kimi;
mod usage_oauth;
mod usage_opencode;
mod usage_places;
mod usage_runtime;
mod usage_stats_opencode_scan;
mod usage_stats_scan;
mod vault_opencode_scan;
mod vault_walk;
mod vendor_cli;
mod view_census;
mod window_runtime;
mod wire_runtime;
mod work_item_store;
mod worktree_evidence_runtime;
mod worktree_reclaim;
mod worktree_runtime;
mod worktree_shared;
mod zo_companion;
mod zo_integration_runtime;
mod zsh_wrapper;

use agent_tools_runtime::*;
use app_paths::{artifact_file, legacy_settings_file, settings_file};
use automation_runtime::*;
use browser_diagnose::*;
use browser_guest_runtime::*;
use browser_nav_state::*;
use browser_runtime::*;
use cmd::{
    abort_conflict_operation, ack_board_agent, add_claude_account, add_codex_account, agent_icon,
    agent_launch_plans, agent_models, agent_teams_mode, agent_terms, answer_approval, answer_ask,
    antigravity_usage, api_router_keys_kept, api_router_presets, api_router_providers,
    apply_ghostty_import, apply_ui_zoom, archive_trust_standing, automation_born_worktrees,
    board_columns, board_snapshot, boot_report, browse_dir, browse_places, browser_click,
    browser_console, browser_devtools, browser_diagnose, browser_eval, browser_find,
    browser_find_clear, browser_grab_arm, browser_grab_disarm, browser_grab_take, browser_history,
    browser_menu_take, browser_navigate, browser_network, browser_place, browser_profiles,
    browser_read, browser_reload, browser_scroll, browser_snapshot, browser_stop, browser_type,
    browser_wait, browser_zoom, build_stamp, cancel_folder_panel, cancel_google_login,
    check_typesafe_key, choose_paths, choose_project, claude_accounts, claude_token_usage,
    claude_usage, claude_usage_stats, clear_delivered_diff_notes, clear_diff_notes, cli_login_list,
    cli_login_logout, cli_login_start, cli_login_wait, clipboard_has_image, clone_repository,
    clone_target_name, close_browser_pane, close_lane, close_onboarding, close_term,
    codex_account_list, codex_token_usage, codex_usage, codex_usage_stats, commit_failure_card,
    commit_file_diff, commit_files, commit_staged, computer_awake_status, computer_confirm_answer,
    computer_guard_status, computer_resume, computer_stop, computer_use_capabilities,
    computer_use_permission_status, computer_use_skill_report, conflict_card, continuation_source,
    cookie_sources, crash_bundle, crash_open_log, create_browser_profile, create_project,
    create_pull_request, create_untitled_markdown, create_worktree, default_project_parent,
    default_tabs, delete_automation, delete_browser_profile, delete_diff_note,
    delete_quick_command, delete_untitled_markdown, delete_untracked,
    developer_permission_statuses, discard_paths, dismiss_external_worktree_prompt,
    enable_automation, end_all_terminal_sessions, end_terminal_session, file_diff, file_version,
    floating_workspace_seat, flow_list, flow_set, focus_lane, focus_main, fs_create, fs_duplicate,
    fs_move, fs_open_default, fs_redo, fs_rename, fs_reveal, fs_trash, fs_undo, gate_lane,
    generate_branch_name, generate_commit_message, generate_pull_request, get_second_brain_scenes,
    git_history, github_assignable_users, github_comment_work_item, github_disconnect,
    github_login_intent, github_merge_pr, github_pr_file_diff, github_preset_query,
    github_review_states, github_select_account, github_set_reviewers, github_set_work_item_open,
    github_status, github_test_connection, github_web_urls, github_work_item_detail,
    github_work_items, gitlab_comment_item, gitlab_inline_comment, gitlab_item_detail,
    gitlab_job_trace, gitlab_merge_mr, gitlab_mr_review, gitlab_pipeline_jobs,
    gitlab_project_members, gitlab_retry_job, gitlab_set_item_open, gitlab_set_mr_reviewers,
    gitlab_status, gitlab_todos, gitlab_update_mr, gitlab_work_items, google_account,
    google_login_finish, google_login_start, google_logout, grok_usage, hooks_report,
    hosted_review_eligibility, image_diff, import_browser_cookies, import_cookie_file,
    import_external_worktrees, install_bundled_skill, install_hooks, jev_summary,
    judge_worker_room, key_input, kimi_usage, lane_fold, lane_lines, lane_scroll, launch_agent_tab,
    launch_plan_for_action, launch_recipes, ledger_agents, list_agents, list_automation_runs,
    list_automations, list_branches, list_claude_sessions, list_diff_notes, list_dir,
    list_quick_commands, list_run_evidence, list_skills, list_system_fonts, list_worktrees,
    listening_ports, log_window_error, logout_codex_login, mark_default_tabs_applied,
    mark_first_run_seen, mark_onboarding, merge_and_remove_worktree, mirror_ready, mouse_input,
    note_webview_error, note_worker_room_change, notification_probe, open_board_popout,
    open_browser_pane, open_commit_remote, open_computer_use_permission,
    open_developer_permission_settings, open_download, open_lane, open_mirror_term,
    open_path_in_application, open_project, open_remote_server_session,
    open_remote_workspace_terminal, open_ssh_terminal, open_term_tab, open_terminal, open_url,
    open_workspace_in_application, opencode_usage, opencode_usage_stats, orchestration_accuracy,
    orchestration_report, orchestration_runtime_state, pane_activities, pane_agents, pane_layouts,
    pane_log, pane_sessions, pane_subagents, paste_input, patch_browser_link_routing,
    patch_browser_user_agents, patch_editing_prefs, patch_floating_workspace,
    patch_left_sidebar_appearance, patch_open_in_applications, patch_terminal_prefs,
    patch_update_prefs, patch_workspace_board_items, patch_workspace_board_status,
    patch_workspace_creation_prefs, path_kinds, paths_exist, pr_check_details, pr_checks,
    preview_ghostty_import, preview_warp_terminal_themes, probe_remote_workspace, probe_ssh_host,
    process_memory, project_catalog, project_kind, project_scripts, pull_request_seed,
    read_image_file, read_primary_selection, read_text_file, recall_folder_panel,
    record_archive_trust, record_repo_trust, relaunch_window, release_status,
    release_untitled_markdown, relogin_claude_account, relogin_codex_account, relogin_codex_login,
    remote_servers, remote_workspaces, remove_api_router, remove_claude_account,
    remove_codex_account, remove_project, remove_remote_server, remove_remote_workspace,
    remove_ssh_host, remove_typesafe_key, remove_worktree, render_mermaid, reopen_onboarding,
    reorder_projects, repo_trust_standing, request_developer_permission, reset_agent_launch,
    reset_computer_use_permissions, resize_lane, resolve_claude_account_identity, resolve_mr_base,
    resolve_pr_base, resource_snapshot, respond_permission, resume_session, resume_vault_session,
    reveal_board_agent, reveal_run_evidence, reveal_skill, reveal_vault_session, run_automation,
    save_agent_launch, save_agent_launch_env, save_api_router, save_automation,
    save_clipboard_image, save_diff_note, save_launch_recipe, save_onboarding_step,
    save_pane_layouts, save_pasted_image, save_quick_command, save_remote_server,
    save_remote_workspace, save_ssh_host, save_stage_layouts, save_typesafe_key,
    save_worktree_prefs, scm_fetch, scm_pull, scm_push, scm_status, scm_tree_rows, search_files,
    search_text, second_brain_export_html, second_brain_graph, second_brain_link,
    second_brain_open, second_brain_page, second_brain_paths, second_brain_relate,
    second_brain_seat_recalls, second_brain_setup, second_brain_status, select_claude_account,
    select_codex_account, send_prompt, session_info, set_active_worktree,
    set_agent_activity_display, set_agent_permission_mode, set_agent_teams_mode,
    set_app_font_family, set_browser_default_profile, set_browser_default_zoom,
    set_browser_home_page, set_browser_open_tabs, set_browser_restore_tabs,
    set_browser_search_engine, set_browser_visits, set_clipboard_image, set_compact_worktree_cards,
    set_computer_awake_mode, set_computer_confirm, set_confirm_close_pinned,
    set_conversation_focus_view, set_crash_watchdog, set_ctrl_tab_order_mode, set_default_agent,
    set_default_task_source, set_diff_side_by_side, set_dock_badge,
    set_external_worktree_visibility, set_guide_dismissed, set_hidden_shortcuts,
    set_hidden_task_sources, set_hide_agent_scratch_workspaces, set_hide_automation_workspaces,
    set_hide_default_branch_workspaces, set_hide_detached_head_workspaces,
    set_hide_sleeping_workspaces, set_hooks_enabled, set_jev_mode, set_jev_model,
    set_keep_default_branch_awake, set_keybinding, set_locale, set_minimize_to_tray_on_close,
    set_notification_preference, set_opencode_cookie, set_opencode_workspace, set_panel_width,
    set_panel_widths, set_previewed_terms, set_project_script_policy, set_project_script_setting,
    set_refresh_local_base_ref_on_worktree_create, set_repo_mark, set_route_classifier,
    set_second_brain_explore, set_second_brain_scenes, set_second_brain_weekly_review,
    set_setup_script_launch_mode, set_shortcut_visibility, set_show_git_ignored_files,
    set_show_menu_bar_icon, set_show_titlebar_app_name, set_sidebar_view,
    set_skip_close_terminal_with_running_process_confirm, set_skip_delete_automation_confirm,
    set_skip_delete_worktree_confirm, set_source_control_compare_base,
    set_source_control_group_order, set_source_control_view_mode, set_status_bar_item,
    set_status_bar_usage_mode, set_task_source_visibility, set_terminal_command,
    set_terminal_opacity, set_terminal_prefs, set_terminal_shortcut_policy, set_theme, set_ui_zoom,
    set_usage_analytics_enabled, set_usage_percentage_display, set_watched_terms, set_window_blur,
    set_workspace_board_column_width, set_worktree_card_property, set_worktree_compare_base,
    setup_guide, sftp_bookmark, sftp_connect, sftp_connections, sftp_disconnect, sftp_enqueue,
    sftp_job_control, sftp_jobs, sftp_list, sftp_mkdir, sftp_read_text, sftp_rename, sftp_search,
    sftp_sources, sftp_write_text, show_download, skill_bundle_parse, skill_detail,
    skill_install_plan, skill_reveal, skills_list, skills_rescan, slash_commands,
    source_control_compare_context, ssh_hosts, ssh_import_config, ssh_link_connect,
    ssh_link_disconnect, ssh_link_states, ssh_open_remote_term, ssh_probe_target,
    ssh_remove_target, ssh_save_target, ssh_submit_credential, ssh_targets, stage_layouts,
    stage_path, stage_paths, stats_summary, stop_workspace_port, subagent_log, submodule_status,
    supply_chain_graph, supply_chain_report, suppress_external_worktree_inbox, term_focus,
    term_fold, term_has_running_process, term_key, term_lines, term_mouse, term_paste, term_pull,
    term_resize, term_scroll, term_search, term_snapshot, term_text, term_view_to_line,
    terminal_command, terminal_command_argv, terminal_prefs, terminal_sessions,
    terminal_windows_status, test_local_network_permission, test_remote_server,
    test_remote_workspace, test_router_connection, test_ssh_host, text_input, tip_verdict,
    tour_decision, typesafe_settings, unstage_path, unstage_paths, update_check, update_download,
    update_history, update_install, upstream_status, use_system_claude_login, validate_branch_name,
    vault_sessions, verify_claude_accounts, verify_codex_accounts, watch_files, wire_answer,
    wire_interrupt, wire_log, wire_models, wire_send, wire_set_mode, wire_set_model, wire_start,
    wire_stop, work_item_seed, worker_screen, workspace_cleanup_scan, workspace_space_cancel,
    workspace_space_git, workspace_space_scan, worktree_committed_diff, worktree_evidence,
    worktree_last_agent, worktree_loss, worktree_prefs, worktree_stamp, write_primary_selection,
    write_text_file,
};
use cmd::{
    artifact_copy_path, artifact_counts, artifact_delete, artifact_import_transcripts,
    artifact_open, artifact_preview, artifact_register, artifact_reveal, artifact_search,
    artifact_thumbnail, artifact_versions, artifacts_list, set_artifacts_retention_days,
    set_vault_session_limit,
};
use cmd::{
    claim_coordinator_seat, coordinator_handover_status, coordinator_seat_runs,
    set_coordinator_handover,
};
use emulator::{
    acknowledge_emulator_payload, android_accessibility_tree, android_accessibility_tree_direct,
    android_button, android_button_direct, android_emulators, android_emulators_direct,
    android_install_app, android_launch_app, android_logs, android_rotate, android_rotate_direct,
    android_screenshot_direct, android_set_permission, android_swipe, android_swipe_direct,
    android_tap, android_tap_direct, android_text, android_text_direct, choose_emulator_app,
    ios_accessibility_tree, ios_accessibility_tree_direct, ios_button, ios_button_direct,
    ios_install_app, ios_launch_app, ios_logs, ios_multi_touch, ios_rotate, ios_rotate_direct,
    ios_screenshot_direct, ios_set_permission, ios_swipe, ios_swipe_direct, ios_tap,
    ios_tap_direct, ios_text, ios_text_direct, ios_touch, mobile_emulators,
    mobile_emulators_direct, open_mobile_emulator, set_emulator_stream_engaged,
    set_emulator_stream_paused, set_emulator_stream_viewport, shutdown_android_emulator,
    shutdown_mobile_emulator, start_android_stream, start_emulator_stream, start_emulator_video,
    stop_emulator_stream,
};
use pane_runtime::*;
use pick_runtime::*;
use project_runtime::*;
use save_intent::{OnDisk, SaveIntent};
use scm_runtime::*;
use settings_runtime::*;
use shell_runtime::*;
use system_locale::{LOCALES, system_locale};
use system_runtime::*;
use terminal_prefs_runtime::*;
use terminal_registry::{TerminalRegistry, lock_pty};
use terminal_theme_import::{
    CUSTOM_THEME_SELECTION_PREFIX, CustomTerminalTheme, MAX_CUSTOM_TERMINAL_THEMES,
    WarpThemeImportSource, WarpThemePreview, has_custom_theme_selection, merge_custom_themes,
    normalized_custom_themes, remove_custom_theme,
};
use usage_runtime::*;
use window_runtime::*;
use worktree_runtime::*;
#[allow(unused_imports)]
use zerocode_shell_cmd_jira::commands::{
    JIRA_SITE_ID_CHARS, JiraMethod, jira_authorization, jira_create_issue_body,
    jira_create_issue_url, jira_http_method, jira_https_url, jira_search_jql, jira_site_id,
    valid_jira_project_key,
};
use zerocode_shell_cmd_jira::commands::{
    jira_agent_context, jira_attachment_preview, jira_comment_issue, jira_connect,
    jira_create_issue, jira_disconnect, jira_issue_comments, jira_issue_detail, jira_issue_options,
    jira_issues, jira_projects, jira_search_issues, jira_select_site, jira_status, jira_sync_retry,
    jira_sync_set_paused, jira_test_connection, jira_update_issue, jira_worktree_started,
    link_jira_worktree, schedule_jira_sync, set_jira_link_sync, unlink_worktree_item,
    work_item_links,
};
use zerocode_shell_cmd_jira::{
    __cmd__jira_agent_context, __cmd__jira_attachment_preview, __cmd__jira_comment_issue,
    __cmd__jira_connect, __cmd__jira_create_issue, __cmd__jira_disconnect,
    __cmd__jira_issue_comments, __cmd__jira_issue_detail, __cmd__jira_issue_options,
    __cmd__jira_issues, __cmd__jira_projects, __cmd__jira_search_issues, __cmd__jira_select_site,
    __cmd__jira_status, __cmd__jira_sync_retry, __cmd__jira_sync_set_paused,
    __cmd__jira_test_connection, __cmd__jira_update_issue, __cmd__jira_worktree_started,
    __cmd__link_jira_worktree, __cmd__set_jira_link_sync, __cmd__unlink_worktree_item,
    __cmd__work_item_links, __tauri_command_name_jira_agent_context,
    __tauri_command_name_jira_attachment_preview, __tauri_command_name_jira_comment_issue,
    __tauri_command_name_jira_connect, __tauri_command_name_jira_create_issue,
    __tauri_command_name_jira_disconnect, __tauri_command_name_jira_issue_comments,
    __tauri_command_name_jira_issue_detail, __tauri_command_name_jira_issue_options,
    __tauri_command_name_jira_issues, __tauri_command_name_jira_projects,
    __tauri_command_name_jira_search_issues, __tauri_command_name_jira_select_site,
    __tauri_command_name_jira_status, __tauri_command_name_jira_sync_retry,
    __tauri_command_name_jira_sync_set_paused, __tauri_command_name_jira_test_connection,
    __tauri_command_name_jira_update_issue, __tauri_command_name_jira_worktree_started,
    __tauri_command_name_link_jira_worktree, __tauri_command_name_set_jira_link_sync,
    __tauri_command_name_unlink_worktree_item, __tauri_command_name_work_item_links,
};
use zerocode_shell_cmd_linear::commands::{
    linear_agent_context, linear_comment_issue, linear_connect, linear_create_issue,
    linear_disconnect, linear_issue_comments, linear_issue_detail, linear_issue_options,
    linear_issues, linear_search_issues, linear_status,
};
use zerocode_shell_cmd_linear::{
    __cmd__linear_agent_context, __cmd__linear_comment_issue, __cmd__linear_connect,
    __cmd__linear_create_issue, __cmd__linear_disconnect, __cmd__linear_issue_comments,
    __cmd__linear_issue_detail, __cmd__linear_issue_options, __cmd__linear_issues,
    __cmd__linear_search_issues, __cmd__linear_status, __tauri_command_name_linear_agent_context,
    __tauri_command_name_linear_comment_issue, __tauri_command_name_linear_connect,
    __tauri_command_name_linear_create_issue, __tauri_command_name_linear_disconnect,
    __tauri_command_name_linear_issue_comments, __tauri_command_name_linear_issue_detail,
    __tauri_command_name_linear_issue_options, __tauri_command_name_linear_issues,
    __tauri_command_name_linear_search_issues, __tauri_command_name_linear_status,
};

/// Pump cadence while something is moving. One display frame: fast enough
/// that typing feels local, slow enough that eight busy lanes cost nothing
/// measurable.
const PUMP_INTERVAL: Duration = Duration::from_millis(16);

/// Pump cadence once every surface has gone quiet.
///
/// The loop used to run at [`PUMP_INTERVAL`] forever, so a window with one
/// idle shell sitting at a prompt still took two locks and read every pty
/// sixty-two times a second. Measured against Orca on the same machine and
/// the same repository, both idle: Orca 0.1% CPU, this window 0.3%. Orca is
/// event-driven — libuv hands it the pty when there is something to read —
/// and this is the honest approximation of that without rebuilding the I/O
/// layer.
///
/// Typing is not slowed by it, because input does not wait for the next tick:
/// every command that writes to a pty calls [`Cadence::wake`] and the pump is
/// running again before the child has echoed. What this does cost is the
/// first *unsolicited* frame — an agent that starts printing on its own — by
/// up to this much, once, after which the loop is back at display rate.
const IDLE_INTERVAL: Duration = Duration::from_millis(120);

/// How many empty rounds count as "gone quiet". A few frames rather than one,
/// so a pause between two keystrokes does not drop the cadence and pay the
/// wake-up on the next letter.
const QUIET_ROUNDS: u32 = 12;

/// How closely the pump follows a wake it has not yet answered with a frame.
///
/// A wake is raised BEFORE the write it announces lands in the child, and the
/// child needs a scheduler turn before it echoes — so the round a wake starts
/// usually reads nothing, and the letter then sat out a whole
/// [`PUMP_INTERVAL`] before the next look, with the webview's own paint frame
/// after that: 16–32ms end to end, and *jittered* across that range, which is
/// what a hand feels as unnatural typing (reported: "타자치는 부분이
/// 부자연스러워"). Orca never has this beat — libuv hands its renderer the
/// bytes the moment they exist, and its output scheduler writes keystroke
/// echo straight to the parser (`pane-terminal-output-scheduler.ts`, the
/// `latencySensitive` road). The chase is that event, approximated without
/// rebuilding the I/O layer: after a wake, look again this often until
/// something arrives. Scroll rides the same chase — the wheel wakes the pump,
/// the grid moves microseconds later, and the frame that shows it goes out on
/// the next look instead of at the next display tick.
const CHASE_INTERVAL: Duration = Duration::from_millis(2);

/// How many chase looks one wake buys: one display frame's worth, so
/// [`CHASE_ROUNDS`] × [`CHASE_INTERVAL`] = [`PUMP_INTERVAL`] and a wake whose
/// answer never comes costs exactly one frame of brisk looking before the
/// ordinary beat resumes.
const CHASE_ROUNDS: u32 = 8;

/// How long one round may spend draining children before it has to go paint.
///
/// Until this existed, a round moved [`zerocode_pty::PTY_OUTPUT_BYTE_BUDGET`]
/// — 256 KiB — per shell and then slept [`PUMP_INTERVAL`] whatever was left
/// waiting. That is a **throughput ceiling of 16 MiB/s per shell**, and it is
/// nowhere near what the parser can do: measured on this machine, the grid
/// eats 80.9 MiB/s of plain text at 60×220 and 89.8 MiB/s of build log, so the
/// ceiling sat at a fifth of the engine behind it. `cat` of a 10 MiB log took
/// 640 ms of which 124 ms was parsing; the other 516 ms was this loop asleep
/// on a pty that had more to say. A terminal that is not ours reads until the
/// read would block and paints on the next vsync, and the gap between those
/// two sentences is what "출력이 기본 터미널보다 버벅거린다" names.
///
/// The information was always there and was thrown away: `pump` returns
/// [`zerocode_pty::Pumped::bytes`], and `bytes == budget` means "this child
/// has more" — the round read it as `pumped.bytes > 0`, a bool, and napped.
///
/// So the drain is budgeted in TIME rather than in bytes. A round keeps
/// pumping a saturated shell until it comes up short or this much of the round
/// is gone, which makes throughput a function of the parser instead of a
/// function of the nap.
///
/// Measured, `cat` of a 10 MiB file of 100-column lines through a real pty,
/// best of three, this loop's cadence simulated around
/// [`zerocode_pty::PtyLane::pump`]:
///
/// | drain | 24×80 | 60×220 | rounds |
/// |---|---|---|---|
/// | none (the old 256 KiB a round) | 773 ms, 13.2 MiB/s | 777 ms, 13.1 MiB/s | 42 |
/// | 4 ms | 385 ms, 26.5 MiB/s | 403 ms, 25.3 MiB/s | 22 |
/// | **8 ms** | **253 ms, 40.4 MiB/s** | **256 ms, 39.9 MiB/s** | **15** |
/// | 12 ms | 154 ms, 66.2 MiB/s | 192 ms, 53.1 MiB/s | 10 |
///
/// Three times faster, and the second column is the half nobody asked for:
/// the same output crosses in 15 rounds instead of 42, so a flood costs the
/// webview **a third of the frames** it used to. Draining and painting pull
/// the same way here — a terminal that is not ours reads until the read would
/// block and paints on the next vsync, and this is that sentence.
///
/// Half a display frame, not the whole one, and 12 ms is left on the table on
/// purpose: the other half is what the rest of the round and the webview's own
/// paint are owed, and a drain allowed to eat the frame would trade a slow
/// `cat` for a window that stops answering during one. The fairness the byte
/// budget bought is unchanged — every shell is still visited once per round,
/// and no shell can be drained past the deadline the shells before it have
/// already spent.
const DRAIN_BUDGET: Duration = Duration::from_millis(8);

/// How often the pump asks the terminals which process group is holding them.
///
/// The cadence half of the agent-departure watch — the count half is
/// `zerocode_core::agent_exit::LOOKS_BEFORE_GONE`, and the two multiply out to
/// how long a lying badge can stand. Named from the core constant rather than
/// spelled twice, because the product of the two IS the behaviour and a drift
/// between them would change it silently.
const FOREGROUND_LOOK_EVERY: Duration =
    Duration::from_millis(zerocode_core::agent_exit::LOOK_EVERY_MS);

/// How often the pump asks the operating system whether a quiet child is
/// actually still alive.
///
/// Not every round: this is `waitpid` per shell, on the render path, and a
/// shell exiting is rare. A second is far below the threshold at which
/// somebody reads a pane as stuck, and it costs one syscall per shell per
/// second rather than sixty.
const REAP_LOOK_EVERY: Duration = Duration::from_secs(1);

/// How often every standing order gets a beat.
///
/// The fifth gate on the pump's clock, beside the schedule sweep and the reap
/// look, and slow for the same reason they are: what it asks — is a run under
/// its ceiling with work waiting — changes when a worker starts or finishes,
/// which is a human number of times a minute, not sixty times a second.
///
/// A second is also the shortest gap that can be honest. Cutting a pane forks
/// and execs, so a beat can outlast the gap between two beats; the tick holds
/// its own door against that, and a slower clock means it is rarely asked to.
const AUTO_BEAT_EVERY: Duration = Duration::from_secs(1);

/// How often a text-generation child is checked for completion.
///
/// This happens to match [`IDLE_INTERVAL`], but it is a separate policy: one
/// budgets subprocess observation while the other controls an idle UI pump.
const GENERATION_POLL_EVERY: Duration = Duration::from_millis(120);

/// How long importing a terminal theme may spend building its preview.
///
/// Warp and Ghostty previews do the same bounded filesystem work; their two
/// timeout sites must move together.
const THEME_PREVIEW_BUDGET: Duration = Duration::from_secs(5);

/// How long WKWebView may take to deliver a native snapshot.
///
/// A missing callback must release its blocking receiver instead of holding a
/// runtime worker forever.
const SNAPSHOT_BUDGET: Duration = Duration::from_secs(10);

/// Agent screenshots are local artifacts, not an unbounded binary response.
/// This matches the emulator frame ceiling and comfortably covers a native
/// full-resolution phone or browser viewport.
const AGENT_SCREENSHOT_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Initial rows for a person-facing pty before its surface reports geometry.
const PTY_BIRTH_ROWS: u16 = 24;
/// Initial columns for a person-facing pty before its surface reports geometry.
const PTY_BIRTH_COLS: u16 = 96;

/// Rows for the hidden Claude and Codex usage probes.
///
/// These probes need enough screen for the CLI's whole usage answer but are
/// not person-facing panes, so their geometry is intentionally independent
/// from [`PTY_BIRTH_ROWS`] and [`PTY_BIRTH_COLS`].
const PROBE_PTY_ROWS: u16 = 40;
/// Columns for the hidden Claude and Codex usage probes.
const PROBE_PTY_COLS: u16 = 120;

/// The pump's clock, and the way anything else can hurry it along.
///
/// A condition variable rather than a shared deadline: a keystroke arriving
/// during an idle sleep has to *interrupt* it, not merely change what the
/// next sleep will be. Storing a flag would leave the first letter after a
/// pause waiting out the remainder of a 120ms nap.
#[derive(Default)]
struct Cadence {
    stirred: Mutex<bool>,
    signal: Condvar,
}

impl Cadence {
    /// Something was sent to a child. Run now.
    fn wake(&self) {
        let mut stirred = self
            .stirred
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *stirred = true;
        self.signal.notify_one();
    }

    /// Sleep until `how_long` passes or somebody calls [`Cadence::wake`],
    /// and say which of the two ended it.
    ///
    /// The flag is checked before waiting and cleared after: a wake that
    /// lands between two rounds must not be lost, or the keystroke that
    /// raised it waits for the timeout anyway.
    ///
    /// The answer matters because a wake is raised *before* the write it
    /// belongs to reaches the child. Waking the loop for one round is not
    /// enough on its own — that round can take the registry lock first, find
    /// nothing, and put the loop straight back to sleep with the flag already
    /// spent, which is how an echo ended up waiting out a whole idle nap. The
    /// caller uses this to go back to display rate instead.
    fn rest(&self, how_long: Duration) -> bool {
        let mut stirred = self
            .stirred
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !*stirred {
            let (guard, _) = self
                .signal
                .wait_timeout(stirred, how_long)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stirred = guard;
        }
        std::mem::replace(&mut stirred, false)
    }
}

/// A shell this window hosts. Not a [`LaneId`]: a lane is an agent session the
/// daemon owns, and these are plain terminals that live and die with the
/// window.
type TermId = u32;

/// The floating panel's shell, which existed before terminals could be tabs
/// and keeps its id so nothing about it had to change.
const FLOAT_TERM: TermId = 0;

/// A lane bell can never wear the interrupted word.
///
/// Not a default and not a "we do not know yet": an interrupt is a fact
/// about a PANE — a person pressed a key in a terminal — and a lane has no
/// terminal for anybody to press a key in. Named so the one call site says
/// which of those two it means, because a bare `false` there reads as the
/// other one.
const LANE_NEVER_INTERRUPTED: bool = false;

/// Launch state that must outlive the fenced spawn but never the readiness
/// verdict. The receiver is taken by `Host::await_worker_ready` after the team
/// table has been released; keeping the cleanup authority beside it makes the
/// failure path one operation rather than four maps that can drift apart.
struct PendingWorkerReadiness {
    waiting: std::sync::mpsc::Receiver<DeliveryOutcome>,
    submitted: Option<std::sync::mpsc::Receiver<()>>,
    deadline: Instant,
    team: String,
    pane: String,
    previous_pane_token: Option<String>,
    isolated: Option<IsolatedWorkerCheckout>,
    /// A restore: the process was resumed into this pane and its readiness
    /// wait carries no briefing — see `await_worker_ready`, which will not
    /// tear down a restored worker that is alive and at work.
    restoring: bool,
}

struct WorkerReadinessSlot {
    /// Taken by the post-fence waiter, leaving the slot itself as the reaper's
    /// marker that this terminal has not been committed as a worker yet.
    pending: Option<PendingWorkerReadiness>,
    /// A process may exit before the waiter runs. The reaper owns the screen
    /// in that race and leaves it here for the typed refusal.
    exited_screen: Option<String>,
}

/// A successful worker waiting for its final turn-end hook before cleanup.
/// The hook is the acknowledgement that `worker_done` returned to the agent;
/// closing at command receipt would kill the reporting CLI mid-response.
#[derive(Clone)]
struct IsolatedWorkerCheckout {
    orchestrator: Orchestrator,
    path: PathBuf,
}

struct CompletedWorkerCleanup {
    worker: String,
    checkout: Option<PathBuf>,
    isolated: Option<IsolatedWorkerCheckout>,
}

/// One interactive Zo process whose pid-published channel this window is
/// adopting. `session == None` is the single in-flight discovery attempt;
/// keeping that state in the same map prevents the foreground beat from
/// opening a new connection every 500 ms while `session.info` is pending.
struct ZoPaneAdoption {
    pid: u32,
    file: PathBuf,
    session: Option<String>,
    owns_subscription: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ZoChannelOwner {
    Lane(LaneId),
    Term(TermId),
}

struct ForegroundAgentLook {
    term: TermId,
    child_in_front: Option<bool>,
    agent: Option<&'static str>,
    process: Option<u32>,
}

/// Confirm that a submitting Enter was consumed, retrying that Enter once
/// when a capable provider stays silent for one normal TUI quiet window.
/// Providers without a prompt-submit hook pass `None` and keep the terminal
/// delivery contract they can actually prove.
fn await_prompt_submission(
    submitted: Option<&std::sync::mpsc::Receiver<()>>,
    deadline: Instant,
    acknowledgement_window: Duration,
    retry_enter: impl FnOnce() -> bool,
) -> bool {
    let Some(submitted) = submitted else {
        return true;
    };
    let first_wait = acknowledgement_window.min(deadline.saturating_duration_since(Instant::now()));
    match submitted.recv_timeout(first_wait) {
        Ok(()) => true,
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => false,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            retry_enter()
                && submitted
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .is_ok()
        }
    }
}

/// The reason a worker launch gives when its briefing never went in, naming
/// the door the readiness wait was still at when there is one to name.
///
/// "did not accept its briefing: Err(Timeout)" was all three failed launches
/// of 2026-09-17 said, while each pane had shaken hands and was answering a
/// cursor poll five times a second — the wait knew, and the sentence did not.
fn briefing_refusal(
    delivered: &Result<DeliveryOutcome, std::sync::mpsc::RecvTimeoutError>,
    unmet: Option<zerocode_pty::ready::Unmet>,
) -> String {
    let why = unmet.map_or_else(|| format!("{delivered:?}"), |unmet| unmet.to_string());
    format!("the worker TUI did not accept its briefing: {why}")
}

struct ShellRuntime {
    /// The desktop directories resolved once from Tauri during `setup`.
    /// Compatibility mode currently points every class at the same legacy
    /// root; the migration gate changes that for both binaries together.
    paths: app_paths::AppPaths,
    /// Linux PRIMARY selection handle. Other platforms keep the same typed
    /// state but use the webview's private selection buffer.
    primary_selection: primary_selection::PrimarySelection,
    /// Keeps the legacy authority stable for this window's whole lifetime
    /// when a recoverable migration/preflight error leaves the old profile
    /// active. A future process cannot switch to platform paths until every
    /// such window and CLI writer has released its shared lease.
    _legacy_authority: Mutex<Option<LegacyAuthorityLease>>,
    /// `None` on a machine without `zo` — the lane surfaces fold away and the
    /// window still opens (F2b).
    supervisor: Option<LaneSupervisor>,
    /// A discovered harness whose persisted token could not be read or
    /// written. This disables only lane surfaces; editor, settings, git and
    /// plain terminals still open, and the canonical token is never replaced
    /// with an identity the detached server cannot know.
    supervisor_unavailable: bool,
    registry: Mutex<LaneRegistry>,
    /// Every shell this window is hosting, by id. A **real shell** in the
    /// checkout being looked at, not a lane: `claude`, `zo`, anything on
    /// `PATH` runs here — the window never decides what a terminal is for.
    ///
    /// A pool rather than one, because Orca opens terminals as tabs and each
    /// tab is its own process (docs/reverse/orca-ui-inventory.md 1-e). The
    /// floating panel is [`FLOAT_TERM`] and keeps the id it always had; every
    /// `⌘T` takes the next one. An entry disappears when its shell exits.
    ///
    /// Sharded, not one lock: the registry's map lock covers only membership,
    /// and each shell carries its own lock for the real work, so a keystroke
    /// never waits out another terminal's parse. The lock order lives on
    /// [`TerminalRegistry`].
    terminals: TerminalRegistry,
    /// The sixty-sample memory histories and Windows CPU baseline used only by
    /// the resource-manager popover. Process discovery itself runs outside
    /// this lock; the state retained here is small and bounded.
    tree_ops: Mutex<file_tree_ops::History>,
    tree_hooks: Mutex<file_tree_hooks::Pending>,
    resource_usage: Mutex<resource_usage::ResourceUsageState>,
    /// Where the floating shell was seated when it spawned, as its header
    /// says it — kept so reopening the panel (or a reloaded webview) names
    /// the running shell's real seat instead of today's setting. Never
    /// cleared on exit: the only reader runs right after the spawn-or-running
    /// check, and a respawn overwrites it before anyone can read it stale.
    float_seat: Mutex<Option<String>>,
    /// The id the next terminal tab gets. Never reused: an id that came back
    /// would let a frame for a shell that ended paint into its replacement.
    next_term: Mutex<TermId>,
    /// Which shells each OS window is actually READING, by webview label.
    ///
    /// The pump used to hand every window every shell's frame, and with four
    /// agents streaming at once that is the whole of the typing lag people
    /// reported: a webview deserialised a frame on the very thread that owes
    /// the next keystroke, and a screen behind another tab threw it away
    /// *after* paying for it. So a shell's readers are exactly the windows
    /// named here (`zerocode_pty::readers`), and nothing else changes — every
    /// pty is still read and every grid still parses every byte, because a
    /// background shell's scrollback has to be right the moment somebody looks
    /// at it.
    ///
    /// **Declarative, per window**: a window sends its CURRENT whole set on
    /// every visibility change, so the entry is replaced rather than adjusted.
    /// A count that goes up and down would drift the first time a window
    /// missed a decrement — and the symptom of that drift is a terminal that
    /// silently stops painting, which is the worst bug this door can have.
    /// Keyed by label because there are two windows (the main one and the
    /// board pop-out, which borrows a live screen into its preview) and either
    /// may read a shell; a window's entry goes when its window does.
    watched_terms: Mutex<HashMap<String, HashSet<TermId>>>,
    /// Which repository owns each checkout the last `project_catalog` listed,
    /// by the path the sidebar row carries. A workspace click resolves its
    /// path here first and asks git only for a path this answer does not name.
    catalog_owners: Mutex<HashMap<String, (Orchestrator, Worktree)>>,
    /// The shells a window is PREVIEWING — the board's cards, which show every
    /// running agent at once.
    ///
    /// A second tier, and it exists because the first one is all-or-nothing:
    /// the stage unhides one tab's panes, so "reading" is one shell however
    /// many agents are working, and every other one's frames stopped dead at
    /// the pump. The board is the surface that answers "what is everyone
    /// doing", and it could not be built out of a set that never holds more
    /// than one.
    ///
    /// Same shape and same contract as [`AppState::watched_terms`]: declared
    /// whole, keyed by the window's own label, gone when that window is. A
    /// term in BOTH is watched — the full tier wins, or one screen would take
    /// a delta and a snapshot by turns and tear between them.
    previewed_terms: Mutex<HashMap<String, HashSet<TermId>>>,
    /// Sessions whose structured channel already has a subscriber thread.
    ///
    /// One set, two jobs. It **refuses a second subscriber** for a session:
    /// two would deliver every `permission_prompt` twice, and answering the
    /// duplicate would carry a `prompt_id` the server has already retired.
    /// And dropping an id from it is how [`close_lane`] tells that thread to
    /// stop — the loop checks membership before forwarding a frame, so a lane
    /// the person closed cannot raise a modal afterwards.
    subscriptions: Arc<Mutex<HashSet<String>>>,
    /// Each pane-owned Zo session's private events-channel address.
    ///
    /// Unlike the retained `zo serve` supervisor used by remote lanes, a local
    /// IDE pane opens one socket per process. Permission replies, status reads,
    /// and subscriptions must all return to that exact address.
    channels: Arc<Mutex<HashMap<String, String>>>,
    /// Which lane or terminal won each session-id subscription claim.
    channel_owners: Mutex<HashMap<String, ZoChannelOwner>>,
    /// The project this window is open on — the repository root when there is
    /// one, else the working directory.
    ///
    /// Fixed for the window's life: the serve is keyed to it and the
    /// orchestrator was opened on it, so it is the window's identity rather
    /// than its viewport.
    project_root: PathBuf,
    /// The open file tabs, watched for the writes this window did not make.
    ///
    /// Shared with the poller thread `setup` spawns — the [`watch_files`]
    /// command replaces the set, the thread stats it and emits `fs:changed`.
    /// An `Arc` because the thread outlives every borrow the state could
    /// hand it.
    watched: Arc<file_watch::WatchSet>,
    /// The checkout the file surfaces are showing, and the repository that
    /// owns it. See [`ActiveContext`] for why the two are one lock.
    ///
    /// Every path command resolves against this, so choosing a worktree moves
    /// the tree, the search, the viewer and the git badges together. Split
    /// from `project_root` because they answer different questions — a lane
    /// can be running in a worktree the person is not currently looking at.
    active: Mutex<ActiveContext>,
    /// Prompts on their way into shells, by the shell they are addressed to.
    ///
    /// One ACTIVE per shell: a second prompt for a terminal that is still
    /// swallowing the first would interleave two bracketed envelopes, and the
    /// child would read the pair as one corrupt paste. Later prompts wait in
    /// [`AppState::prompt_queue`] — each sequence owns the line until its
    /// Enter fires, then the next takes it.
    ///
    /// The pump turns these, because the pump is already the one thing that
    /// looks at every terminal at the display rate. A thread per pending
    /// prompt would be a second cadence to keep honest.
    deliveries: Mutex<HashMap<TermId, PromptDelivery>>,
    /// Restart continuations awaiting the resumed agent's first working hook.
    pending_nudges: Mutex<restart_nudge_runtime::PendingNudges>,
    /// Fresh Zo worker briefings held until their pane event subscriber has
    /// completed `session.subscribe`.
    ///
    /// The composer remains idle while this map owns the delivery. Releasing
    /// it any earlier can start the turn before the only observer of
    /// `turn{phase:"start"}` exists, leaving readiness to time out and tear
    /// down a worker that is already doing its task.
    zo_worker_deliveries: Mutex<HashMap<TermId, PromptDelivery>>,
    /// Completion observers for launch-time deliveries whose caller must not
    /// report a worker ready before its briefing actually reached the TUI.
    /// Ordinary sends have no observer and keep their asynchronous event path.
    delivery_waiters: Mutex<HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>>,
    /// Worker starts waiting for the launch-time delivery's verdict.
    ///
    /// Separate from `delivery_waiters`: that map is written by the pump,
    /// while this one owns the receiver and the resources to roll back. Most
    /// importantly, it is consumed only after the team-table fence is gone.
    worker_readiness: Mutex<HashMap<TermId, WorkerReadinessSlot>>,
    /// Prompt-submit acknowledgements for providers whose lifecycle transport
    /// has that event. Armed before delivery, consumed by the first matching
    /// hook report or Zo turn-start frame.
    worker_prompt_submits: Mutex<HashMap<TermId, std::sync::mpsc::SyncSender<()>>>,
    /// Successful, non-retained workers awaiting their final Done event.
    completed_worker_cleanups: Mutex<HashMap<TermId, CompletedWorkerCleanup>>,
    /// Terms whose checkout was cut specifically for an orchestration worker.
    /// Placement is the window's fact, so cleanup authority lives here rather
    /// than being duplicated into the durable work ledger.
    isolated_worker_terms: Mutex<HashMap<TermId, IsolatedWorkerCheckout>>,
    /// The prompts each shell will take NEXT, in the order they were asked
    /// (Orca's per-PTY send queue, native-chat-pty-send-queue.ts:1-4: without
    /// one, a second send raced the first Enter's window). [`send_prompt`]
    /// used to refuse instead, which turned every double-send in an
    /// automation into an error somebody had to retry by hand.
    ///
    /// A queued prompt is stored as its INGREDIENTS, not as a built
    /// [`PromptDelivery`]: the delivery's readiness clock starts at
    /// construction, and a prompt built while parked would spend its whole
    /// timeout waiting for its turn.
    prompt_queue: Mutex<HashMap<TermId, VecDeque<QueuedPrompt>>>,
    /// Which agent a terminal was STARTED as, for the ones that were.
    ///
    /// Written at spawn, from the argv this window itself assembled — which
    /// is surer knowledge than Orca has: it sniffs run-time evidence (agent
    /// status hooks, then tab-title hints — `deriveNotesSendAgentTargets`)
    /// because its terminals can change what they run. Ours can too, so this
    /// map can go stale the same way a title hint can; it answers "what was
    /// launched", and the send path still waits for the agent to actually
    /// listen before typing at it.
    agent_terms: Mutex<HashMap<TermId, &'static str>>,
    /// The nonce each launched agent was handed, by the shell it runs in.
    ///
    /// A hook event carries the token its agent was started with. Relaunching a
    /// pane gives the new occupant a new one, so a slow event from the old
    /// occupant — a `Stop` that took the network's time to arrive — can be told
    /// from the live agent's and dropped, instead of painting the tab done while
    /// the agent that is actually there is working.
    launch_tokens: Mutex<HashMap<TermId, String>>,
    /// The untitled markdowns THIS window minted and has not yet let go of.
    ///
    /// The delete road below only ever removes a path that is in here — the
    /// webview's word alone must not delete files, and the set of files this
    /// feature may take back is exactly the set it created (1-fw).
    untitled_markdowns: Mutex<std::collections::HashSet<String>>,
    /// The browser panes THIS window minted — the markdown mint's posture, one
    /// door over (1-fy): steer/close commands name a pane by label over
    /// loopback IPC, and the only labels they may touch are the ones this
    /// window handed out. Without the set, any local process that can knock
    /// could close webviews — including the main one — by guessing labels.
    browser_panes: Mutex<std::collections::HashSet<String>>,
    /// Where each browser pane stands, as its OWN page-load events last said
    /// (opened at, navigated to, loaded). The window never asks a webview for
    /// its URL: wry unwraps `WKWebView.URL`, which is nil after a failed
    /// provisional navigation, on the main thread — the whole window died
    /// three times in ten minutes on 2026-09-07 answering `zerocode-browser
    /// list` over a tab whose dev server had gone.
    browser_urls: Mutex<HashMap<String, String>>,
    /// The next browser pane's number. Monotonic, never reissued: a label that
    /// came back around to a different page would let a stale event repaint
    /// the wrong tab.
    browser_born: Mutex<u64>,
    /// Each minted pane's birth facts, title and navigation record — what
    /// `tabs` and `diagnose` answer from, and where the dead-load clock
    /// writes. Never asked of the webview (`the_shell_never_asks_a_webview_
    /// for_its_url`); every field is something a hook said.
    browser_records: Mutex<HashMap<String, BrowserPaneRecord>>,
    /// Labels the agents' door reserved and the window has not yet claimed
    /// — `zerocode-browser open` answers its label before the pane exists,
    /// and `open_browser_pane` accepts a label only from this set.
    browser_reserved: Mutex<std::collections::HashSet<String>>,
    /// The agent's own session id for each pane that reported one.
    ///
    /// The vendor's handle on the conversation, which outlives our process — so
    /// this is what lets a tab be closed and the SAME conversation reopened.
    /// Written from hook events, never guessed: an id we invented would resume
    /// nothing.
    pane_sessions: Mutex<HashMap<TermId, zerocode_core::ProviderSession>>,
    /// What each pane's agent last reported, and when.
    ///
    /// The window is told every state change as it happens, so this is not how
    /// the tabs get painted — it is how a surface that opens LATER knows what is
    /// going on. The board is that surface: opened after four agents have been
    /// running for an hour, it has to be right immediately, and the events that
    /// would have told it are long past.
    ///
    /// Held beside the launch token and the session for the same reason they are
    /// here: all three are facts about a running agent, and they have to be
    /// forgotten together when the pane closes.
    pane_states: Mutex<HashMap<TermId, PaneState>>,
    /// 재시작을 살아남는 `pane_states`의 그림자 (P0-13) — Orca의
    /// last-status.json 원장. 열쇠는 `last_status::key(worktree, term)`,
    /// 값은 병합이 끝난 마지막 소식의 사본이다. 지우는 문은 두 개: 부팅
    /// 하이드레이트의 7일 TTL(`last_status::HYDRATE_MAX_AGE`), 그리고 같은
    /// 열쇠를 덮는 다음 소식 — 판/워크트리가 유한하니 파일도 유한하다.
    last_statuses: Mutex<HashMap<String, last_status::LastStatus>>,
    /// 이 프로세스가 각 판을 원장의 **어느 자리**에 앉혔는지 — 봉투가 없는
    /// 소식이 고칠 행을 추측하지 않기 위해서만 있다.
    ///
    /// 원장의 열쇠는 워크트리를 품고 판 번호는 그 뒤에 붙는데, 번호는
    /// 재시작마다 `next_term`의 `FLOAT_TERM + 1`에서 다시 발급된다. 그래서
    /// 번호만으로는 "어느 워크트리의 몇 번"이 정해지지 않고, 하이드레이트로
    /// 올라온 지난 세션의 행들까지 같은 번호를 들고 함께 앉아 있다. 이
    /// 프로세스가 자기 손으로 적은 자리를 기억해 두면 찾을 것이 하나뿐이다.
    last_status_seats: Mutex<HashMap<TermId, String>>,
    /// 마지막으로 디스크에 눕힌 바이트 — 같은 내용을 두 번 fsync하지
    /// 않기 위한 동일성 스킵(Orca `lastWrittenJson`, server.ts:3204).
    last_status_written: Mutex<Option<Vec<u8>>>,
    /// 원장 쓰기의 세대 — popout bounds와 같은 디바운스 문법: 마지막
    /// 소식 뒤 `PERSIST_DEBOUNCE`가 지나야 적고, 그 사이의 새 소식은
    /// 앞선 잠을 무효로 만든다.
    last_status_writes: std::sync::atomic::AtomicU64,
    /// 마지막으로 울린 벨의 주소 (P0-14) — macOS `Reopen`이 사람을
    /// 데려다줄 곳. 벨이 발사됐다는 사실이 곧 "그 자리를 보고 있지
    /// 않았다"이므로(초점이었다면 suppressed), 복귀 안내에는 이 하나면
    /// 충분하다. 소비되면 비워진다: 한 번의 복귀는 한 번의 안내다.
    last_ring: Mutex<Option<LastRing>>,
    /// What the pty's foreground group has been saying about each pane that
    /// claims to hold a running agent.
    ///
    /// The memory behind the debounce — see `zerocode_core::agent_exit`, which
    /// owns the judgement; this field is only the lock around it and the map
    /// from pane to run. Entries exist only while a pane is being watched: one
    /// appears when a pane's state first claims an agent, and it goes when the
    /// verdict lands, so a pane that has been declared empty starts a fresh
    /// watch the moment somebody runs another agent in the same shell.
    ///
    /// Held beside the four maps above and forgotten with them when the pane
    /// closes.
    foreground_watch: Mutex<HashMap<TermId, zerocode_core::ForegroundWatch>>,
    /// Panes whose agent claim came from the KERNEL rather than from a launch
    /// or a hook — see `sweep_arrived_agents`.
    ///
    /// Kept apart so that sweep can retract only its OWN claims: an agent this
    /// window launched, or one that reported a hook, has its own departure
    /// rules and must never be cleared because a shell happened to be in the
    /// foreground for one look.
    foreground_agents: Mutex<HashMap<TermId, zerocode_core::ForegroundWatch>>,
    /// Panes whose pty child is an interactive shell (`spawn_shell`): the
    /// panes where "the child holds the terminal" means nothing is running in
    /// front of the shell. Everywhere else the child in front is the agent
    /// itself, or a wrapper without job control that proves nothing — see
    /// `agent_teams::Host::shell_in_front`. Forgotten when the pane closes.
    shell_panes: Mutex<std::collections::HashSet<TermId>>,
    /// Private Zo channels discovered from interactive processes already
    /// running inside ordinary terminal panes.
    zo_adoptions: Mutex<HashMap<TermId, ZoPaneAdoption>>,
    zo_integrations: Mutex<zo_integration_runtime::Integrations>,
    /// The nested-agent runs whose viewer pages are open — run id to the pane
    /// and vendor that started it, so a finish without an id in its payload
    /// can still find its page (1-fk).
    workers: Mutex<HashMap<String, (TermId, &'static str)>>,
    /// Which pane asked for each pane — the board's subagent tree, at its one
    /// source.
    ///
    /// Written where a pane is BORN of another pane and nowhere else, because
    /// that is the only moment the fact exists: once both are running they are
    /// two agents in two panes and nothing about either says which came first.
    /// Orca reads the same fact out of an orchestration database it keeps for
    /// other reasons; this window has no such database, so the launch writes
    /// it down as it happens.
    ///
    /// Held beside the three above, and forgotten with them when the pane
    /// closes. Never persisted: a parent that outlived the process would point
    /// at a terminal id this run has not issued yet.
    pane_parents: Mutex<HashMap<TermId, TermId>>,
    /// Which helper each pane IS, for the panes that are one — zo's pane lane
    /// cuts a helper a pane of its own and names it on the split
    /// (`-e ZO_AGENT_ID`, t-3024). The pane was the helper, so the pane
    /// ending is the helper finishing (t-3098): `forget_term_state` retires
    /// it in its parent's roster by this id. Written beside `pane_parents`,
    /// forgotten with it, never persisted, and empty for every pane that
    /// is nobody's helper, which is most of them.
    pane_helpers: Mutex<HashMap<TermId, String>>,
    /// Parents whose helper roster a state door changed with no
    /// `AppHandle` to tell the window — `forget_term_state` retiring a
    /// helper whose pane ended. The pump pays each one through the roster's
    /// one emit (`publish_owed_rosters`) and drains the set as it goes.
    unpublished_rosters: Mutex<std::collections::HashSet<TermId>>,
    /// Where each held pane's foreground process last stood (pane_cwd_runtime).
    pane_cwds: Mutex<HashMap<TermId, String>>,
    /// The team environment each pane was started with, for the panes that
    /// belong to an orchestrating agent's team.
    ///
    /// Kept because a teammate can open a teammate: the second split has to
    /// carry the same team id, token and `TMUX` as the first, and the only
    /// place those exist after the launch is the environment they were handed
    /// in. Empty for every pane that is not part of a team, which is most of
    /// them.
    team_envs: Mutex<HashMap<TermId, Vec<(String, String)>>>,
    /// The helpers running inside each pane's agent — see `subagents()`.
    subagents: Mutex<HashMap<TermId, Vec<hooks::SubagentRow>>>,
    /// A lead's `Done` the roster held back, parked until the last helper's
    /// stop replays it as the all-clear — see `pending_done()`. Beside the
    /// report, the worktree its ring belongs to.
    pending_done: Mutex<HashMap<TermId, (String, hooks::PaneHookReport)>>,
    /// The last few things each agent DID, by card — see `activities()`.
    activities: Mutex<HashMap<String, ActivityRing>>,
    /// Which [`answer_ask`] walk each pane is on. A new answer bumps the
    /// pane's generation and the previous walk's thread stops at its next
    /// step — Orca's `cancelInFlight`, held here because the timers are here.
    ask_sends: Mutex<HashMap<TermId, u64>>,
    /// Each pane's stop-gesture inference — the per-terminal machine that
    /// decides whether an Escape or a Ctrl+C meant "stop this turn"
    /// ([`zerocode_core::interrupt::InterruptInference`]).
    ///
    /// Here rather than in core for the same reason `ask_sends` is: the rules
    /// are pure and the CLOCK is ours.
    interrupt_inference: Mutex<HashMap<TermId, zerocode_core::interrupt::InterruptInference>>,
    /// Which settle each pane is on, in the shape `ask_sends` already uses. A
    /// newer gesture bumps the generation and the sleeping thread from the
    /// older one finds its stamp gone and says nothing — otherwise two
    /// gestures 200ms apart both flush and the second reads a baseline the
    /// first already spent.
    inference_sends: Mutex<HashMap<TermId, u64>>,
    /// When each worktree last rang a notification — the cooldown's memory.
    rings: Mutex<zerocode_core::notify::RingLedger>,
    /// What the notify seat remembers about the rings it asked about: the
    /// person's last hand on the window, each pane's last rings, the rows
    /// waiting for their label and the rings held for the next hand
    /// (t-6043). The decisions live in `notify_call`; this is the lock.
    notify_book: Mutex<notify_call::NotifyBook>,
    /// The bell's memory of each lane, so a ring follows a CHANGE.
    ///
    /// The decision itself — first appearance, repeat, straggler after death —
    /// lives in `zerocode_core::notify::LaneBell`, where it is behaviour
    /// tested; this field is only the lock around it. The lock also carries
    /// the ordering rule: the bell is taken before the registry, never while
    /// the registry is held.
    lane_bell: Mutex<zerocode_core::notify::LaneBell>,
    /// The macOS menu-bar and Windows system-tray lifecycle. Its controller
    /// serializes native state and close policy with settings persistence.
    native_tray: native_tray::NativeTray,
    /// The window's one hold on the power system — fed by the same door
    /// every pane state walks through (P0-17).
    awake: awake::AwakeService,
    /// The last commit git refused, and what it was carrying.
    ///
    /// One, not a list: the card sits under the commit box and there is one
    /// box. Cleared by the commit that succeeds — a card offering to fix a
    /// failure that has since been fixed is worse than no card.
    commit_failure: Mutex<Option<CommitFailure>>,
    /// The pump's clock. Written to by every command that sends something to
    /// a child, so the loop is running again before the echo comes back.
    cadence: Cadence,
    /// Pending runtime SSH credential questions (P0-20). The connect roads
    /// ask through it and `ssh_submit_credential` answers into it; see
    /// `ssh_prompt` for the ported Orca contract.
    ssh_credentials: ssh_prompt::CredentialBroker,
    /// Whether this process actually got the blurred window material.
    ///
    /// Not the same as the stored setting: the material is asked for once, on
    /// the way up, so a switch flipped since then is an intent and this is the
    /// fact. The settings screen shows Orca's "Requires restart" banner on
    /// exactly the disagreement between the two.
    window_blur_active: bool,
    /// SSH metadata is part of the settings document; this service owns only
    /// its OS-native credential half.
    ssh_hosts: ssh_hosts::SshHostService,
    /// What each saved SSH target's connection is doing right now. Memory
    /// only: a state that survived a relaunch would be a claim about a master
    /// nobody has checked, so every launch starts from silence.
    ssh_links: Mutex<HashMap<String, ssh_link::LinkReport>>,
    /// The CLIs driven without a screen — Codex `app-server`, Gemini CLI
    /// `--acp` — one child each (docs/design/agent-wire-sessions-20260915.md).
    wires: wire_runtime::WireRuntime,
    /// Which intention currently owns each target's connection. Every
    /// Connect and Disconnect bumps the number; a watcher captured an older
    /// one retires the moment it looks. This is how a background ladder can
    /// never overwrite what a person just asked for.
    ssh_link_epochs: Mutex<HashMap<String, u64>>,
    /// Remote server metadata shares the settings repository, while bearer
    /// tokens stay behind this native-credential boundary.
    remote_servers: remote_servers::RemoteServerService,
}

/// The checkout the file surfaces are showing, and the repository that owns it.
///
/// One value behind one lock, because they are one fact. Held as two mutexes
/// they could be read a moment apart, and a command that paired a new root with
/// the previous repository's orchestrator would run git in a checkout that
/// repository has never heard of — `git -C <other repo's worktree>` either
/// fails or, worse, answers about the wrong tree. Every command that needs
/// both takes one snapshot.
///
/// `Clone` so a caller can let the lock go before it walks a tree or spawns
/// git: holding it across that would block every other file command for as
/// long as the walk takes.
#[derive(Clone)]
struct ActiveContext {
    root: PathBuf,
    /// `None` when the selected project is not a git repository — an ordinary
    /// state, not a failure: the worktree list is empty and the rest works.
    orchestrator: Option<Orchestrator>,
}

/// The two paths a per-project operation needs, derived from one active
/// context snapshot.
struct ActiveProjectContext {
    /// The checkout whose repository file and legacy settings may be read.
    checkout: PathBuf,
    /// The repository root that owns the durable settings key.
    repo_root: PathBuf,
}

/// Membership of a lock that a panicking thread may have poisoned.
///
/// The set holds plain session ids and is valid at every step, so continuing
/// with it beats a window whose lanes can never subscribe again.
fn subscribed(live: &Mutex<HashSet<String>>) -> MutexGuard<'_, HashSet<String>> {
    live.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ShellRuntime {
    fn config_root(&self) -> &Path {
        self.paths.active_root(app_paths::PathClass::Config)
    }

    fn cache_root(&self) -> &Path {
        self.paths.active_root(app_paths::PathClass::Cache)
    }

    /// The registry lock, surviving a poisoned mutex.
    ///
    /// A panic in one command must not freeze every lane forever after: the
    /// registry holds plain state that is valid at every step, so continuing
    /// with it is strictly better than a window that stops painting.
    fn ssh_links(&self) -> MutexGuard<'_, HashMap<String, ssh_link::LinkReport>> {
        self.ssh_links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ssh_link_epochs(&self) -> MutexGuard<'_, HashMap<String, u64>> {
        self.ssh_link_epochs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn registry(&self) -> MutexGuard<'_, LaneRegistry> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn channels(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn channel_owners(&self) -> MutexGuard<'_, HashMap<String, ZoChannelOwner>> {
        self.channel_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn terminals(&self) -> &TerminalRegistry {
        &self.terminals
    }

    fn resource_usage(&self) -> MutexGuard<'_, resource_usage::ResourceUsageState> {
        self.resource_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn float_seat(&self) -> MutexGuard<'_, Option<String>> {
        self.float_seat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ssh_hosts(&self) -> &ssh_hosts::SshHostService {
        &self.ssh_hosts
    }

    fn remote_servers(&self) -> &remote_servers::RemoteServerService {
        &self.remote_servers
    }

    fn watched_terms(&self) -> MutexGuard<'_, HashMap<String, HashSet<TermId>>> {
        self.watched_terms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn catalog_owners(&self) -> MutexGuard<'_, HashMap<String, (Orchestrator, Worktree)>> {
        self.catalog_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn previewed_terms(&self) -> MutexGuard<'_, HashMap<String, HashSet<TermId>>> {
        self.previewed_terms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take a freshly spawned shell into the window's keeping.
    ///
    /// The one door, because there are SEVEN spawn sites — the float, a tab, an
    /// agent launch, a resume, an automation run, a teammate pane, a restored
    /// session — and every one of them owes the new shell the same thing: the
    /// scrollback depth this person chose. A depth applied at six of seven
    /// sites is the bug that reads as "the setting works, except for agents".
    ///
    /// The grid's own default is what a shell gets when nothing was chosen, so
    /// this is a no-op on a machine with no settings file.
    fn hold_terminal(
        &self,
        settings: &settings::SettingsRepository,
        term: TermId,
        pty: impl Into<PtyHandle>,
    ) {
        let mut pty = pty.into();
        let prefs = load_settings_for_boot(settings).document.terminal_prefs;
        pty.terminal_mut()
            .grid_mut()
            .set_scrollback_cap(prefs.scrollback);
        self.terminals().insert(term, pty);
    }

    fn rings(&self) -> MutexGuard<'_, zerocode_core::notify::RingLedger> {
        self.rings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn notify_book(&self) -> MutexGuard<'_, notify_call::NotifyBook> {
        self.notify_book
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lane_bell(&self) -> MutexGuard<'_, zerocode_core::notify::LaneBell> {
        self.lane_bell
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn commit_failure(&self) -> MutexGuard<'_, Option<CommitFailure>> {
        self.commit_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn deliveries(&self) -> MutexGuard<'_, HashMap<TermId, PromptDelivery>> {
        self.deliveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pending_nudges(&self) -> MutexGuard<'_, restart_nudge_runtime::PendingNudges> {
        self.pending_nudges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn zo_worker_deliveries(&self) -> MutexGuard<'_, HashMap<TermId, PromptDelivery>> {
        self.zo_worker_deliveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn delivery_waiters(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>> {
        self.delivery_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn worker_readiness(&self) -> MutexGuard<'_, HashMap<TermId, WorkerReadinessSlot>> {
        self.worker_readiness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn worker_prompt_submits(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, std::sync::mpsc::SyncSender<()>>> {
        self.worker_prompt_submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn completed_worker_cleanups(&self) -> MutexGuard<'_, HashMap<TermId, CompletedWorkerCleanup>> {
        self.completed_worker_cleanups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn isolated_worker_terms(&self) -> MutexGuard<'_, HashMap<TermId, IsolatedWorkerCheckout>> {
        self.isolated_worker_terms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn prompt_queue(&self) -> MutexGuard<'_, HashMap<TermId, VecDeque<QueuedPrompt>>> {
        self.prompt_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn agent_terms(&self) -> MutexGuard<'_, HashMap<TermId, &'static str>> {
        self.agent_terms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn launch_tokens(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.launch_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn untitled_markdowns(&self) -> MutexGuard<'_, std::collections::HashSet<String>> {
        self.untitled_markdowns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn browser_panes(&self) -> MutexGuard<'_, std::collections::HashSet<String>> {
        self.browser_panes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn browser_urls(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.browser_urls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn browser_records(&self) -> MutexGuard<'_, HashMap<String, BrowserPaneRecord>> {
        self.browser_records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn browser_reserved(&self) -> MutexGuard<'_, std::collections::HashSet<String>> {
        self.browser_reserved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pane_sessions(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ProviderSession>> {
        self.pane_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pane_states(&self) -> MutexGuard<'_, HashMap<TermId, PaneState>> {
        self.pane_states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn last_statuses(&self) -> MutexGuard<'_, HashMap<String, last_status::LastStatus>> {
        self.last_statuses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn last_status_seats(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.last_status_seats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn last_ring(&self) -> MutexGuard<'_, Option<LastRing>> {
        self.last_ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pane_parents(&self) -> MutexGuard<'_, HashMap<TermId, TermId>> {
        self.pane_parents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pane_helpers(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.pane_helpers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn unpublished_rosters(&self) -> MutexGuard<'_, std::collections::HashSet<TermId>> {
        self.unpublished_rosters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pane_cwds(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.pane_cwds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn workers(&self) -> MutexGuard<'_, HashMap<String, (TermId, &'static str)>> {
        self.workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn foreground_watch(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ForegroundWatch>> {
        self.foreground_watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn foreground_agents(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ForegroundWatch>> {
        self.foreground_agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn shell_panes(&self) -> MutexGuard<'_, std::collections::HashSet<TermId>> {
        self.shell_panes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn zo_adoptions(&self) -> MutexGuard<'_, HashMap<TermId, ZoPaneAdoption>> {
        self.zo_adoptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The helpers running inside each pane's agent, by pane.
    ///
    /// Kept here and not only forwarded, for the reason `pane_states` is: a
    /// board opened — or a window reloaded — an hour into a session was not
    /// listening when the helpers started, and five agents running invisibly
    /// is the exact situation these rows exist to end.
    fn subagents(&self) -> MutexGuard<'_, HashMap<TermId, Vec<hooks::SubagentRow>>> {
        self.subagents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The parked all-clears — a `Done` each roster is holding back.
    fn pending_done(&self) -> MutexGuard<'_, HashMap<TermId, (String, hooks::PaneHookReport)>> {
        self.pending_done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What each agent has been DOING, by card.
    ///
    /// Keyed by the board's own card name (`term:3`, `sub:3:a-1`) and not by
    /// [`TermId`], because a helper running inside an agent has no pane of its
    /// own and its tool calls are the ones nobody could see at all — a
    /// map keyed by shell could not tell the parent's work from its five
    /// workers'.
    ///
    /// Bounded twice over: [`ACTIVITY_RING`] entries per card, and a card
    /// disappears with the shell it hangs under (`forget_activities`). What
    /// makes the second one matter is that this map is keyed by a STRING —
    /// nothing about a key expires on its own, so the door that forgets a
    /// shell has to forget its helpers' keys too.
    fn activities(&self) -> MutexGuard<'_, HashMap<String, ActivityRing>> {
        self.activities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Everything one shell's card and its helpers' cards were holding.
    ///
    /// A sweep rather than a `remove`, because the helper keys are not derived
    /// from anything this door is given: a pane that closed with five workers
    /// running leaves five keys nothing else will ever name, and no stop event
    /// is coming for them — the process that would have sent it is the one
    /// that just died.
    fn forget_activities(&self, term: TermId) {
        let pane = hooks::activity_pane(term);
        let helpers = hooks::activity_subagent_prefix(term);
        self.activities()
            .retain(|card, _| card != &pane && !card.starts_with(&helpers));
    }

    fn team_envs(&self) -> MutexGuard<'_, HashMap<TermId, Vec<(String, String)>>> {
        self.team_envs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ask_sends(&self) -> MutexGuard<'_, HashMap<TermId, u64>> {
        self.ask_sends
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn interrupt_inference(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, zerocode_core::interrupt::InterruptInference>> {
        self.interrupt_inference
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn inference_sends(&self) -> MutexGuard<'_, HashMap<TermId, u64>> {
        self.inference_sends
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The active checkout and its repository, as one snapshot.
    ///
    /// The lock is released before this returns, so a caller is free to spend
    /// seconds in git with it.
    fn active(&self) -> ActiveContext {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Just the repository, for the callers that have no path to resolve.
    /// Anything needing both asks [`AppState::active`] once instead.
    fn orchestrator(&self) -> Option<Orchestrator> {
        self.active().orchestrator
    }

    /// Move the window's attention, both halves at once — and write it down,
    /// so the next boot wakes up here instead of in the launcher's `/`.
    fn set_active_context(&self, root: PathBuf, orchestrator: Option<Orchestrator>) {
        remember_last_workspace(self.config_root(), &root);
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            ActiveContext { root, orchestrator };
    }

    /// The next terminal id, consumed.
    fn take_term_id(&self) -> TermId {
        let mut next = self
            .next_term
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = *next;
        *next += 1;
        id
    }

    /// The directory the file surfaces are showing, as an owned path.
    ///
    /// Owned on purpose: the callers walk trees and spawn git, and holding
    /// this lock across that would block every other file command for the
    /// length of a filesystem walk.
    fn active_root(&self) -> PathBuf {
        self.active().root
    }

    /// The checkout and logical project that owns its preferences.
    ///
    /// A linked worktree is a checkout, not a second project. Both paths come
    /// from one lock acquisition so a workspace switch cannot pair one
    /// project's checkout with another project's durable settings key.
    fn active_project_context(&self) -> ActiveProjectContext {
        let here = self.active();
        let repo_root = here
            .orchestrator
            .as_ref()
            .map_or_else(|| here.root.clone(), |open| open.repo_root().to_path_buf());
        ActiveProjectContext {
            checkout: here.root,
            repo_root,
        }
    }
}

/// Access to shell-only lifecycle state attached to the shared state value.
///
/// The state crate deliberately exposes only the first extraction's three
/// capabilities. This trait keeps the rest of the composition root private to
/// this crate while allowing existing command code to use the same managed
/// `AppState` during the incremental migration.
trait ShellStateExt {
    fn shell_runtime(&self) -> &ShellRuntime;

    fn config_root(&self) -> &Path;
    fn cache_root(&self) -> &Path;
    fn ssh_links(&self) -> MutexGuard<'_, HashMap<String, ssh_link::LinkReport>>;
    fn ssh_link_epochs(&self) -> MutexGuard<'_, HashMap<String, u64>>;
    fn registry(&self) -> MutexGuard<'_, LaneRegistry>;
    fn terminals(&self) -> &TerminalRegistry;
    fn resource_usage(&self) -> MutexGuard<'_, resource_usage::ResourceUsageState>;
    fn float_seat(&self) -> MutexGuard<'_, Option<String>>;
    fn ssh_hosts(&self) -> &ssh_hosts::SshHostService;
    fn remote_servers(&self) -> &remote_servers::RemoteServerService;
    fn watched_terms(&self) -> MutexGuard<'_, HashMap<String, HashSet<TermId>>>;
    fn catalog_owners(&self) -> MutexGuard<'_, HashMap<String, (Orchestrator, Worktree)>>;
    fn previewed_terms(&self) -> MutexGuard<'_, HashMap<String, HashSet<TermId>>>;
    fn hold_terminal(&self, term: TermId, pty: impl Into<PtyHandle>);
    fn rings(&self) -> MutexGuard<'_, zerocode_core::notify::RingLedger>;
    fn notify_book(&self) -> MutexGuard<'_, notify_call::NotifyBook>;
    fn lane_bell(&self) -> MutexGuard<'_, zerocode_core::notify::LaneBell>;
    fn commit_failure(&self) -> MutexGuard<'_, Option<CommitFailure>>;
    fn deliveries(&self) -> MutexGuard<'_, HashMap<TermId, PromptDelivery>>;
    fn pending_nudges(&self) -> MutexGuard<'_, restart_nudge_runtime::PendingNudges>;
    fn zo_worker_deliveries(&self) -> MutexGuard<'_, HashMap<TermId, PromptDelivery>>;
    fn delivery_waiters(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>>;
    fn worker_readiness(&self) -> MutexGuard<'_, HashMap<TermId, WorkerReadinessSlot>>;
    fn worker_prompt_submits(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, std::sync::mpsc::SyncSender<()>>>;
    fn completed_worker_cleanups(&self) -> MutexGuard<'_, HashMap<TermId, CompletedWorkerCleanup>>;
    fn isolated_worker_terms(&self) -> MutexGuard<'_, HashMap<TermId, IsolatedWorkerCheckout>>;
    fn prompt_queue(&self) -> MutexGuard<'_, HashMap<TermId, VecDeque<QueuedPrompt>>>;
    fn agent_terms(&self) -> MutexGuard<'_, HashMap<TermId, &'static str>>;
    fn launch_tokens(&self) -> MutexGuard<'_, HashMap<TermId, String>>;
    fn untitled_markdowns(&self) -> MutexGuard<'_, std::collections::HashSet<String>>;
    fn browser_panes(&self) -> MutexGuard<'_, std::collections::HashSet<String>>;
    fn browser_urls(&self) -> MutexGuard<'_, HashMap<String, String>>;
    fn browser_born(&self) -> MutexGuard<'_, u64>;
    fn browser_records(&self) -> MutexGuard<'_, HashMap<String, BrowserPaneRecord>>;
    fn browser_reserved(&self) -> MutexGuard<'_, std::collections::HashSet<String>>;
    fn pane_sessions(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ProviderSession>>;
    fn pane_states(&self) -> MutexGuard<'_, HashMap<TermId, PaneState>>;
    fn last_statuses(&self) -> MutexGuard<'_, HashMap<String, last_status::LastStatus>>;
    fn last_status_seats(&self) -> MutexGuard<'_, HashMap<TermId, String>>;
    fn last_status_writes(&self) -> &std::sync::atomic::AtomicU64;
    fn last_status_written(&self) -> MutexGuard<'_, Option<Vec<u8>>>;
    fn last_ring(&self) -> MutexGuard<'_, Option<LastRing>>;
    fn pane_parents(&self) -> MutexGuard<'_, HashMap<TermId, TermId>>;
    fn pane_helpers(&self) -> MutexGuard<'_, HashMap<TermId, String>>;
    fn unpublished_rosters(&self) -> MutexGuard<'_, std::collections::HashSet<TermId>>;
    fn pane_cwds(&self) -> MutexGuard<'_, HashMap<TermId, String>>;
    fn workers(&self) -> MutexGuard<'_, HashMap<String, (TermId, &'static str)>>;
    fn foreground_watch(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ForegroundWatch>>;
    fn foreground_agents(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ForegroundWatch>>;
    fn shell_panes(&self) -> MutexGuard<'_, std::collections::HashSet<TermId>>;
    fn zo_adoptions(&self) -> MutexGuard<'_, HashMap<TermId, ZoPaneAdoption>>;
    fn subagents(&self) -> MutexGuard<'_, HashMap<TermId, Vec<hooks::SubagentRow>>>;
    fn pending_done(&self) -> MutexGuard<'_, HashMap<TermId, (String, hooks::PaneHookReport)>>;
    fn activities(&self) -> MutexGuard<'_, HashMap<String, ActivityRing>>;
    fn forget_activities(&self, term: TermId);
    fn team_envs(&self) -> MutexGuard<'_, HashMap<TermId, Vec<(String, String)>>>;
    fn ask_sends(&self) -> MutexGuard<'_, HashMap<TermId, u64>>;
    fn interrupt_inference(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, zerocode_core::interrupt::InterruptInference>>;
    fn inference_sends(&self) -> MutexGuard<'_, HashMap<TermId, u64>>;
    fn active(&self) -> ActiveContext;
    fn orchestrator(&self) -> Option<Orchestrator>;
    fn set_active_context(&self, root: PathBuf, orchestrator: Option<Orchestrator>);
    fn take_term_id(&self) -> TermId;
    fn active_root(&self) -> PathBuf;
    fn active_project_context(&self) -> ActiveProjectContext;

    fn primary_selection(&self) -> &primary_selection::PrimarySelection;
    fn supervisor(&self) -> Option<&LaneSupervisor>;
    fn supervisor_unavailable(&self) -> bool;
    fn project_root(&self) -> &Path;
    fn watched(&self) -> &Arc<file_watch::WatchSet>;
    fn subscriptions(&self) -> &Arc<Mutex<HashSet<String>>>;
    fn channels(&self) -> MutexGuard<'_, HashMap<String, String>>;
    fn channel_owners(&self) -> MutexGuard<'_, HashMap<String, ZoChannelOwner>>;
    fn cadence(&self) -> &Cadence;
    fn ssh_credentials(&self) -> &ssh_prompt::CredentialBroker;
    fn awake(&self) -> &awake::AwakeService;
    fn native_tray(&self) -> &native_tray::NativeTray;
    fn window_blur_active(&self) -> bool;
    fn set_legacy_authority(&self, lease: Option<LegacyAuthorityLease>);
}

impl ShellStateExt for AppState {
    fn shell_runtime(&self) -> &ShellRuntime {
        self.shell_extension::<ShellRuntime>()
            .expect("shell runtime must be installed before state is managed")
    }

    fn config_root(&self) -> &Path {
        self.shell_runtime().config_root()
    }

    fn cache_root(&self) -> &Path {
        self.shell_runtime().cache_root()
    }

    fn ssh_links(&self) -> MutexGuard<'_, HashMap<String, ssh_link::LinkReport>> {
        self.shell_runtime().ssh_links()
    }

    fn ssh_link_epochs(&self) -> MutexGuard<'_, HashMap<String, u64>> {
        self.shell_runtime().ssh_link_epochs()
    }

    fn registry(&self) -> MutexGuard<'_, LaneRegistry> {
        self.shell_runtime().registry()
    }

    fn terminals(&self) -> &TerminalRegistry {
        self.shell_runtime().terminals()
    }

    fn resource_usage(&self) -> MutexGuard<'_, resource_usage::ResourceUsageState> {
        self.shell_runtime().resource_usage()
    }

    fn float_seat(&self) -> MutexGuard<'_, Option<String>> {
        self.shell_runtime().float_seat()
    }

    fn ssh_hosts(&self) -> &ssh_hosts::SshHostService {
        self.shell_runtime().ssh_hosts()
    }

    fn remote_servers(&self) -> &remote_servers::RemoteServerService {
        self.shell_runtime().remote_servers()
    }

    fn watched_terms(&self) -> MutexGuard<'_, HashMap<String, HashSet<TermId>>> {
        self.shell_runtime().watched_terms()
    }

    fn catalog_owners(&self) -> MutexGuard<'_, HashMap<String, (Orchestrator, Worktree)>> {
        self.shell_runtime().catalog_owners()
    }

    fn previewed_terms(&self) -> MutexGuard<'_, HashMap<String, HashSet<TermId>>> {
        self.shell_runtime().previewed_terms()
    }

    fn hold_terminal(&self, term: TermId, pty: impl Into<PtyHandle>) {
        self.shell_runtime()
            .hold_terminal(self.settings(), term, pty);
    }

    fn rings(&self) -> MutexGuard<'_, zerocode_core::notify::RingLedger> {
        self.shell_runtime().rings()
    }

    fn notify_book(&self) -> MutexGuard<'_, notify_call::NotifyBook> {
        self.shell_runtime().notify_book()
    }

    fn lane_bell(&self) -> MutexGuard<'_, zerocode_core::notify::LaneBell> {
        self.shell_runtime().lane_bell()
    }

    fn commit_failure(&self) -> MutexGuard<'_, Option<CommitFailure>> {
        self.shell_runtime().commit_failure()
    }

    fn deliveries(&self) -> MutexGuard<'_, HashMap<TermId, PromptDelivery>> {
        self.shell_runtime().deliveries()
    }

    fn pending_nudges(&self) -> MutexGuard<'_, restart_nudge_runtime::PendingNudges> {
        self.shell_runtime().pending_nudges()
    }

    fn zo_worker_deliveries(&self) -> MutexGuard<'_, HashMap<TermId, PromptDelivery>> {
        self.shell_runtime().zo_worker_deliveries()
    }

    fn delivery_waiters(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>> {
        self.shell_runtime().delivery_waiters()
    }

    fn worker_readiness(&self) -> MutexGuard<'_, HashMap<TermId, WorkerReadinessSlot>> {
        self.shell_runtime().worker_readiness()
    }

    fn worker_prompt_submits(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, std::sync::mpsc::SyncSender<()>>> {
        self.shell_runtime().worker_prompt_submits()
    }

    fn completed_worker_cleanups(&self) -> MutexGuard<'_, HashMap<TermId, CompletedWorkerCleanup>> {
        self.shell_runtime().completed_worker_cleanups()
    }

    fn isolated_worker_terms(&self) -> MutexGuard<'_, HashMap<TermId, IsolatedWorkerCheckout>> {
        self.shell_runtime().isolated_worker_terms()
    }

    fn prompt_queue(&self) -> MutexGuard<'_, HashMap<TermId, VecDeque<QueuedPrompt>>> {
        self.shell_runtime().prompt_queue()
    }

    fn agent_terms(&self) -> MutexGuard<'_, HashMap<TermId, &'static str>> {
        self.shell_runtime().agent_terms()
    }

    fn launch_tokens(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.shell_runtime().launch_tokens()
    }

    fn untitled_markdowns(&self) -> MutexGuard<'_, std::collections::HashSet<String>> {
        self.shell_runtime().untitled_markdowns()
    }

    fn browser_panes(&self) -> MutexGuard<'_, std::collections::HashSet<String>> {
        self.shell_runtime().browser_panes()
    }

    fn browser_urls(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.shell_runtime().browser_urls()
    }

    fn browser_born(&self) -> MutexGuard<'_, u64> {
        self.shell_runtime()
            .browser_born
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn browser_records(&self) -> MutexGuard<'_, HashMap<String, BrowserPaneRecord>> {
        self.shell_runtime().browser_records()
    }

    fn browser_reserved(&self) -> MutexGuard<'_, std::collections::HashSet<String>> {
        self.shell_runtime().browser_reserved()
    }

    fn pane_sessions(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ProviderSession>> {
        self.shell_runtime().pane_sessions()
    }

    fn pane_states(&self) -> MutexGuard<'_, HashMap<TermId, PaneState>> {
        self.shell_runtime().pane_states()
    }

    fn last_statuses(&self) -> MutexGuard<'_, HashMap<String, last_status::LastStatus>> {
        self.shell_runtime().last_statuses()
    }

    fn last_status_seats(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.shell_runtime().last_status_seats()
    }

    fn last_status_writes(&self) -> &std::sync::atomic::AtomicU64 {
        &self.shell_runtime().last_status_writes
    }

    fn last_status_written(&self) -> MutexGuard<'_, Option<Vec<u8>>> {
        self.shell_runtime()
            .last_status_written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn last_ring(&self) -> MutexGuard<'_, Option<LastRing>> {
        self.shell_runtime().last_ring()
    }

    fn pane_parents(&self) -> MutexGuard<'_, HashMap<TermId, TermId>> {
        self.shell_runtime().pane_parents()
    }

    fn pane_helpers(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.shell_runtime().pane_helpers()
    }

    fn unpublished_rosters(&self) -> MutexGuard<'_, std::collections::HashSet<TermId>> {
        self.shell_runtime().unpublished_rosters()
    }

    fn pane_cwds(&self) -> MutexGuard<'_, HashMap<TermId, String>> {
        self.shell_runtime().pane_cwds()
    }

    fn workers(&self) -> MutexGuard<'_, HashMap<String, (TermId, &'static str)>> {
        self.shell_runtime().workers()
    }

    fn foreground_watch(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ForegroundWatch>> {
        self.shell_runtime().foreground_watch()
    }

    fn foreground_agents(&self) -> MutexGuard<'_, HashMap<TermId, zerocode_core::ForegroundWatch>> {
        self.shell_runtime().foreground_agents()
    }

    fn shell_panes(&self) -> MutexGuard<'_, std::collections::HashSet<TermId>> {
        self.shell_runtime().shell_panes()
    }

    fn zo_adoptions(&self) -> MutexGuard<'_, HashMap<TermId, ZoPaneAdoption>> {
        self.shell_runtime().zo_adoptions()
    }

    fn subagents(&self) -> MutexGuard<'_, HashMap<TermId, Vec<hooks::SubagentRow>>> {
        self.shell_runtime().subagents()
    }

    fn pending_done(&self) -> MutexGuard<'_, HashMap<TermId, (String, hooks::PaneHookReport)>> {
        self.shell_runtime().pending_done()
    }

    fn activities(&self) -> MutexGuard<'_, HashMap<String, ActivityRing>> {
        self.shell_runtime().activities()
    }

    fn forget_activities(&self, term: TermId) {
        self.shell_runtime().forget_activities(term);
    }

    fn team_envs(&self) -> MutexGuard<'_, HashMap<TermId, Vec<(String, String)>>> {
        self.shell_runtime().team_envs()
    }

    fn ask_sends(&self) -> MutexGuard<'_, HashMap<TermId, u64>> {
        self.shell_runtime().ask_sends()
    }

    fn interrupt_inference(
        &self,
    ) -> MutexGuard<'_, HashMap<TermId, zerocode_core::interrupt::InterruptInference>> {
        self.shell_runtime().interrupt_inference()
    }

    fn inference_sends(&self) -> MutexGuard<'_, HashMap<TermId, u64>> {
        self.shell_runtime().inference_sends()
    }

    fn active(&self) -> ActiveContext {
        self.shell_runtime().active()
    }

    fn orchestrator(&self) -> Option<Orchestrator> {
        self.shell_runtime().orchestrator()
    }

    fn set_active_context(&self, root: PathBuf, orchestrator: Option<Orchestrator>) {
        self.shell_runtime().set_active_context(root, orchestrator);
    }

    fn take_term_id(&self) -> TermId {
        self.shell_runtime().take_term_id()
    }

    fn active_root(&self) -> PathBuf {
        self.shell_runtime().active_root()
    }

    fn active_project_context(&self) -> ActiveProjectContext {
        self.shell_runtime().active_project_context()
    }

    fn primary_selection(&self) -> &primary_selection::PrimarySelection {
        &self.shell_runtime().primary_selection
    }

    fn supervisor(&self) -> Option<&LaneSupervisor> {
        self.shell_runtime().supervisor.as_ref()
    }

    fn supervisor_unavailable(&self) -> bool {
        self.shell_runtime().supervisor_unavailable
    }

    fn project_root(&self) -> &Path {
        &self.shell_runtime().project_root
    }

    fn watched(&self) -> &Arc<file_watch::WatchSet> {
        &self.shell_runtime().watched
    }

    fn subscriptions(&self) -> &Arc<Mutex<HashSet<String>>> {
        &self.shell_runtime().subscriptions
    }

    fn channels(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.shell_runtime().channels()
    }

    fn channel_owners(&self) -> MutexGuard<'_, HashMap<String, ZoChannelOwner>> {
        self.shell_runtime().channel_owners()
    }

    fn cadence(&self) -> &Cadence {
        &self.shell_runtime().cadence
    }

    fn ssh_credentials(&self) -> &ssh_prompt::CredentialBroker {
        &self.shell_runtime().ssh_credentials
    }

    fn awake(&self) -> &awake::AwakeService {
        &self.shell_runtime().awake
    }

    fn native_tray(&self) -> &native_tray::NativeTray {
        &self.shell_runtime().native_tray
    }

    fn window_blur_active(&self) -> bool {
        self.shell_runtime().window_blur_active
    }

    fn set_legacy_authority(&self, lease: Option<LegacyAuthorityLease>) {
        *self
            .shell_runtime()
            ._legacy_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = lease;
    }
}

/// Ask the platform for its blurred-window material, once, on the way up.
///
/// Orca's own only ever asks Windows (`backgroundMaterial: "acrylic"`), so its
/// macOS users have a switch that does nothing. Each platform is asked for its
/// own here: macOS's `HudWindow` is the NSVisualEffectView material that
/// acrylic is the Windows answer to, and Windows gets acrylic itself.
///
/// A refusal is not an error worth stopping for — an old Windows build has no
/// acrylic, a Linux compositor has no material at all — and the window simply
/// stays opaque, which is what it looked like before the switch existed.
fn apply_window_material(app: &AppHandle) {
    use tauri::window::{Color, Effect, EffectState, EffectsBuilder};

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let effects = EffectsBuilder::new()
        .effects([Effect::HudWindow, Effect::Acrylic])
        // The material follows the window rather than greying out when the
        // window loses focus: this is the app's surface, not a sheet over it.
        .state(EffectState::Active)
        // Windows' acrylic takes a tint; the macOS materials ignore it.
        .color(Color(0, 0, 0, 0))
        .build();
    if let Err(error) = window.set_effects(effects) {
        eprintln!("zerocode-shell: the window material was refused: {error}");
    }
}

/// Assemble state only after Tauri can resolve its configured application
/// directories. No command is reachable until `setup` has managed this value.
fn build_app_state(paths: app_paths::AppPaths, root: PathBuf) -> AppState {
    // Measured before this function writes anything. In compatibility mode the
    // active config root is the legacy profile; after the all-artifact gate it
    // becomes Tauri's app config directory.
    let profile_existed = state_profile_existed(paths.active_root(app_paths::PathClass::Config));
    let settings_root = paths
        .active_root(app_paths::PathClass::Config)
        .to_path_buf();
    // Before anything reads it: a Finder launch's `/` becomes the workspace
    // the window actually was in. See `boot_workspace_root`.
    let root = boot_workspace_root(root, &settings_root);
    let local_data_root = paths
        .active_root(app_paths::PathClass::LocalData)
        .to_path_buf();
    let (supervisor, supervisor_unavailable) = match discover_supervisor(&root, &local_data_root) {
        Ok(supervisor) => (supervisor, false),
        Err(_) => {
            eprintln!(
                "zerocode-shell: lanes are disabled because persistent session state is unavailable"
            );
            (None, true)
        }
    };
    let jira = jira_store::JiraStore::new(settings_root.clone());
    let linear = zerocode_shell_state::linear_store::LinearStore::new(settings_root.clone());
    let ssh_hosts = ssh_hosts::SshHostService::new();
    let remote_servers = remote_servers::RemoteServerService::new();
    let settings = Arc::new(settings::SettingsRepository::new(settings_root.clone()));
    if let Err(error) = migrate_legacy_project_settings(&settings) {
        eprintln!("zerocode-shell: could not migrate project settings: {error}");
    }
    if let Err(error) = migrate_legacy_agent_launches(&settings) {
        eprintln!("zerocode-shell: could not migrate agent launch settings: {error}");
    }
    let boot_settings = load_settings_for_boot(&settings);
    let wants_blur = boot_settings.window_blur;
    // The power hold starts under the person's stored choice, before any
    // agent can report — an `on` that waited for the first hook event would
    // let the machine sleep through exactly the launch it was set for.
    let awake = awake::AwakeService::default();
    awake.set_mode(boot_settings.document.computer_awake_mode);
    // The operator's last-step policy starts under the stored choice too.
    computer_use::confirm::set_policy(boot_settings.document.computer_confirm_policy());
    // Opened first: git's own spelling of the repository root is what every
    // worktree comparison is made against, so starting from it means the main
    // checkout matches its own entry instead of missing by a symlink.
    note_project(&settings, &settings_root, &root);
    // Persist the first-run answer before the webview can ask for it.
    settle_onboarding(&settings_root, profile_existed);
    settle_tour_eligibility(&settings_root);
    let orchestrator = Orchestrator::open(&root).ok();
    let active_root = orchestrator
        .as_ref()
        .map_or_else(|| root.clone(), |open| open.repo_root().to_path_buf());
    // The same fact the workspace switch records: where this window is. A
    // boot that only ever READ the memory would leave first-run and
    // pre-upgrade profiles empty until somebody switched by hand.
    remember_last_workspace(&settings_root, &active_root);

    // 지난 세션의 판들 — Orca hydrateLastStatusFromDisk의 관용 그대로,
    // 없는 파일은 첫 실행이고 상한 밖과 손상은 경고 한 줄로 강등된다.
    let hydrated = match last_status::load(
        &paths
            .active_root(app_paths::PathClass::LocalData)
            .join(app_paths::artifact_file::LAST_STATUS),
        epoch_ms_now(),
    ) {
        Ok(one) => one,
        Err(warning) => {
            eprintln!("zerocode-shell: {warning}");
            last_status::Hydrated::default()
        }
    };
    if hydrated.pruned > 0 {
        // 버린 행의 수는 이름으로 남긴다. 조용히 버리면 「보드가 어제를
        // 반만 기억한다」가 원인 없는 증상이 된다.
        eprintln!(
            "zerocode-shell: last-status에서 행 {}개를 버렸습니다(모양이 낯설거나 상한 밖)",
            hydrated.pruned
        );
    }
    // 아무것도 안 버렸으면 **디스크의 바이트가 곧 우리가 들고 있는 것**이므로,
    // 그것으로 동일성 스킵을 예열해 첫 소식이 같은 내용을 다시 적지 않게 한다.
    // 하나라도 버렸으면 예열하지 않는다 — 그 자체가 「고친 파일을 반드시 한 번
    // 적는다」가 된다(Orca도 같은 갈림: `server.ts:3046-3057`).
    let last_status_primed =
        (hydrated.pruned == 0 && !hydrated.raw.is_empty()).then_some(hydrated.raw);
    let last_statuses = hydrated.entries;

    let runtime = ShellRuntime {
        paths,
        primary_selection: primary_selection::PrimarySelection::default(),
        _legacy_authority: Mutex::new(None),
        subagents: Mutex::default(),
        pending_done: Mutex::default(),
        prompt_queue: Mutex::default(),
        activities: Mutex::default(),
        supervisor,
        supervisor_unavailable,
        registry: Mutex::new(LaneRegistry::new()),
        terminals: TerminalRegistry::default(),
        tree_ops: Mutex::new(file_tree_ops::History::default()),
        tree_hooks: Mutex::new(file_tree_hooks::Pending::default()),
        resource_usage: Mutex::new(resource_usage::ResourceUsageState::default()),
        deliveries: Mutex::new(HashMap::new()),
        pending_nudges: Mutex::new(restart_nudge_runtime::PendingNudges::default()),
        zo_worker_deliveries: Mutex::new(HashMap::new()),
        delivery_waiters: Mutex::new(HashMap::new()),
        worker_readiness: Mutex::new(HashMap::new()),
        worker_prompt_submits: Mutex::new(HashMap::new()),
        completed_worker_cleanups: Mutex::new(HashMap::new()),
        isolated_worker_terms: Mutex::new(HashMap::new()),
        agent_terms: Mutex::new(HashMap::new()),
        launch_tokens: Mutex::new(HashMap::new()),
        untitled_markdowns: Mutex::new(std::collections::HashSet::new()),
        browser_panes: Mutex::new(std::collections::HashSet::new()),
        browser_urls: Mutex::new(HashMap::new()),
        browser_born: Mutex::new(0),
        browser_records: Mutex::new(HashMap::new()),
        browser_reserved: Mutex::new(std::collections::HashSet::new()),
        pane_sessions: Mutex::new(HashMap::new()),
        pane_states: Mutex::new(HashMap::new()),
        last_statuses: Mutex::new(last_statuses),
        // 자리 지도는 늘 빈 손으로 시작한다: 하이드레이트로 올라온 행들은
        // 지난 프로세스가 앉힌 것이라 이 프로세스의 어떤 번호도 가리키지
        // 않는다. 그것을 물려받는 것이 곧 지난 세션의 소식을 덮을 권한을
        // 물려받는 일이다.
        last_status_seats: Mutex::new(HashMap::new()),
        last_status_written: Mutex::new(last_status_primed),
        last_status_writes: std::sync::atomic::AtomicU64::new(0),
        last_ring: Mutex::new(None),
        foreground_watch: Mutex::new(HashMap::new()),
        foreground_agents: Mutex::new(HashMap::new()),
        shell_panes: Mutex::new(std::collections::HashSet::new()),
        zo_adoptions: Mutex::new(HashMap::new()),
        zo_integrations: Mutex::default(),
        workers: Mutex::new(HashMap::new()),
        pane_parents: Mutex::new(HashMap::new()),
        pane_helpers: Mutex::new(HashMap::new()),
        unpublished_rosters: Mutex::default(),
        pane_cwds: Mutex::default(),
        team_envs: Mutex::new(HashMap::new()),
        ask_sends: Mutex::new(HashMap::new()),
        // 제스처의 기억도 늘 빈 손으로 시작한다 — 지난 프로세스가 반쯤
        // 누른 이중 Escape를 이 프로세스가 이어받을 이유가 없다.
        interrupt_inference: Mutex::new(HashMap::new()),
        inference_sends: Mutex::new(HashMap::new()),
        float_seat: Mutex::new(None),
        next_term: Mutex::new(FLOAT_TERM + 1),
        watched_terms: Mutex::new(HashMap::new()),
        catalog_owners: Mutex::new(HashMap::new()),
        previewed_terms: Mutex::new(HashMap::new()),
        subscriptions: Arc::new(Mutex::new(HashSet::new())),
        channels: Arc::new(Mutex::new(HashMap::new())),
        channel_owners: Mutex::new(HashMap::new()),
        watched: Arc::new(file_watch::WatchSet::default()),
        active: Mutex::new(ActiveContext {
            root: active_root,
            orchestrator,
        }),
        project_root: root,
        rings: Mutex::new(zerocode_core::notify::RingLedger::default()),
        notify_book: Mutex::new(notify_call::NotifyBook::default()),
        lane_bell: Mutex::new(zerocode_core::notify::LaneBell::default()),
        native_tray: native_tray::NativeTray::default(),
        awake,
        commit_failure: Mutex::new(None),
        cadence: Cadence::default(),
        ssh_credentials: ssh_prompt::CredentialBroker::default(),
        window_blur_active: wants_blur,
        ssh_hosts,
        ssh_links: Mutex::new(HashMap::new()),
        wires: wire_runtime::WireRuntime::default(),
        ssh_link_epochs: Mutex::new(HashMap::new()),
        remote_servers,
    };
    let state = AppState::new(local_data_root, settings, jira, linear);
    state.install_shell_extension(runtime);
    state
}

const SINGLE_INSTANCE_BYPASS_ENV: &str = "ZEROCODE_BYPASS_SINGLE_INSTANCE_LOCK";

/// Whether this launch was told to skip the one-window-per-profile lock.
///
/// `"1"` and nothing else — Orca's own reading of its bypass switch
/// (`ORCA_BYPASS_SINGLE_INSTANCE_LOCK`, single-instance-lock.ts:63); an env
/// var that also counts "0" as yes is a switch nobody can turn off. Orca
/// offers the bypass only on packaged macOS because its dev runs lock in a
/// profile of their own (`orca-dev` userData); this app is one profile
/// everywhere, so the escape hatch answers everywhere — and carries the same
/// warning: two windows on one profile fight over the hook endpoint file.
fn single_instance_lock_bypassed(said: Option<&str>) -> bool {
    said == Some("1")
}

fn main() -> ExitCode {
    crumbs::register_main_thread();
    // Before anything else, and before any state is touched: what is this?
    //
    // A release used to be checkable at two layers — the code and the branch
    // — and unanswerable at the third, the binary somebody is actually
    // running. The UI ships compressed, so probing the file for a function
    // name finds nothing whether the UI is current or a year stale; the
    // bundle version is static. This is the door that makes "which build is
    // on this machine" a question with an answer, and `-dirty` is what turns
    // "we only build from a committed snapshot" from a rule people follow
    // into a fact the artifact reports.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|asked| asked == "--version" || asked == "-V")
    {
        println!("zerocode-shell {}", env!("CARGO_PKG_VERSION"));
        println!("commit {}", env!("ZEROCODE_COMMIT"));
        println!("ui {}", env!("ZEROCODE_UI_DIGEST"));
        return ExitCode::SUCCESS;
    }
    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
    if let Err(error) = chromium_browser::install_application() {
        eprintln!("zerocode-shell: Chromium을 시작할 수 없습니다: {error}");
        return ExitCode::FAILURE;
    }
    // First, before anything can ask: start reading the user's shell PATH. A
    // Finder-launched app inherits launchd's minimal PATH, on which no agent
    // exists — see `shell_path`. Warmed in the background so a slow rc file
    // delays nothing.
    shell_path::warm();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = project_root(&cwd);

    // ONE window per profile, decided before anything else is. Every boot
    // writes a fresh port and token into the hook endpoint file
    // (`hooks::start`), so a second launch points every agent's hook script
    // at a process that quits when its window closes — the first window sits
    // "connected" to a bridge nothing reports to. Orca holds a
    // single-instance lock over exactly this (its two discovery files,
    // single-instance-lock.ts:22-46): the duplicate asks the running window
    // to come forward and exits. The plugin is registered FIRST so the
    // duplicate leaves in plugin setup, before any state is touched. Two
    // knowing divergences: the duplicate exits 0 (the plugin's contract;
    // Orca's exit code 3 answers a systemd serve unit this app does not
    // have), and there is no dev-mode skip (Orca's dev locks in a separate
    // `orca-dev` profile; ours shares the one profile, where skipping would
    // keep the exact clobber this lock ends).
    let mut builder = tauri::Builder::default().manage(sftp_runtime::SftpService::default());
    if single_instance_lock_bypassed(std::env::var(SINGLE_INSTANCE_BYPASS_ENV).ok().as_deref()) {
        eprintln!(
            "zerocode-shell: {SINGLE_INSTANCE_BYPASS_ENV}=1 — the single-instance lock is off \
             for this launch; do not run two windows on one profile."
        );
    } else {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // The duplicate has already left; this runs in the window that
            // stays, and the person who double-clicked asked for a window —
            // this is the one they own (Orca activates its desktop window
            // the same way, `shouldActivateDesktopForSecondInstance`; no
            // serve mode here to carve out). `show` first: a window parked
            // in the tray is hidden, not minimized, and focusing a hidden
            // window does nothing anyone can see.
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }));
    }

    let result = builder
        // No dialog plugin: the native file panel is the `zerocode-pick`
        // helper's, in its own process (t-2982) — opened in this one it held
        // the main thread through AppKit's remote view service.
        // The one door out of the window: an agent that stopped while the
        // person was in another app has no other way to say so.
        .plugin(tauri_plugin_notification::init())
        // Plain text only. The capability grants no HTML, image, or file
        // clipboard commands, and guest browser webviews are not named there.
        .plugin(tauri_plugin_clipboard_manager::init())
        // Versioned updates (t-3191): the feed check and the signed
        // download. Registered, not granted — `capabilities/default.json`
        // names neither plugin, so the window's only roads are the five
        // `update_*` commands and `relaunch_window` (design §2.3, §3).
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            api_router_keys_kept,
            api_router_presets,
            api_router_providers,
            test_router_connection,
            save_api_router,
            remove_api_router,
            typesafe_settings,
            save_typesafe_key,
            remove_typesafe_key,
            set_jev_mode,
            set_route_classifier,
            set_jev_model,
            check_typesafe_key,
            jev_summary,
            judge_worker_room,
            note_worker_room_change,
            crash_bundle,
            crash_open_log,
            set_crash_watchdog,
            zo_integration_runtime::zo_integration_details,
            boot_report,
            settings_snapshot,
            ssh_hosts,
            save_ssh_host,
            remove_ssh_host,
            probe_ssh_host,
            test_ssh_host,
            ssh_submit_credential,
            open_ssh_terminal,
            ssh_targets,
            ssh_save_target,
            ssh_remove_target,
            ssh_import_config,
            ssh_probe_target,
            ssh_link_states,
            ssh_link_connect,
            ssh_link_disconnect,
            ssh_open_remote_term,
            remote_workspaces,
            sftp_sources,
            sftp_bookmark,
            sftp_connect,
            sftp_disconnect,
            sftp_connections,
            sftp_list,
            sftp_search,
            sftp_mkdir,
            sftp_rename,
            sftp_read_text,
            sftp_write_text,
            sftp_enqueue,
            sftp_jobs,
            sftp_job_control,

            probe_remote_workspace,
            save_remote_workspace,
            remove_remote_workspace,
            test_remote_workspace,
            open_remote_workspace_terminal,
            remote_servers,
            save_remote_server,
            remove_remote_server,
            test_remote_server,
            open_remote_server_session,
            developer_permission_statuses,
            request_developer_permission,
            open_developer_permission_settings,
            test_local_network_permission,
            open_lane,
            focus_lane,
            close_lane,
            text_input,
            key_input,
            lane_scroll,
            lane_fold,
            mouse_input,
            paste_input,
            answer_ask,
            answer_approval,
            resize_lane,
            respond_permission,
            gate_lane,
            list_dir,
            browse_dir,
            browse_places,
            path_kinds,
            session_info,
            scm_status,
            stage_path,
            unstage_path,
            stage_paths,
            unstage_paths,
            discard_paths,
            delete_untracked,
            commit_staged,
            upstream_status,
            scm_push,
            scm_pull,
            scm_fetch,
            commit_failure_card,
            conflict_card,
            abort_conflict_operation,
            submodule_status,
            generate_commit_message,
            git_history,
            commit_files,
            commit_file_diff,
            open_commit_remote,
            file_diff,
            source_control_compare_context,
            worktree_committed_diff,
            set_worktree_compare_base,
            image_diff,
            list_skills,
            skills_list,
            skills_rescan,
            skill_detail,
            skill_install_plan,
            skill_bundle_parse,
            skill_reveal,
            reveal_skill,
            fs_create,
            fs_move,
            fs_undo,
            fs_redo,
            fs_rename,
            fs_trash,
            fs_duplicate,
            fs_reveal,
            paths_exist,
            fs_open_default,
            orchestration_report,
            orchestration_runtime_state,
            computer_use_skill_report,
            install_bundled_skill,
            second_brain_status,
            second_brain_setup,
            second_brain_link,
            second_brain_open,
            second_brain_graph,
            second_brain_page,
            second_brain_paths,
            second_brain_export_html,
            second_brain_relate,
            second_brain_seat_recalls,
            get_second_brain_scenes,
            set_second_brain_scenes,
            set_second_brain_explore,
            set_second_brain_weekly_review,
            supply_chain_graph,
            supply_chain_report,
            computer_use_permission_status,
            open_computer_use_permission,
            reset_computer_use_permissions,
            computer_use_capabilities,
            vault_sessions,
            resume_vault_session,
            reveal_vault_session,
            open_download,
            show_download,
            browser_snapshot,
            set_clipboard_image,
            list_worktrees,
            worktree_stamp,
            create_worktree,
            worktree_prefs,
            worktree_last_agent,
            worktree_evidence,
            restore_orchestration_workers,
            save_worktree_prefs,
            validate_branch_name,
            list_branches,
            work_item_seed,
            github_work_items,
            github_assignable_users,
            github_preset_query,
            github_web_urls,
            github_review_states,
            github_work_item_detail,
            github_set_reviewers,
            github_pr_file_diff,
            github_merge_pr,
            github_comment_work_item,
            github_set_work_item_open,
            resolve_pr_base,
            resolve_mr_base,
            worktree_loss,
            merge_and_remove_worktree,
            remove_worktree,
            workspace_cleanup_scan,
            workspace_space_scan,
            workspace_space_cancel,
            workspace_space_git,
            set_active_worktree,
            search_files,
            search_text,
            choose_project,
            choose_paths,
            recall_folder_panel,
            cancel_folder_panel,
            clone_target_name,
            default_project_parent,
            clone_repository,
            create_project,
            open_project,
            project_kind,
            remove_project,
            reorder_projects,
            project_catalog,
            set_repo_mark,
            set_external_worktree_visibility,
            dismiss_external_worktree_prompt,
            import_external_worktrees,
            suppress_external_worktree_inbox,
            read_text_file,
            file_version,
            write_text_file,
            watch_files,
            read_image_file,
            render_mermaid,
            open_terminal,
            patch_floating_workspace,
            floating_workspace_seat,
            open_term_tab,
            open_mirror_term,
            mirror_ready,
            pane_log,
            subagent_log,
            terminal_command,
            set_terminal_command,
            terminal_command_argv,
            log_window_error,
            process_memory,
            resource_snapshot,
            listening_ports,
            stop_workspace_port,
            send_prompt,
            list_agents,
            set_default_agent,
            agent_terms,
            launch_agent_tab,
            list_claude_sessions,
            claude_usage,
            claude_usage_stats,
            codex_usage_stats,
            opencode_usage_stats,
            claude_token_usage,
            codex_token_usage,
            codex_usage,
            kimi_usage,
            grok_usage,
            opencode_usage,
            set_opencode_cookie,
            set_opencode_workspace,
            antigravity_usage,
            google_account,
            google_login_start,
            google_login_finish,
            cancel_google_login,
            google_logout,
            pr_checks,
            pr_check_details,
            open_workspace_in_application,
            open_path_in_application,
            open_url,
            note_webview_error,
            create_untitled_markdown,
            release_untitled_markdown,
            delete_untitled_markdown,
            open_browser_pane,
            browser_profiles,
            create_browser_profile,
            delete_browser_profile,
            set_browser_default_profile,
            cookie_sources,
            import_browser_cookies,
            import_cookie_file,
            mobile_emulators,
            open_mobile_emulator,
            start_emulator_stream,
            stop_emulator_stream,
            acknowledge_emulator_payload,
            set_emulator_stream_paused,
            set_emulator_stream_engaged,
            set_emulator_stream_viewport,
            choose_emulator_app,
            shutdown_mobile_emulator,
            ios_tap,
            ios_touch,
            ios_swipe,
            ios_text,
            ios_button,
            ios_rotate,
            ios_accessibility_tree,
            ios_multi_touch,
            ios_install_app,
            ios_launch_app,
            ios_set_permission,
            ios_logs,
            android_emulators,
            start_android_stream,
            start_emulator_video,
            android_tap,
            android_swipe,
            android_text,
            android_button,
            android_rotate,
            android_accessibility_tree,
            android_install_app,
            android_launch_app,
            android_set_permission,
            android_logs,
            shutdown_android_emulator,
            browser_navigate,
            browser_history,
            browser_reload,
            browser_stop,
            browser_place,
            browser_devtools,
            browser_find,
            browser_find_clear,
            browser_zoom,
            browser_eval,
            browser_read,
            browser_click,
            browser_type,
            browser_wait,
            browser_console,
            browser_network,
            browser_scroll,
            browser_diagnose,
            browser_grab_arm,
            browser_menu_take,
            browser_grab_take,
            browser_grab_disarm,
            focus_main,
            close_browser_pane,
            agent_icon,
            agent_launch_plans,
            save_agent_launch,
            save_agent_launch_env,
            reset_agent_launch,
            set_agent_permission_mode,
            project_scripts,
            default_tabs,
            mark_default_tabs_applied,
            set_project_script_policy,
            set_project_script_setting,
            repo_trust_standing,
            record_repo_trust,
            archive_trust_standing,
            record_archive_trust,
            save_onboarding_step,
            close_onboarding,
            reopen_onboarding,
            mark_onboarding,
            set_guide_dismissed,
            mark_first_run_seen,
            setup_guide,
            tour_decision,
            tip_verdict,
            github_status,
            github_select_account,
            github_test_connection,
            github_disconnect,
            github_login_intent,
            gitlab_status,
            gitlab_work_items,
            gitlab_todos,
            gitlab_item_detail,
            gitlab_comment_item,
            gitlab_set_item_open,
            gitlab_pipeline_jobs,
            gitlab_retry_job,
            gitlab_job_trace,
            gitlab_merge_mr,
            gitlab_update_mr,
            gitlab_mr_review,
            gitlab_set_mr_reviewers,
            gitlab_project_members,
            gitlab_inline_comment,
            notification_probe,
            set_notification_preference,
            set_browser_home_page,
            set_browser_search_engine,
            patch_browser_link_routing,
            patch_browser_user_agents,
            set_browser_restore_tabs,
            set_browser_default_zoom,
            set_browser_open_tabs,
            set_browser_visits,
            claude_accounts,
            add_claude_account,
            select_claude_account,
            use_system_claude_login,
            relogin_claude_account,
            verify_claude_accounts,
            resolve_claude_account_identity,
            remove_claude_account,
            generate_pull_request,
            generate_branch_name,
            pull_request_seed,
            hosted_review_eligibility,
            create_pull_request,
            codex_account_list,
            add_codex_account,
            relogin_codex_account,
            relogin_codex_login,
            logout_codex_login,
            cli_login_list,
            cli_login_start,
            cli_login_logout,
            cli_login_wait,
            verify_codex_accounts,
            select_codex_account,
            remove_codex_account,
            hooks_report,
            set_hooks_enabled,
            install_hooks,
            pane_sessions,
            pane_agents,
            ledger_agents,
            coordinator_handover_status,
            coordinator_seat_runs,
            claim_coordinator_seat,
            set_coordinator_handover,
            pane_subagents,
            pane_activities,
            board_columns,
            board_snapshot,
            orchestration_accuracy,
            agent_models,
            slash_commands,
            wire_answer,
            wire_interrupt,
            wire_log,
            wire_models,
            wire_send,
            wire_set_mode,
            wire_set_model,
            wire_start,
            wire_stop,
            set_dock_badge,
            open_board_popout,
            ack_board_agent,
            reveal_board_agent,
            set_watched_terms,
            term_pull,
            set_previewed_terms,
            term_snapshot,
            worker_screen,
            continuation_source,
            launch_recipes,
            save_launch_recipe,
            launch_plan_for_action,
            resume_session,
            list_quick_commands,
            save_quick_command,
            delete_quick_command,
            list_diff_notes,
            save_diff_note,
            delete_diff_note,
            clear_diff_notes,
            clear_delivered_diff_notes,
            list_automations,
            save_automation,
            delete_automation,
            enable_automation,
            run_automation,
            list_automation_runs,
            list_run_evidence,
            reveal_run_evidence,
            flow_list,
            flow_console::flow_evidence,
            flow_console::flow_execute,
            flow_set,
            artifacts_list,
            artifact_search,
            artifact_preview,
            artifact_counts,
            artifact_open,
            artifact_reveal,
            artifact_copy_path,
            artifact_delete,
            artifact_register,
            artifact_versions,
            artifact_thumbnail,
            artifact_import_transcripts,
            set_artifacts_retention_days, set_vault_session_limit,
            automation_born_worktrees,
            set_hide_automation_workspaces,
            set_hide_default_branch_workspaces,
            set_hide_detached_head_workspaces,
            set_sidebar_view,
            set_hide_sleeping_workspaces,
            set_keep_default_branch_awake,
            set_hide_agent_scratch_workspaces,
            set_worktree_card_property,
            patch_workspace_board_status,
            patch_workspace_board_items,
            set_workspace_board_column_width,
            set_agent_teams_mode,
            agent_teams_mode,
            set_confirm_close_pinned,
            set_computer_confirm,
            computer_confirm_answer,
            computer_stop,
            computer_resume,
            computer_guard_status,
            set_skip_close_terminal_with_running_process_confirm,
            set_ctrl_tab_order_mode,
            patch_workspace_creation_prefs,
            patch_open_in_applications,
            set_skip_delete_worktree_confirm,
            set_skip_delete_automation_confirm,
            set_diff_side_by_side,
            set_conversation_focus_view,
            patch_editing_prefs,
            read_primary_selection,
            write_primary_selection,
            list_system_fonts,
            keyboard_input_source::terminal_keyboard_layout,
            set_locale,
            set_theme,
            apply_ui_zoom,
            set_ui_zoom,
            set_app_font_family,
            set_show_titlebar_app_name,
            set_show_menu_bar_icon,
            set_minimize_to_tray_on_close,
            set_compact_worktree_cards,
            set_agent_activity_display,
            set_show_git_ignored_files,
            set_source_control_group_order,
            set_source_control_compare_base,
            set_refresh_local_base_ref_on_worktree_create,
            patch_left_sidebar_appearance,
            set_status_bar_item,
            set_usage_percentage_display,
            set_status_bar_usage_mode,
            stats_summary,
            set_usage_analytics_enabled,
            set_source_control_view_mode,
            scm_tree_rows,
            set_terminal_opacity,
            set_window_blur,
            relaunch_window,
            build_stamp,
            release_status,
            patch_update_prefs,
            update_check,
            update_download,
            update_install,
            update_history,
            set_hidden_shortcuts,
            set_shortcut_visibility,
            set_keybinding,
            set_hidden_task_sources,
            set_task_source_visibility,
            set_default_task_source,
            jira_status,
            work_item_links,
            jira_sync_set_paused,
            jira_sync_retry,
            link_jira_worktree,
            unlink_worktree_item,
            set_jira_link_sync,
            jira_connect,
            jira_select_site,
            jira_test_connection,
            jira_disconnect,
            jira_issues,
            jira_projects,
            jira_issue_detail,
            jira_issue_comments,
            jira_attachment_preview,
            jira_agent_context,
            jira_comment_issue,
            jira_create_issue,
            jira_issue_options,
            jira_update_issue,
            jira_search_issues,
            jira_worktree_started,
            linear_status,
            linear_connect,
            linear_disconnect,
            linear_issues,
            linear_search_issues,
            linear_issue_detail,
            linear_issue_comments,
            linear_comment_issue,
            linear_create_issue,
            linear_issue_options,
            linear_agent_context,
            set_panel_widths,
            set_panel_width,
            pane_layouts,
            save_pane_layouts,
            stage_layouts,
            save_stage_layouts,
            term_has_running_process,
            terminal_sessions,
            end_terminal_session,
            end_all_terminal_sessions,
            close_term,
            term_text,
            term_key,
            term_mouse,
            term_paste,
            save_pasted_image,
            save_clipboard_image,
            clipboard_has_image,
            term_scroll,
            term_fold,
            term_search,
            term_lines,
            lane_lines,
            term_view_to_line,
            term_focus,
            terminal_prefs,
            terminal_windows_status,
            preview_warp_terminal_themes,
            preview_ghostty_import,
            apply_ghostty_import,
            set_terminal_prefs,
            patch_terminal_prefs,
            set_setup_script_launch_mode,
            set_terminal_shortcut_policy,
            set_computer_awake_mode,
            computer_awake_status,
            last_statuses,
            term_resize,

        ])
        .setup(move |app| {
            computer_use::initialize(app.path().resource_dir().ok());
            run_evidence::install_window(app.handle().clone());
            let resolved = app_paths::resolve(app).map_err(std::io::Error::other)?;
            let (paths, legacy_authority) =
                match state_migration::migrate_if_needed(&resolved) {
                    Ok(paths) => (paths, None),
                    Err(migration_error) => {
                        let legacy = resolved.legacy_state().ok_or_else(|| {
                            std::io::Error::other(format!(
                                "platform path migration was not activated: {migration_error}"
                            ))
                        })?;
                        let lease = LegacyAuthorityLease::acquire(legacy).map_err(|error| {
                            std::io::Error::other(format!(
                                "legacy path authority could not be held after migration failed: {error}"
                            ))
                        })?;
                        // The exclusive migration may have completed while
                        // this process waited for the shared lease. Resolve
                        // again under the lock instead of trusting the stale
                        // pre-migration snapshot.
                        let guarded =
                            app_paths::resolve(app).map_err(std::io::Error::other)?;
                        if guarded.platform_paths_active() {
                            drop(lease);
                            let activated = state_migration::migrate_if_needed(&guarded).map_err(
                                |error| {
                                    std::io::Error::other(format!(
                                        "platform path migration was not activated: {error}"
                                    ))
                                },
                            )?;
                            (activated, None)
                        } else if guarded.migration_pending() {
                            return Err(std::io::Error::other(format!(
                                "platform path migration is pending and neither authority is writable: {migration_error}"
                            ))
                            .into());
                        } else {
                            eprintln!(
                                "zerocode-shell: platform path migration was deferred; retaining the legacy authority for this window: {migration_error}"
                            );
                            (guarded, Some(lease))
                        }
                    }
                };
            paths.ensure_active_roots()?;
            install_window_panic_hook(
                paths
                    .active_root(app_paths::PathClass::LocalData)
                    .to_path_buf(),
            );
            let crash_root = paths.active_root(app_paths::PathClass::LocalData);
            if let Err(error) = crash::begin(crash_root) {
                note_window_event(crash_root, &format!("crash boot: {error}"));
            }
            let state = build_app_state(paths, root.clone());
            state.set_legacy_authority(legacy_authority);
            let wants_blur = state.window_blur_active();
            if !app.manage(state) {
                return Err(std::io::Error::other("application state was already managed").into());
            }
            // Where the update stands for this process (t-3191): the phase the
            // pane paints, the announced version, the verified archive and
            // the staged bundle `relaunch_window` swaps in.
            if !app.manage(cmd::update::UpdateState::new()) {
                return Err(std::io::Error::other("update state was already managed").into());
            }
            let handle = app.handle().clone();
            let managed = app.state::<AppState>();
            #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
            {
                let chromium_root = managed.config_root().join("browser-chromium");
                chromium_browser::initialize(&handle, &chromium_root)
                    .map_err(std::io::Error::other)?;
            }
            let stale_codex_routes = codex_queue::reap_stale();
            if stale_codex_routes > 0 {
                note_window_event(
                    managed.local_data_root(),
                    &format!("reaped {stale_codex_routes} stale Codex app-server sidecars"),
                );
            }
            // The stats head's counters, read from disk once and kept.
            stats_events_store::open(managed.config_root());
            managed.native_tray().install_handler(&handle);
            let boot_settings = load_settings_for_boot(managed.settings()).document;
            hang_watchdog::configure(crash::Limits::overlay(&boot_settings.crash));
            // The readiness probe reads the account stores under this root
            // and no other; until it is named, every door answers unknown.
            readiness_runtime::configure_root(managed.config_root());
            readiness_runtime::configure(readiness_runtime::limits_of(&boot_settings.readiness));
            crumbs::record("boot", format_args!("settings"));
            if let Err(error) = managed.native_tray().sync_for_boot(
                &handle,
                boot_settings.show_menu_bar_icon,
                boot_settings.minimize_to_tray_on_close,
            ) {
                eprintln!("zerocode-shell: the native tray could not be started: {error}");
            }
            if wants_blur {
                apply_window_material(&handle);
            }
            // 팝아웃의 수명은 메인 창에 매여 있다. 창이 열리기 전에 걸어
            // 두는 것이 아니라 여기서 한 번 거는 것으로 충분하다 — 팝아웃은
            // 나중에 태어나고, 이 청취자는 메인 창이 죽는 순간에만 본다.
            // 자리부터. 사람이 창을 보기 전에 옮기는 편이 덜 튄다.
            restore_main_window_bounds(&handle);
            close_popout_with_main(&handle);
            // The hook bridge, before anything can launch an agent. `start`
            // binds loopback and writes the endpoint file; `None` means this
            // machine gets no hooks and every other surface still works.
            let local_data_root = app.state::<AppState>().local_data_root().to_path_buf();
            // Where the second brain is, before anything can open a pane. Every
            // later settings write republishes it (`mutate_settings`); this one
            // read is what carries the last run's answer across a restart.
            let saved_vault = load_settings_resilient(app.state::<AppState>().settings())
                .document
                .second_brain_vault;
            hooks::publish_second_brain_vault(&saved_vault);
            // And the agents' global instructions say the same: the block
            // setup wrote, written again for a vault saved before this road
            // existed or an agent installed since — idempotent, off the boot
            // path.
            cmd::second_brain::relink_saved_vault_at_boot(saved_vault);
            // The runs the last window left, before its road is open. An
            // orchestration is a plan and a plan outlives the screen that
            // showed it — what does not outlive it is the terminals, and every
            // one of those is closed here rather than being carried forward as
            // a pane that is not there.
            crumbs::record("boot", format_args!("ledger"));
            let swept = orchestration::open(&local_data_root, now_epoch_ms());
            if swept.ended > 0 {
                note_window_event(
                    &local_data_root,
                    &format!(
                        "orchestration: {} terminals ended with the last window",
                        swept.ended
                    ),
                );
            }
            // Said separately because it is the opposite fact: these attempts
            // did NOT end, and their tasks are still dispatched.
            if swept.sleeping > 0 {
                note_window_event(
                    &local_data_root,
                    &format!(
                        "orchestration: {} terminals are being seated again",
                        swept.sleeping
                    ),
                );
            }
            crumbs::record("boot", format_args!("hooks"));
            if let Some((events, teams, browser, computer, federation)) =
                hooks::start(&local_data_root)
            {
                let listener = handle.clone();
                tauri::async_runtime::spawn(async move { hook_loop(listener, events).await });
                // The same bridge's other road: an orchestrating agent asking
                // for a pane. Its own task because a leader is BLOCKED on the
                // answer — a split queued behind an hour of hook events is a
                // teammate that appears an hour late.
                let cutter = handle.clone();
                tauri::async_runtime::spawn(async move { teams_loop(cutter, teams).await });
                // And the third: `zerocode-browser` from any pane (1-g4). Its
                // own task for the same reason as the teams' — the agent is
                // blocked on the answer.
                let steering = handle.clone();
                tauri::async_runtime::spawn(async move { browser_loop(steering, browser).await });
                // The fourth road owns the persistent native accessibility
                // provider. Browser and emulator work route to their in-app
                // surfaces; ordinary desktop apps arrive here.
                let computer_app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    computer_loop(computer_app, computer).await
                });
                // The fifth road: another MACHINE's window borrowing panes
                // here. Each call blocks on ledger law and possibly a spawn,
                // so it runs on the blocking pool — and the home side's
                // relay heartbeat lives in the orchestration module itself.
                let farm = handle.clone();
                tauri::async_runtime::spawn(async move {
                    federation_loop(farm, federation).await
                });
            }
            // And the socket a walk's first question will ride (t-5535). The
            // agents' roads are open above, so a walk can now be asked for;
            // the handshake it would otherwise pay is opened here instead, on
            // the helper's own thread — nothing on the boot path waits for it.
            systemone::warm_for_walks();
            // The Jira sync outbox the last window enqueued but never finished:
            // a crash between enqueue and the network leaves Pending rows behind.
            // Resume them now — held by default, so a drain posts nothing until a
            // person turns syncing back on, and a Pending row was never sent, so
            // finishing it can never duplicate a Jira write.
            let sync_drain = handle.clone();
            tauri::async_runtime::spawn(async move {
                let pending = {
                    let state = sync_drain.state::<AppState>();
                    work_item_store::pending_event_ids(state.settings()).unwrap_or_default()
                };
                for event_id in pending {
                    schedule_jira_sync(&sync_drain, event_id);
                }
            });
            // Reconciling the agents' own config files rewrites up to ten files
            // and walks PATH, so it happens off the startup path entirely: a
            // window that waits on ten settings-file merges before it paints has
            // made a reporting channel into a launch delay.
            let config_root = app.state::<AppState>().config_root().to_path_buf();
            // The artifact store, opened before anything can hand it a report:
            // the catalog is read here (one file), the retention overlay is the
            // saved setting, and the evidence folders the automation ledger
            // knows are registered as scan sources. The first walk rides the
            // reconcile thread below — no thread of its own — and the file
            // watcher's artifact lane says when to look again.
            let retention_days = load_settings_resilient(app.state::<AppState>().settings())
                .document
                .artifacts_retention_days;
            artifact_runtime::install(Arc::new(artifact_runtime::Store::open(
                &local_data_root,
                zerocode_core::artifact::Limits::default(),
            )), handle.clone());
            artifact_runtime::note_retention_days(retention_days);
            automation_runtime::adopt_stored_evidence(&local_data_root);
            // 판의 첫 1초 (t-5645): the device somebody opened last is woken
            // now, before a pane asks for it, and the watch that shuts an
            // unwatched device down starts. Both spawn their own threads —
            // nothing here waits on a simulator.
            emulator::on_window_boot(&handle);
            let scanning = handle.clone();
            // zo rides along (t-3191, design §2.4): the bundle's `Resources/bin/zo`
            // against `~/.local/bin/zo`, judged and swapped by rename on this
            // same one-shot boot thread — no thread of its own — and reported
            // to the update state for the settings pane's one line.
            let zo_resources = app.path().resource_dir().ok();
            std::thread::spawn(move || {
                if hooks::hooks_enabled(&config_root) {
                    hooks::reconcile(&config_root, &local_data_root, &installed_agent_slugs());
                } else {
                    // Off means the entries go, and "off" was possibly chosen in
                    // a previous run — so this pass is what actually enforces it.
                    hooks::reconcile(&config_root, &local_data_root, &[]);
                }
                if let (Some(home), Ok(app_version)) = (
                    dirs::home_dir(),
                    semver::Version::parse(env!("CARGO_PKG_VERSION")),
                ) {
                    let report =
                        zo_companion::run_at_boot(zo_resources.as_deref(), &home, &app_version);
                    scanning
                        .state::<cmd::update::UpdateState>()
                        .note_zo(report);
                }
                artifact_runtime::rescan_and_tell(&scanning, now_epoch_ms());
                // The transcripts' artifacts and pages (t-3233 §2): one
                // bounded, incremental pass on this same thread.
                artifact_runtime::backfill_transcripts(&scanning);
            });
            // The open files, watched for the writes this window did not
            // make. Its own thread rather than a job on the pump: the pump
            // ticks for terminal screens, and a stat that hangs on a dead
            // network mount must stall the file marks, never the shells.
            let watched = app.state::<AppState>().watched().clone();
            let files = handle.clone();
            std::thread::spawn(move || {
                watched.run(file_watch::POLL, |events| {
                    // The artifact lane is the store's trigger, not the
                    // window's news: it re-walks its folders here, on this
                    // thread, and the window hears `artifacts:changed` only
                    // when the catalog moved. The tabs lane keeps its shape.
                    let (mine, rest): (Vec<_>, Vec<_>) = events
                        .into_iter()
                        .partition(|change| change.lane == artifact_runtime::WATCH_LANE);
                    // The second brain's lane is the window's news (t-2931):
                    // the graph view re-reads on it while it is showing,
                    // through its own floor. Nothing is scanned here.
                    let (vault, tabs): (Vec<_>, Vec<_>) = rest
                        .into_iter()
                        .partition(|change| change.lane == cmd::second_brain::WATCH_LANE);
                    if !mine.is_empty() {
                        artifact_runtime::rescan_and_tell(&files, now_epoch_ms());
                    }
                    if !vault.is_empty() {
                        let _ = files.emit(
                            cmd::second_brain::CHANGED_EVENT,
                            file_watch::FsChanged { events: vault },
                        );
                    }
                    if !tabs.is_empty() {
                        let _ = files.emit("fs:changed", file_watch::FsChanged { events: tabs });
                    }
                });
            });
            std::thread::spawn(move || pump_loop(&handle));
            // Sleep's far edge (P0-17 잔여): a fixed nap, and the wall
            // clock's overshoot judged by `resume_watch::slept_for`. The
            // window hears Orca's own channel name and re-checks its SSH
            // links; the husk keeps Orca's breadcrumb.
            let resume_app = app.handle().clone();
            std::thread::spawn(move || {
                let mut watchdog = hang_watchdog::Monitor::new();
                let cadence = resume_watch::POLL;
                loop {
                    let before = std::time::SystemTime::now();
                    std::thread::sleep(cadence);
                    let Ok(wall_delta) = before.elapsed() else {
                        // The clock walked backwards — ntp, not sleep.
                        continue;
                    };
                    let Some(slept) = resume_watch::slept_for(resume_watch::POLL, wall_delta)
                    else {
                        watchdog.tick(&resume_app);
                        continue;
                    };
                    watchdog.resumed();
                    note_window_event(
                        resume_app.state::<AppState>().local_data_root(),
                        &format!("system slept {}s", slept.as_secs()),
                    );
                    let _ = resume_app.emit_to(
                        MAIN_WINDOW_LABEL,
                        "system:resumed",
                        SystemResumed {
                            slept_ms: u64::try_from(slept.as_millis()).unwrap_or(u64::MAX),
                        },
                    );
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!());

    let app = match result {
        Ok(app) => app,
        Err(error) => {
            // A webview that will not start is worth a sentence, not a panic
            // backtrace: the cause is almost always the environment (no display, a
            // missing system webview), which a stack trace says nothing about.
            eprintln!("zerocode-shell: the window could not start: {error}");
            return ExitCode::FAILURE;
        }
    };
    app.run(|handle, event| {
        let _scope = crumbs::MainScope::enter(hang_watchdog::event_name(&event));
        // The one moment every shell is still alive and nothing more will be
        // typed into any of them — what the screens hold NOW is what the
        // next session should reopen showing.
        if let tauri::RunEvent::ExitRequested { code, .. } = &event {
            let reason = code.map_or_else(
                || "exit requested: user interaction or operating system".to_string(),
                |code| format!("exit requested: application code {code}"),
            );
            note_window_event(handle.state::<AppState>().local_data_root(), &reason);
        }
        match event {
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
                if chromium_browser::defer_exit(handle, code.unwrap_or(0)) {
                    api.prevent_exit();
                    return;
                }
                // The ledger hears the goodbye before any pane goes (t-3058):
                // seated workers sleep instead of being settled by their own
                // panes' exits on the way out.
                orchestration::window_exiting(now_epoch_ms());
                handle.state::<AppState>().native_tray().begin_exit();
                emulator::shutdown_all(handle);
                codex_queue::shutdown_all();
                capture_scrollback_at_exit(handle);
                // 원장의 마지막 플러시 — 디바운스가 아직 자고 있어도 여기서
                // 적힌다(Orca quit의 flushStatusPersistSync와 같은 자리).
                write_last_statuses_now(handle);
            }
            tauri::RunEvent::Exit => {
                #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
                chromium_browser::shutdown();
                // Again, idempotently: a restart from the main thread skips
                // `ExitRequested` (tauri's own note on `restart`), and the
                // log of every restart today shows exactly that.
                orchestration::window_exiting(now_epoch_ms());
                if let Err(error) = crash::clean_exit(handle.state::<AppState>().local_data_root())
                {
                    note_window_event(
                        handle.state::<AppState>().local_data_root(),
                        &format!("crash clean-exit: {error}"),
                    );
                }
                note_window_event(
                    handle.state::<AppState>().local_data_root(),
                    "event loop exited",
                );
            }
            // 알림을 누른 손이 앱으로 돌아오는 문 (P0-14) — macOS에서 알림
            // 클릭도 dock 클릭도 Reopen으로 온다. 방금( RING_CLICK_WINDOW_MS
            // 안에) 벨이 울렸을 때만 그 주소로 안내한다: 그 밖의 Reopen은
            // 벨과 무관한 복귀이고, 안내는 소비되면 비워진다.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                let state = handle.state::<AppState>();
                let invited = state
                    .last_ring()
                    .take_if(|ring| epoch_ms_now().saturating_sub(ring.at) <= RING_CLICK_WINDOW_MS);
                if let Some(ring) = invited {
                    let _ = handle.emit_to(MAIN_WINDOW_LABEL, "notify:activate", ring);
                }
            }
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == MAIN_WINDOW_LABEL
                && handle
                    .state::<AppState>()
                    .native_tray()
                    .hide_main_on_close(handle) =>
            {
                api.prevent_close();
            }
            tauri::RunEvent::WindowEvent { label, event, .. }
                if label == MAIN_WINDOW_LABEL
                    && matches!(event, tauri::WindowEvent::Focused(true)) =>
            {
                handle
                    .state::<AppState>()
                    .native_tray()
                    .clear_activity(handle);
            }
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } => {
                note_window_event(
                    handle.state::<AppState>().local_data_root(),
                    &format!("window destroyed: {label}"),
                );
            }
            _ => {}
        }
    });
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    include!("main_unit_tests.rs");
}
