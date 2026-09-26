//! Window Tauri commands, grouped by domain.
//!
//! The `generate_handler!` registration list stays in `main.rs`; this module
//! only re-exports bare command names for that one catalog.

pub(crate) mod agent_launch;
pub(crate) mod api_routers;
pub(crate) mod appearance;
pub(crate) mod artifacts;
pub(crate) mod board;
pub(crate) mod console;
pub(crate) use console::{agent_models, slash_commands};
pub(crate) mod wire;
pub(crate) use wire::{
    HAND_OVER_EXIT_POLL, HAND_OVER_EXIT_WAIT, program_left, wait_process_group_gone, wire_answer,
    wire_image, wire_interrupt, wire_log, wire_models, wire_send, wire_set_mode, wire_set_model,
    wire_start, wire_stop,
};
pub(crate) mod browser;
pub(crate) mod fs;
pub(crate) mod github_pr;
pub(crate) mod integration_prefs;
pub(crate) mod onboarding;
pub(crate) mod project;
pub(crate) mod remote;
pub(crate) mod sftp;
pub(crate) use sftp::{
    sftp_bookmark, sftp_connect, sftp_connections, sftp_disconnect, sftp_enqueue, sftp_job_control,
    sftp_jobs, sftp_list, sftp_mkdir, sftp_read_text, sftp_rename, sftp_search, sftp_sources,
    sftp_write_text,
};
pub(crate) mod repo_policy;
pub(crate) mod review;
pub(crate) mod scm;
pub(crate) mod second_brain;
pub(crate) mod session;
pub(crate) mod settings;
pub(crate) mod skills;
pub(crate) mod supply_chain;
pub(crate) mod system;
pub(crate) mod terminal;
pub(crate) mod update;
pub(crate) mod usage;
pub(crate) mod workspace;
pub(crate) mod worktree;

pub(crate) use remote::{
    developer_permission_statuses, open_developer_permission_settings, open_remote_server_session,
    open_remote_workspace_terminal, open_ssh_terminal, probe_remote_workspace, probe_ssh_host,
    remote_servers, remote_workspaces, remove_remote_server, remove_remote_workspace,
    remove_ssh_host, request_developer_permission, save_remote_server, save_remote_workspace,
    save_ssh_host, ssh_hosts, ssh_import_config, ssh_link_connect, ssh_link_disconnect,
    ssh_link_states, ssh_open_remote_term, ssh_probe_target, ssh_remove_target, ssh_save_target,
    ssh_submit_credential, ssh_targets, test_local_network_permission, test_remote_server,
    test_remote_workspace, test_ssh_host,
};

pub(crate) use browser::{
    browser_click, browser_console, browser_devtools, browser_diagnose, browser_eval, browser_find,
    browser_find_clear, browser_grab_arm, browser_grab_disarm, browser_grab_take, browser_history,
    browser_menu_take, browser_navigate, browser_network, browser_place, browser_profiles,
    browser_read, browser_reload, browser_scroll, browser_stop, browser_type, browser_wait,
    browser_zoom, close_browser_pane, cookie_sources, create_browser_profile,
    delete_browser_profile, focus_main, import_browser_cookies, import_cookie_file,
    note_webview_error, open_browser_pane, set_browser_default_profile,
};

pub(crate) use fs::{
    browse_dir, browse_places, browser_snapshot, computer_live_reflex_check,
    computer_use_capabilities, computer_use_permission_status, computer_use_tcc_row_action,
    file_version, fs_create, fs_duplicate, fs_move, fs_open_default, fs_redo, fs_rename, fs_reveal,
    fs_trash, fs_undo, image_diff, list_dir, open_computer_use_permission, open_download,
    orchestration_runtime_state, path_kinds, paths_exist, read_image_file, read_text_file,
    render_mermaid, reset_computer_use_permissions, resume_vault_session, reveal_vault_session,
    set_clipboard_image, show_download, vault_sessions, watch_files, write_text_file,
};

pub(crate) use usage::{
    add_claude_account, add_codex_account, antigravity_usage, cancel_google_login,
    claude_account_usage, claude_accounts, claude_autoswitch_apply, claude_token_usage,
    claude_usage, claude_usage_stats, cli_login_list, cli_login_logout, cli_login_start,
    cli_login_wait, cli_login_witness, cli_login_witness_drop, codex_account_list,
    codex_token_usage, codex_usage, codex_usage_stats, google_account, google_login_finish,
    google_login_start, google_logout, grok_usage, kimi_usage, logout_codex_login, opencode_usage,
    opencode_usage_stats, relogin_claude_account, relogin_codex_account, relogin_codex_login,
    remove_claude_account, remove_codex_account, resolve_claude_account_identity,
    select_claude_account, select_codex_account, set_opencode_cookie, set_opencode_workspace,
    stats_summary, use_system_claude_login, verify_claude_accounts, verify_codex_accounts,
};

pub(crate) use appearance::{
    apply_ghostty_import, apply_ui_zoom, busy_census, computer_awake_status, leave_cancel,
    leave_now, leave_when_idle, listening_ports, log_window_error, patch_left_sidebar_appearance,
    patch_terminal_prefs, preview_ghostty_import, preview_warp_terminal_themes, process_memory,
    relaunch_window, resource_snapshot, set_agent_activity_display, set_app_font_family,
    set_compact_worktree_cards, set_computer_awake_mode, set_locale, set_minimize_to_tray_on_close,
    set_refresh_local_base_ref_on_worktree_create, set_setup_script_launch_mode,
    set_show_git_ignored_files, set_show_menu_bar_icon, set_show_titlebar_app_name,
    set_source_control_compare_base, set_source_control_group_order, set_status_bar_item,
    set_terminal_opacity, set_terminal_prefs, set_terminal_shortcut_policy, set_theme, set_ui_zoom,
    set_window_blur, stop_workspace_port, terminal_command_argv, terminal_prefs,
    terminal_windows_status,
};

pub(crate) use onboarding::{
    cancel_folder_panel, choose_paths, choose_project, clone_repository, clone_target_name,
    close_onboarding, create_project, default_project_parent, mark_first_run_seen, mark_onboarding,
    recall_folder_panel, reopen_onboarding, save_onboarding_step, set_guide_dismissed, setup_guide,
    tip_verdict, tour_decision,
};

pub(crate) use agent_launch::{
    agent_launch_plans, delete_quick_command, list_quick_commands, reset_agent_launch,
    save_agent_launch, save_agent_launch_env, save_quick_command, set_agent_permission_mode,
};

pub(crate) use board::{
    ack_board_agent, agent_icon, board_columns, board_desk, board_snapshot, claim_coordinator_seat,
    clipboard_has_image, continuation_source, coordinator_handover_status, coordinator_seat_runs,
    create_untitled_markdown, delete_untitled_markdown, desk_ack, desk_checkouts, desk_reply,
    hooks_report, install_hooks, ledger_agents, machine_load, open_board_popout,
    orchestration_accuracy, pane_activities, pane_agents, pane_sessions, pane_subagents,
    release_untitled_markdown, resume_session, reveal_board_agent, save_clipboard_image,
    save_pasted_image, set_coordinator_handover, set_dock_badge, set_hooks_enabled,
    set_previewed_terms, set_watched_terms, term_pull, term_snapshot, worker_screen,
};

pub(crate) use review::{
    automation_born_worktrees, clear_delivered_diff_notes, clear_diff_notes, delete_automation,
    delete_diff_note, enable_automation, flow_list, flow_set, github_review_states,
    list_automation_runs, list_automations, list_diff_notes, list_run_evidence,
    open_path_in_application, open_url, open_workspace_in_application, pr_check_details, pr_checks,
    reveal_run_evidence, run_automation, save_automation, save_diff_note,
};

pub(crate) use api_routers::{
    api_router_keys_kept, api_router_presets, api_router_providers, remove_api_router,
    save_api_router, test_router_connection,
};

pub(crate) mod typesafe;
pub(crate) use typesafe::{
    check_typesafe_key, jev_day, jev_summary, remove_typesafe_key, save_typesafe_key,
    set_jev_enabled, set_jev_model, set_route_classifier, typesafe_settings,
};

pub(crate) mod type_value;
pub(crate) use type_value::{remove_type_value_key, save_type_value_key, type_value_keys};

pub(crate) mod worker_room;
pub(crate) use worker_room::{judge_worker_room, note_worker_room_change, note_worker_room_seen};

pub(crate) use github_pr::{github_pr_file_diff, github_set_reviewers};

pub(crate) use repo_policy::{
    archive_trust_standing, default_tabs, dismiss_external_worktree_prompt,
    import_external_worktrees, mark_default_tabs_applied, project_scripts, record_archive_trust,
    record_repo_trust, repo_trust_standing, set_external_worktree_visibility,
    set_project_script_policy, set_project_script_setting, set_repo_mark,
    suppress_external_worktree_inbox,
};

pub(crate) use integration_prefs::{
    github_disconnect, github_login_intent, github_select_account, github_status,
    github_test_connection, gitlab_status, notification_probe, patch_browser_link_routing,
    patch_browser_user_agents, set_browser_default_zoom, set_browser_home_page,
    set_browser_open_tabs, set_browser_restore_tabs, set_browser_search_engine, set_browser_visits,
    set_notification_preference,
};

pub(crate) use workspace::{
    workspace_cleanup_scan, workspace_space_cancel, workspace_space_git, workspace_space_scan,
};

pub(crate) use project::{
    gate_lane, open_lane, open_project, project_catalog, project_kind, remove_project,
    reorder_projects, respond_permission, search_files, search_text,
};

pub(crate) use terminal::{
    agent_terms, answer_approval, answer_ask, close_lane, close_term, end_all_terminal_sessions,
    end_terminal_session, focus_lane, key_input, lane_fold, lane_lines, lane_scroll,
    launch_agent_tab, mirror_ready, mouse_input, open_mirror_term, open_term_tab, open_terminal,
    pane_image, pane_log, paste_input, resize_lane, send_prompt, subagent_log, term_focus,
    term_fold, term_has_running_process, term_key, term_lines, term_mouse, term_paste, term_resize,
    term_scroll, term_search, term_text, term_view_to_line, terminal_sessions, text_input,
};

pub(crate) use scm::{
    abort_conflict_operation, commit_failure_card, commit_file_diff, commit_files, commit_staged,
    conflict_card, create_pull_request, delete_untracked, discard_paths, file_diff,
    generate_branch_name, generate_commit_message, generate_pull_request, git_history,
    hosted_review_eligibility, open_commit_remote, pull_request_seed, scm_fetch, scm_pull,
    scm_push, scm_status, set_worktree_compare_base, source_control_compare_context, stage_path,
    stage_paths, submodule_status, unstage_path, unstage_paths, upstream_status,
    worktree_committed_diff,
};

pub(crate) use system::{build_stamp, release_status};

pub(crate) use update::{
    patch_update_prefs, update_check, update_download, update_history, update_install,
};

pub(crate) use worktree::{
    create_worktree, github_assignable_users, github_comment_work_item, github_merge_pr,
    github_preset_query, github_set_work_item_open, github_web_urls, github_work_item_detail,
    github_work_items, gitlab_comment_item, gitlab_inline_comment, gitlab_item_detail,
    gitlab_job_trace, gitlab_merge_mr, gitlab_mr_review, gitlab_pipeline_jobs,
    gitlab_project_members, gitlab_retry_job, gitlab_set_item_open, gitlab_set_mr_reviewers,
    gitlab_todos, gitlab_update_mr, gitlab_work_items, list_branches, list_worktrees,
    merge_and_remove_worktree, remove_worktree, resolve_mr_base, resolve_pr_base,
    save_worktree_prefs, set_active_worktree, validate_branch_name, work_item_seed,
    worktree_evidence, worktree_last_agent, worktree_loss, worktree_prefs, worktree_stamp,
};

pub(crate) use settings::{
    agent_teams_mode, claude_autoswitch_mode, computer_confirm_answer, computer_guard_status,
    computer_resume, computer_stop, floating_workspace_seat, list_system_fonts, pane_layouts,
    patch_editing_prefs, patch_floating_workspace, patch_open_in_applications,
    patch_workspace_board_items, patch_workspace_board_status, patch_workspace_creation_prefs,
    read_primary_selection, save_pane_layouts, save_stage_layouts, scm_tree_rows,
    set_agent_teams_mode, set_artifacts_retention_days, set_claude_autoswitch_mode,
    set_computer_confirm, set_computer_live_reflex, set_confirm_close_pinned,
    set_conversation_focus_view, set_ctrl_tab_order_mode, set_default_task_source,
    set_diff_side_by_side, set_hidden_shortcuts, set_hidden_task_sources,
    set_hide_agent_scratch_workspaces, set_hide_automation_workspaces,
    set_hide_default_branch_workspaces, set_hide_detached_head_workspaces,
    set_hide_sleeping_workspaces, set_keep_default_branch_awake, set_keybinding, set_panel_width,
    set_panel_widths, set_shortcut_visibility, set_sidebar_view,
    set_skip_close_terminal_with_running_process_confirm, set_skip_delete_automation_confirm,
    set_skip_delete_worktree_confirm, set_source_control_view_mode, set_status_bar_usage_mode,
    set_task_source_visibility, set_terminal_command, set_usage_analytics_enabled,
    set_usage_percentage_display, set_vault_session_limit, set_workspace_board_column_width,
    set_worktree_card_property, stage_layouts, terminal_command, write_primary_selection,
};

pub(crate) use session::{
    boot_report, launch_plan_for_action, launch_recipes, list_agents, list_claude_sessions,
    save_launch_recipe, session_info, set_default_agent,
};

pub(crate) use second_brain::{
    get_second_brain_scenes, second_brain_export_html, second_brain_graph, second_brain_link,
    second_brain_open, second_brain_page, second_brain_paths, second_brain_relate,
    second_brain_seat_recalls, second_brain_setup, second_brain_status, set_second_brain_explore,
    set_second_brain_scenes, set_second_brain_weekly_review,
};

pub(crate) use supply_chain::{supply_chain_graph, supply_chain_report};

pub(crate) use artifacts::{
    artifact_copy_path, artifact_counts, artifact_delete, artifact_import_transcripts,
    artifact_open, artifact_preview, artifact_register, artifact_reveal, artifact_search,
    artifact_thumbnail, artifact_versions, artifacts_list,
};
pub(crate) use skills::{
    computer_use_skill_report, install_bundled_skill, list_skills, orchestration_report,
    reveal_skill, skill_bundle_parse, skill_detail, skill_install_plan, skill_reveal, skills_list,
    skills_rescan,
};

pub(crate) use session::{crash_bundle, crash_open_log, set_crash_watchdog};
