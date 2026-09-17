use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use commands::{SelfImproveAction, SlashCommand};
use runtime::ConfigLoader;
use serde_json::Value;
use tools::{
    execute_config, execute_enter_plan_mode, execute_exit_plan_mode, ConfigInput, ConfigValue,
    EnterPlanModeInput, ExitPlanModeInput,
};

use crate::cli_args::format_unknown_slash_command;
use crate::session::live_cli::LiveCli;
use crate::session::smart_settings::{execute_deep_tier_command, execute_smart_text_command};
use zo_cli::tui::modals::Effort;
use crate::git_helpers::resolve_git_branch_for;
use crate::{
    build_bughunter_prompt, build_council_prompt, build_distill_prompt, build_issue_prompt,
    build_pr_prompt, build_ultraplan_prompt, create_managed_session_handle,
    render_export_text, render_hooks_report, render_review_report, run_init,
};

use super::{
    config_bool_value, config_string_value, delete_share_gist, last_copy_payload,
    last_thinkback_lines, open_in_desktop, plan_mode_enabled_in_current_worktree,
    render_repl_help_with_prompt_commands, run_pr_comments, sanitize_session_name,
    security_secret_scan, share_gist_warning, upload_share_to_gist, write_share_artifact,
    write_to_clipboard,
};

impl LiveCli {
    pub(crate) fn handle_repl_command(
        &mut self,
        command: SlashCommand,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if let Some(quit) = self.dispatch_basic_view_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_discovery_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_metrics_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = Self::dispatch_release_notes_cmds(&command)? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_environment_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = Self::dispatch_privacy_cmds(&command)? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_project_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = Self::dispatch_workspace_nav_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_sharing_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_rename_copy_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_session_core_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_history_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = Self::dispatch_theme_voice_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = Self::dispatch_preference_simple_cmds(&command)? {
            return Ok(quit);
        }
        if let Some(quit) = Self::dispatch_output_style_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_analysis_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = self.dispatch_tasks_cmds(command.clone())? {
            return Ok(quit);
        }
        if let Some(quit) = Self::dispatch_plan_cmds(command.clone())? {
            return Ok(quit);
        }
        self.dispatch_fallback_cmds(command)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "unhandled slash command").into()
        })
    }

    fn dispatch_basic_view_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Help => {
                println!(
                    "{}",
                    render_repl_help_with_prompt_commands(&self.runtime.prompt_commands)
                );
                false
            }
            SlashCommand::Status => {
                self.print_status();
                false
            }
            SlashCommand::Cost => {
                self.print_cost();
                false
            }
            SlashCommand::Config { section } => {
                Self::print_config(section.as_deref())?;
                false
            }
            SlashCommand::Memory => {
                Self::print_memory()?;
                false
            }
            SlashCommand::Dream => {
                self.run_dream()?;
                false
            }
            SlashCommand::Refine => {
                println!(
                    "{}",
                    crate::session::refine::run_refine(&self.cwd, &self.session.id, None)
                );
                false
            }
            SlashCommand::Diff => {
                Self::print_diff()?;
                false
            }
            SlashCommand::Version => {
                Self::print_version();
                false
            }
            SlashCommand::Feedback => {
                eprintln!("Report issues in the project repository.");
                false
            }
            SlashCommand::Doctor => {
                let cwd = crate::current_cli_cwd()?;
                let report = crate::doctor::run(crate::doctor::DoctorMode::Repair, &cwd);
                eprintln!("{}", report.render());
                false
            }
            SlashCommand::Upgrade => {
                let binary = std::env::current_exe()?;
                // The binary's OWN build commit (stamped at compile time), not
                // the repo's current HEAD — those differ exactly when a rebuild
                // has landed but this process is still the old binary.
                let built_sha = crate::GIT_SHA.unwrap_or("unknown");
                let git_head = Command::new("git")
                    .args(["rev-parse", "--short", "HEAD"])
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map_or_else(
                        || "unknown".to_string(),
                        |output| String::from_utf8_lossy(&output.stdout).trim().to_string(),
                    );
                // Compare the stamped SHA (12 chars) against HEAD (short) by
                // common prefix so a rebuilt-but-not-restarted session is
                // called out explicitly.
                let status = if git_head == "unknown" || built_sha == "unknown" {
                    "cannot compare (not in a git worktree)"
                } else if built_sha.starts_with(&git_head) || git_head.starts_with(built_sha) {
                    "current — running binary matches repo HEAD"
                } else {
                    "STALE — repo HEAD moved; rebuild landed but this session still runs the old binary"
                };
                eprintln!(
                    "Upgrade\n  Version          {}\n  Binary           {}\n  Built from       {}\n  Repo HEAD        {}\n  Status           {}\n  Next step        `just deploy` then /restart (or open a new terminal)",
                    env!("CARGO_PKG_VERSION"),
                    binary.display(),
                    built_sha,
                    git_head,
                    status,
                );
                false
            }
            SlashCommand::Cache => {
                eprintln!("Cache: use /cache in TUI mode for prompt cache diagnostics.");
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_discovery_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Teleport { target } => {
                Self::run_teleport(target.as_deref())?;
                false
            }
            SlashCommand::DebugToolCall => {
                self.run_debug_tool_call(None)?;
                false
            }
            SlashCommand::Mcp { action, target } => {
                let args = match (action.as_deref(), target.as_deref()) {
                    (None, None) => None,
                    (Some(action), None) => Some(action.to_string()),
                    (Some(action), Some(target)) => Some(format!("{action} {target}")),
                    (None, Some(target)) => Some(target.to_string()),
                };
                Self::print_mcp(args.as_deref())?;
                false
            }
            SlashCommand::Tools => {
                eprintln!(
                    "Tools\n  Usage            open the persistent TUI and run /tools\n  Status           runtime tool toggles are interactive"
                );
                false
            }
            SlashCommand::Agents { args } => {
                Self::print_agents(args.as_deref())?;
                false
            }
            SlashCommand::Inbox { args } => {
                self.print_inbox(args.as_deref());
                false
            }
            SlashCommand::Skills { args } => {
                Self::print_skills(args.as_deref())?;
                false
            }
            _ => return Ok(None),
        }))
    }

    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::too_many_lines)] // declarative command→handler dispatch table
    fn dispatch_metrics_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Audit => {
                // Surface the ToolGateway ledger (model-facing `Audit` tool) to
                // the operator: per-tool counts, permission denials, routes.
                let (invocations, summary) = {
                    let ctx = self
                        .runtime
                        .tool_executor_mut()
                        .tool_registry_mut()
                        .context();
                    (ctx.tool_invocations(), ctx.audit_summary())
                };
                let mut by_tool: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for inv in &invocations {
                    *by_tool.entry(inv.request.tool_name.clone()).or_default() += 1;
                }
                eprintln!(
                    "Tool audit (this session):\n  Total: {} ({} ok, {} failed)",
                    summary.total, summary.succeeded, summary.failed
                );
                if !by_tool.is_empty() {
                    let line = by_tool
                        .iter()
                        .map(|(name, count)| format!("{name}×{count}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    eprintln!("  By tool: {line}");
                }
                if !summary.denials.is_empty() {
                    eprintln!("  Denied ({}):", summary.denials.len());
                    for denial in &summary.denials {
                        eprintln!(
                            "    {} — {} (needs {})",
                            denial.tool_name, denial.reason, denial.required_mode
                        );
                    }
                }
                if !summary.route_decisions.is_empty() {
                    let routes = summary
                        .route_decisions
                        .iter()
                        .map(|r| format!("{}({:.2})", r.shape, r.confidence))
                        .collect::<Vec<_>>()
                        .join(", ");
                    eprintln!("  Routes: {routes}");
                }
                false
            }
            SlashCommand::Usage { scope } => {
                if scope.as_deref() == Some("help") {
                    eprintln!(
                        "Usage: /usage\nShows detailed API usage statistics for this session"
                    );
                } else {
                    self.print_cost();
                }
                false
            }
            SlashCommand::Files => {
                match std::process::Command::new("git")
                    .args(["ls-files"])
                    .output()
                {
                    Ok(output) if output.status.success() => {
                        let files = String::from_utf8_lossy(&output.stdout);
                        let count = files.lines().count();
                        eprintln!("Tracked files: {count}");
                        for line in files.lines().take(30) {
                            eprintln!("  {line}");
                        }
                        if count > 30 {
                            eprintln!("  ... and {} more", count - 30);
                        }
                    }
                    _ => eprintln!("Not in a git repository or git not available"),
                }
                false
            }
            SlashCommand::Insights => {
                let usage = self.runtime.usage().cumulative_usage();
                eprintln!(
                    "Insights\n  Session ID       {}\n  Messages         {}\n  Turns            {}\n  Total tokens     {}\n  Input tokens     {}\n  Output tokens    {}",
                    self.session.id,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                    usage.total_tokens(),
                    usage.input_tokens,
                    usage.output_tokens
                );
                false
            }
            SlashCommand::Thinkback => {
                let lines = last_thinkback_lines(self.runtime.session());
                if lines.is_empty() {
                    eprintln!(
                        "Thinkback\n  Result           no prior assistant activity captured yet"
                    );
                } else {
                    eprintln!("Thinkback\n  Recent flow      {}", lines.join("\n  "));
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_release_notes_cmds(
        command: &SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::ReleaseNotes => {
                match std::process::Command::new("git")
                    .args(["log", "--no-merges", "--pretty=format:%h %s", "-n", "10"])
                    .output()
                {
                    Ok(output) if output.status.success() => {
                        let body = String::from_utf8_lossy(&output.stdout);
                        if body.trim().is_empty() {
                            eprintln!(
                                "Release notes\n  Result           no commits found in the current repository"
                            );
                        } else {
                            eprintln!("Release notes\n{}", body.trim());
                        }
                    }
                    _ => eprintln!(
                        "Release notes unavailable: not in a git repository or git is not available."
                    ),
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_environment_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Hooks { args } => {
                if args.as_deref() == Some("help") {
                    eprintln!("Usage: /hooks [list|help]\nManage lifecycle hooks");
                } else {
                    eprintln!("{}", render_hooks_report()?);
                }
                false
            }
            SlashCommand::Context { action } => {
                // CC `/context` parity: live context-window breakdown. Args are
                // not part of the command; surface usage instead of pretending
                // an "update" happened (the old stub's false success).
                if action.is_some() {
                    eprintln!("Usage: /context\nShow the live context-window breakdown");
                } else {
                    eprintln!("{}", self.runtime.context_breakdown_report());
                }
                false
            }
            SlashCommand::ReloadContext => {
                eprintln!("{}", self.reload_context()?);
                false
            }
            SlashCommand::Keybindings => {
                eprintln!(
                    "Keybindings\n  Built-ins        Enter submit | Shift+Enter newline | Ctrl+C cancel | Up/Down history"
                );
                false
            }
            SlashCommand::Effort { level } => {
                // P9: `ultra` is now its own static level (no longer just an
                // alias of the renamed-to-`smart` dynamic band) — keep this
                // in lockstep with the vocabulary in
                // `commands::slash_help::specs` and `slash_dispatch`'s
                // invalid-input message.
                const VALID: &str =
                    "off, low, medium, high, xhigh, max, ultra, smart (aliases: smartcode, ultracode, uc)";
                match level.as_deref() {
                    Some(level) => {
                        if let Some(effort) = Effort::from_token(level) {
                            let warning = self.set_effort(effort);
                            eprintln!("Reasoning effort set to: {}", effort.canonical());
                            if let Some(warning) = warning {
                                eprintln!("{warning}");
                            }
                        } else {
                            eprintln!("Invalid effort level: {level}\nValid levels: {VALID}");
                        }
                    }
                    None => eprintln!(
                        "Current effort: high\nAvailable: {VALID}\nUsage: /effort <level>"
                    ),
                }
                false
            }
            SlashCommand::Ide { .. } => {
                // The WebSocket MCP bridge needs the live TUI session (tool
                // registry splice); the plain REPL points there honestly.
                eprintln!("/ide connects to a running IDE extension in the TUI session");
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_privacy_cmds(
        command: &SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::PrivacySettings => {
                let cwd = crate::current_cli_cwd()?;
                let loader = ConfigLoader::default_for(&cwd);
                let discovered = loader.discover();
                let oauth = runtime::load_oauth_credentials()?.is_some();
                let mcp_servers = runtime::list_mcp_oauth_servers()?;
                let credentials_path = runtime::credentials_path()?;
                eprintln!(
                    "Privacy settings\n  Config files      {}\n  Credentials path  {}\n  OAuth token       {}\n  MCP OAuth servers {}\n  Note              review {} to inspect stored settings",
                    discovered.len(),
                    credentials_path.display(),
                    if oauth { "present" } else { "absent" },
                    if mcp_servers.is_empty() {
                        "none".to_string()
                    } else {
                        mcp_servers.join(", ")
                    },
                    loader.config_home().join("settings.json").display()
                );
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_project_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Commit => {
                self.run_commit(None)?;
                false
            }
            SlashCommand::Ship { message } => {
                let result = crate::session::slash_dispatch::handle_ship_at(
                    &self.cwd,
                    &message,
                    |progress| eprintln!("{progress}"),
                );
                eprintln!("{}", result.report);
                false
            }
            SlashCommand::Pr { context } => {
                let branch = resolve_git_branch_for(&std::env::current_dir()?)
                    .unwrap_or_else(|| "unknown".to_string());
                eprintln!(
                    "PR\n  Branch           {branch}\n  Method           queued a draft-and-create turn (gh)"
                );
                self.run_turn(&build_pr_prompt(&branch, context.as_deref()))?;
                false
            }
            SlashCommand::Issue { context } => {
                eprintln!("Issue\n  Method           queued a draft-and-create turn (gh)");
                self.run_turn(&build_issue_prompt(context.as_deref()))?;
                false
            }
            SlashCommand::Init => {
                run_init()?;
                false
            }
            SlashCommand::Export { path } => {
                self.export_session(path.as_deref())?;
                false
            }
            SlashCommand::PrComments { pr_number } => {
                let report = run_pr_comments(pr_number.as_deref())?;
                eprintln!("{report}");
                false
            }
            SlashCommand::CommitPushPr => {
                let report = crate::session::slash_dispatch::handle_commit_push_pr_at(&self.cwd);
                eprintln!("{report}");
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_workspace_nav_cmds(
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Branch { name } => {
                if let Some(branch_name) = name.as_deref() {
                    let branch_name = branch_name.trim();
                    if branch_name.is_empty()
                        || branch_name.starts_with('-')
                        || branch_name.contains(' ')
                        || branch_name.contains("..")
                    {
                        eprintln!("Error: invalid branch name '{branch_name}'.");
                        return Ok(Some(false));
                    }

                    match std::process::Command::new("git")
                        .args(["checkout", "-b", "--", branch_name])
                        .output()
                    {
                        Ok(output) if output.status.success() => {
                            eprintln!("Switched to new branch: {branch_name}");
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            match std::process::Command::new("git")
                                .args(["checkout", "--", branch_name])
                                .output()
                            {
                                Ok(out) if out.status.success() => {
                                    eprintln!("Switched to branch: {branch_name}");
                                }
                                _ => eprintln!("Failed to switch branch: {stderr}"),
                            }
                        }
                        Err(error) => eprintln!("git error: {error}"),
                    }
                } else {
                    match std::process::Command::new("git")
                        .args(["branch", "--show-current"])
                        .output()
                    {
                        Ok(output) if output.status.success() => {
                            let branch = String::from_utf8_lossy(&output.stdout);
                            eprintln!("Current branch: {}", branch.trim());
                        }
                        _ => eprintln!("Not in a git repository or git not available"),
                    }
                    eprintln!("Usage: /branch <name> to create/switch branches");
                }
                false
            }
            SlashCommand::AddDir { path } => {
                match path.as_deref() {
                    Some(dir_path) => {
                        let resolved = if Path::new(dir_path).is_absolute() {
                            PathBuf::from(dir_path)
                        } else {
                            env::current_dir().unwrap_or_default().join(dir_path)
                        };
                        if resolved.is_dir() {
                            // Canonicalize, then merge into the live workspace
                            // roots so reads/writes under the new directory pass
                            // the boundary check (CC `--add-dir` parity).
                            let canonical =
                                std::fs::canonicalize(&resolved).unwrap_or(resolved);
                            let mut roots =
                                runtime::file_ops::additional_workspace_roots();
                            if roots.contains(&canonical) {
                                eprintln!(
                                    "Directory already in context: {}",
                                    canonical.display()
                                );
                            } else {
                                roots.push(canonical.clone());
                                runtime::file_ops::set_additional_workspace_roots(roots);
                                eprintln!(
                                    "Added directory to context: {}",
                                    canonical.display()
                                );
                            }
                        } else {
                            eprintln!("Directory not found: {}", resolved.display());
                        }
                    }
                    None => {
                        eprintln!(
                            "Usage: /add-dir <path>\nAdds a directory to the working context"
                        );
                    }
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_sharing_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Share { target } => {
                let artifact = write_share_artifact(&self.session.id, self.runtime.session())?;
                eprintln!(
                    "Share artifact created\n  Session          {}\n  File             {}\n  Messages         {}",
                    self.session.id,
                    artifact.path.display(),
                    self.runtime.session().messages.len()
                );
                if target.as_deref() == Some("gist") {
                    // Loud warning BEFORE any bytes leave the machine.
                    eprintln!("{}", share_gist_warning(artifact.char_count));
                    match upload_share_to_gist(&self.session.id, self.runtime.session()) {
                        Ok(url) => eprintln!(
                            "Share gist uploaded\n  URL              {url}\n  Revoke           /unshare <id>"
                        ),
                        // Never hard-fail: the local artifact above still stands.
                        Err(e) => eprintln!(
                            "Share gist upload failed\n  Error            {e}\n  Fallback         local artifact above is still available"
                        ),
                    }
                }
                false
            }
            SlashCommand::Unshare { id } => {
                match id.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
                    Some(id) => match delete_share_gist(id) {
                        Ok(()) => eprintln!("Unshare\n  Gist             {id} deleted"),
                        Err(e) => eprintln!("Unshare\n  Error            {e}"),
                    },
                    None => eprintln!("Unshare\n  Usage            /unshare <gist-id>"),
                }
                false
            }
            SlashCommand::Desktop => {
                open_in_desktop(&self.session.path)?;
                eprintln!(
                    "Desktop open requested\n  Session file     {}",
                    self.session.path.display()
                );
                false
            }
            _ => return Ok(None),
        }))
    }

    /// Rename the current session (its id IS its name): persist under the new
    /// id, remove the old file, and rebuild the runtime onto the new handle.
    /// Shared by the REPL `/rename` arm and the interactive TUI `/rename` so both
    /// genuinely rename instead of one path claiming it is unimplemented. Returns
    /// the human-readable report on success.
    pub(crate) fn rename_session(
        &mut self,
        name: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let next_id = sanitize_session_name(name)?;
        if next_id == self.session.id {
            return Err(format!("session already uses id: {next_id}").into());
        }
        let next_handle = create_managed_session_handle(&next_id, self.session_scope)?;
        if next_handle.path.exists() {
            return Err(format!("session '{next_id}' already exists").into());
        }
        let old_path = self.session.path.clone();
        let old_id = self.session.id.clone();
        let mut updated_session = self
            .runtime
            .session()
            .clone()
            .with_persistence_path(next_handle.path.clone());
        updated_session.session_id.clone_from(&next_id);
        updated_session.save_to_path(&next_handle.path)?;
        if old_path != next_handle.path && old_path.exists() {
            fs::remove_file(&old_path)?;
        }
        let runtime = self.build_runtime(
            updated_session,
            &next_handle.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.session = next_handle;
        Ok(format!(
            "Session renamed\n  Previous ID      {old_id}\n  Current ID       {}\n  Session file     {}",
            self.session.id,
            self.session.path.display()
        ))
    }

    /// Set (or, with no argument, show) this session's display name — distinct
    /// from [`rename_session`](Self::rename_session), which changes the session
    /// *id*. Shared by the TUI `/name` dispatcher and the headless REPL so the
    /// two never diverge (one used to claim the command was unimplemented).
    /// Returns `Ok(report)` on show/set, `Err(usage)` on a rejected argument so
    /// the TUI can render it warn-level while the REPL prints it to stderr.
    pub(crate) fn set_display_name(&mut self, name: Option<&str>) -> Result<String, String> {
        let Some(name) = name else {
            let current = self.runtime.session().name.as_deref().unwrap_or("(unnamed)");
            return Ok(format!(
                "Session name\n  Name             {current}\n  Usage            /name <name>"
            ));
        };
        let name = name.trim();
        if name.is_empty() || name.chars().count() > commands::MAX_SESSION_NAME_CHARS {
            return Err(format!(
                "Usage: /name <name> (maximum {} characters)",
                commands::MAX_SESSION_NAME_CHARS
            ));
        }
        self.runtime.session_mut().name = Some(name.to_string());
        self.persist_session().map_err(|error| error.to_string())?;
        Ok(format!("Session name\n  Name             ● {name}"))
    }

    fn dispatch_rename_copy_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Rename { name } => {
                let Some(name) = name.as_deref() else {
                    eprintln!("Usage: /rename <name>");
                    return Ok(Some(false));
                };
                match self.rename_session(name) {
                    Ok(report) => eprintln!("{report}"),
                    Err(error) => eprintln!("Rename failed: {error}"),
                }
                false
            }
            SlashCommand::Copy { target } => {
                let payload = match target.as_deref() {
                    None | Some("last") => {
                        last_copy_payload(self.runtime.session()).ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::NotFound,
                                "no session content available to copy",
                            )
                        })?
                    }
                    Some("all") => render_export_text(self.runtime.session()),
                    Some(other) => {
                        eprintln!("Unknown copy target: {other}\nUsage: /copy [last|all]");
                        return Ok(Some(false));
                    }
                };
                write_to_clipboard(&payload)?;
                eprintln!(
                    "Copied to clipboard\n  Target           {}\n  Characters       {}",
                    target.as_deref().unwrap_or("last"),
                    payload.chars().count()
                );
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_session_core_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Goal { command } => {
                self.handle_goal_command_repl(command)?;
                false
            }
            SlashCommand::Loop { command } => {
                self.handle_loop_command_repl(command)?;
                false
            }
            SlashCommand::Compact { instructions } => {
                self.compact(instructions)?;
                false
            }
            SlashCommand::Model { model } => self.set_model(model)?,
            SlashCommand::Permissions { mode } => self.set_permissions(mode)?,
            SlashCommand::Clear { confirm } => self.clear_session(confirm)?,
            SlashCommand::Resume { session_path } => {
                self.resume_session(session_path.as_deref())?
            }
            SlashCommand::Session { action, target } => {
                self.handle_session_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Plugins { action, target } => {
                self.handle_plugins_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Login { provider } => {
                // A failed sign-in is the user's problem to retry, not grounds
                // for tearing the REPL down: propagating here ended the session
                // on a browser that was simply closed, or on the 3-minute
                // callback timeout.
                if let Err(error) =
                    crate::auth::run_login_provider(provider.as_deref().unwrap_or("claude"))
                {
                    eprintln!("Login failed: {error}");
                }
                false
            }
            SlashCommand::Logout { scope } => {
                // No modal in the headless REPL: bare `/logout` prints the
                // connection list so the user can see what `all` would clear.
                match scope.as_deref().map(str::trim) {
                    Some(scope) if scope.eq_ignore_ascii_case("all") => {
                        crate::auth::run_logout()?;
                    }
                    Some(other) if !other.is_empty() => {
                        eprintln!("Unknown /logout argument: {other}  (use /logout all)");
                    }
                    _ => {
                        println!(
                            "{}\n\n  /logout all  clears every saved OAuth credential",
                            crate::session::slash_dispatch::providers_text_report()
                        );
                    }
                }
                false
            }
            SlashCommand::Fast { mode } => {
                eprintln!("{}", self.toggle_fast(mode.as_deref()));
                false
            }
            SlashCommand::Smart { arg } => {
                match execute_smart_text_command(&self.model, arg.as_deref()) {
                    Ok(message) => eprintln!("{message}"),
                    Err(error) => eprintln!("Smart Router
  Error            {error}"),
                }
                false
            }
            SlashCommand::DeepTier { action } => {
                match execute_deep_tier_command(&self.cwd, &action) {
                    Ok(message) | Err(message) => eprintln!("{message}"),
                }
                false
            }
            SlashCommand::Fork { name } => {
                let forked = self.runtime.session().fork(name);
                let fork_id = forked.session_id.clone();
                eprintln!("Session forked: {fork_id}");
                false
            }
            SlashCommand::Focus => {
                eprintln!("Focus mode: use F11 in TUI mode to toggle.");
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_history_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Rewind { action } => {
                match self.workspace_rewind_report(&action) {
                    Ok(report) => eprintln!("{report}"),
                    Err(error) => eprintln!("Workspace rewind failed: {error}"),
                }
                false
            }
            SlashCommand::Undo { steps } => {
                let n: usize = steps.as_deref().and_then(|s| s.parse().ok()).unwrap_or(1);
                if let Some(ref mut stack) = self.snapshot_stack {
                    for _ in 0..n {
                        match stack.undo() {
                            Ok(r) => eprintln!(
                                "Undo: reverted to turn {}. {} snapshots remaining.",
                                r.restored_turn, r.remaining
                            ),
                            Err(e) => {
                                eprintln!("Undo failed: {e}");
                                break;
                            }
                        }
                    }
                } else {
                    eprintln!("Not in a git repository — undo unavailable.");
                }
                false
            }
            SlashCommand::Redo { steps } => {
                let n: usize = steps.as_deref().and_then(|s| s.parse().ok()).unwrap_or(1);
                if let Some(ref mut stack) = self.snapshot_stack {
                    for _ in 0..n {
                        match stack.redo() {
                            Ok(r) => eprintln!(
                                "Redo: restored to turn {}. {} redo(s) remaining.",
                                r.restored_turn, r.remaining
                            ),
                            Err(e) => {
                                eprintln!("Redo failed: {e}");
                                break;
                            }
                        }
                    }
                } else {
                    eprintln!("Not in a git repository — redo unavailable.");
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_theme_voice_cmds(
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Theme { name } => {
                if let Some(name) = name.as_deref() {
                    let output = execute_config(ConfigInput {
                        setting: "theme".to_string(),
                        value: Some(ConfigValue::String(name.to_string())),
                    })
                    .map_err(io::Error::other)?;
                    let rendered = serde_json::to_value(&output).map_err(io::Error::other)?;
                    let applied = rendered
                        .get("newValue")
                        .and_then(Value::as_str)
                        .unwrap_or(name);
                    eprintln!("Theme set to: {applied}");
                } else {
                    let output = execute_config(ConfigInput {
                        setting: "theme".to_string(),
                        value: None,
                    })
                    .map_err(io::Error::other)?;
                    let rendered = serde_json::to_value(&output).map_err(io::Error::other)?;
                    let current = rendered
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or("default");
                    eprintln!(
                        "Current theme: {current}\nAvailable: default, dark, light, high-contrast\nUsage: /theme <name>"
                    );
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_preference_simple_cmds(
        command: &SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Brief => {
                let current = config_string_value(
                    &execute_config(ConfigInput {
                        setting: "outputStyle".to_string(),
                        value: None,
                    })
                    .map_err(io::Error::other)?,
                )?
                .unwrap_or_else(|| "markdown".to_string());
                let next = if current == "concise" {
                    "markdown"
                } else {
                    "concise"
                };
                let output = execute_config(ConfigInput {
                    setting: "outputStyle".to_string(),
                    value: Some(ConfigValue::String(next.to_string())),
                })
                .map_err(io::Error::other)?;
                let rendered = serde_json::to_value(&output).map_err(io::Error::other)?;
                let applied = rendered
                    .get("newValue")
                    .and_then(Value::as_str)
                    .unwrap_or(next);
                eprintln!("Brief mode set output style to: {applied}");
                false
            }
            SlashCommand::Advisor => {
                let current = config_bool_value(
                    &execute_config(ConfigInput {
                        setting: "advisorModeEnabled".to_string(),
                        value: None,
                    })
                    .map_err(io::Error::other)?,
                )?
                .unwrap_or(false);
                let next = !current;
                execute_config(ConfigInput {
                    setting: "advisorModeEnabled".to_string(),
                    value: Some(ConfigValue::Bool(next)),
                })
                .map_err(io::Error::other)?;
                eprintln!(
                    "Advisor mode {}\n  Effect           prompts guidance-oriented behavior for future sessions that honor this setting",
                    if next { "enabled" } else { "disabled" }
                );
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_output_style_cmds(
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::OutputStyle { style } => {
                #[allow(clippy::single_match_else)]
                match style.as_deref() {
                    Some(style) => {
                        let normalized = style.to_lowercase();
                        let output = execute_config(ConfigInput {
                            setting: "outputStyle".to_string(),
                            value: Some(ConfigValue::String(normalized.clone())),
                        })
                        .map_err(io::Error::other)?;
                        let rendered = serde_json::to_value(&output).map_err(io::Error::other)?;
                        if rendered.get("success").and_then(Value::as_bool) == Some(true) {
                            let applied = rendered
                                .get("newValue")
                                .and_then(Value::as_str)
                                .unwrap_or(normalized.as_str());
                            eprintln!("Output style set to: {applied}");
                        } else {
                            let error = rendered
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown config error");
                            eprintln!("{error}");
                        }
                    }
                    None => {
                        let current = config_string_value(
                            &execute_config(ConfigInput {
                                setting: "outputStyle".to_string(),
                                value: None,
                            })
                            .map_err(io::Error::other)?,
                        )?
                        .unwrap_or_else(|| "markdown".to_string());
                        eprintln!(
                            "Current output style: {current}\nAvailable: concise, verbose, markdown, plain, json\nUsage: /output-style <style>"
                        );
                    }
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_analysis_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Bughunter { scope } => {
                eprintln!(
                    "Bughunter\n  Scope            {}\n  Method           queued a bug-hunt turn",
                    scope.as_deref().unwrap_or("the current repository")
                );
                self.run_turn(&build_bughunter_prompt(scope.as_deref()))?;
                false
            }
            SlashCommand::Ultraplan { task } => {
                eprintln!(
                    "Ultraplan\n  Task             {}\n  Method           queued a planning turn",
                    task.as_deref().unwrap_or("the current repo work")
                );
                self.run_turn(&build_ultraplan_prompt(task.as_deref()))?;
                false
            }
            SlashCommand::Council { task } => {
                eprintln!(
                    "Council\n  Method           SpawnMultiAgent + Council turn\n  Task             {}",
                    task.as_deref().unwrap_or("the current task")
                );
                self.run_turn(&build_council_prompt(task.as_deref()))?;
                false
            }
            SlashCommand::Distill { topic } => {
                eprintln!(
                    "Distill\n  Method           SkillDistill draft turn\n  Topic            {}",
                    topic
                        .as_deref()
                        .unwrap_or("the reusable procedure from this session")
                );
                self.run_turn(&build_distill_prompt(topic.as_deref()))?;
                false
            }
            SlashCommand::SelfImprove { action } => {
                let outcome = match action {
                    SelfImproveAction::Status => crate::session::self_improve::status_report(
                        &self.cwd,
                        self.runtime.feature_config.dream_automation_enabled(),
                    ),
                    SelfImproveAction::Apply { patch_digest } => {
                        crate::session::self_improve::apply(&self.cwd, &patch_digest)
                    }
                    SelfImproveAction::Show { proposal_id } => {
                        crate::session::self_improve::show(&self.cwd, &proposal_id)
                    }
                    SelfImproveAction::Review { proposal_id } => {
                        crate::session::self_improve::review(&self.cwd, &proposal_id)
                    }
                    SelfImproveAction::Reject { proposal_id } => {
                        crate::session::self_improve::reject(&self.cwd, &proposal_id)
                    }
                    SelfImproveAction::Propose => {
                        crate::session::self_improve::ZoSubprocessGenerator::from_current_exe()
                            .and_then(|generator| {
                                crate::session::self_improve::propose(&self.cwd, &generator)
                            })
                            .map(|proposal| {
                                proposal.as_ref().map_or_else(
                                    || "No self-improvement candidates to act on yet.".to_string(),
                                    crate::session::self_improve::format_proposal,
                                )
                            })
                    }
                };
                match outcome {
                    Ok(report) => println!("{report}"),
                    Err(error) => eprintln!(
                        "/improve\n  Status           {}",
                        crate::session::self_improve::escape_terminal(&error)
                    ),
                }
                false
            }
            SlashCommand::Review { scope } => {
                eprintln!("{}", render_review_report(scope.as_deref())?);
                false
            }
            SlashCommand::SecurityReview => {
                let review = render_review_report(None)?;
                let secret_scan = security_secret_scan(None);
                eprintln!("Security review\n{review}\n{secret_scan}");
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_tasks_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Tasks { args } => {
                if args.as_deref() == Some("help") {
                    eprintln!(
                        "Usage: /tasks [list|get <id>|stop <id>|help]\nManage tasks in the current process"
                    );
                } else {
                    let registry = self.runtime.tool_executor_mut().tool_registry_mut();
                    let tasks = &mut registry.context_mut().tasks;
                    match args.as_deref() {
                        None | Some("list") => {
                            let all = tasks.list(None);
                            if all.is_empty() {
                                eprintln!(
                                    "Tasks\n  Count            0\n  Result           no active tasks are currently registered in this process"
                                );
                            } else {
                                eprintln!("Tasks\n  Count            {}", all.len());
                                for task in all {
                                    eprintln!(
                                        "  {}  {}  {}",
                                        task.task_id,
                                        task.status,
                                        task.description.as_deref().unwrap_or(task.prompt.as_str())
                                    );
                                }
                            }
                        }
                        Some(raw) if raw.starts_with("get ") => {
                            let task_id = raw.trim_start_matches("get ").trim();
                            match tasks.get(task_id) {
                                Some(task) => {
                                    eprintln!(
                                        "Task\n  ID               {}\n  Status           {}\n  Prompt           {}\n  Description      {}\n  Messages         {}\n  Output bytes     {}",
                                        task.task_id,
                                        task.status,
                                        task.prompt,
                                        task.description.as_deref().unwrap_or("<none>"),
                                        task.messages.len(),
                                        task.output.len()
                                    );
                                }
                                None => eprintln!("Task not found: {task_id}"),
                            }
                        }
                        Some(raw) if raw.starts_with("stop ") => {
                            let task_id = raw.trim_start_matches("stop ").trim();
                            match tasks.stop(task_id) {
                                Ok(task) => {
                                    eprintln!("Stopped task {} ({})", task.task_id, task.status);
                                }
                                Err(error) => eprintln!("{error}"),
                            }
                        }
                        Some(other) => {
                            eprintln!(
                                "Unsupported /tasks arguments: {other}\nUsage: /tasks [list|get <id>|stop <id>|help]"
                            );
                        }
                    }
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_plan_cmds(
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Plan { mode } => {
                match mode.as_deref() {
                    Some("on" | "true" | "1") => {
                        let output = execute_enter_plan_mode(EnterPlanModeInput::default())
                            .map_err(io::Error::other)?;
                        let rendered = serde_json::to_value(&output).map_err(io::Error::other)?;
                        let message = rendered
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Enabled worktree-local plan mode override.");
                        eprintln!("{message}");
                    }
                    Some("off" | "false" | "0") => {
                        let output = execute_exit_plan_mode(ExitPlanModeInput::default())
                            .map_err(io::Error::other)?;
                        let rendered = serde_json::to_value(&output).map_err(io::Error::other)?;
                        let message = rendered
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Restored the prior worktree-local plan mode setting.");
                        eprintln!("{message}");
                    }
                    Some(other) => eprintln!("Invalid planning mode: {other}. Use /plan [on|off]."),
                    None => {
                        let active = plan_mode_enabled_in_current_worktree()?;
                        eprintln!(
                            "Planning mode is currently {}.\nUsage: /plan [on|off]",
                            if active { "enabled" } else { "disabled" }
                        );
                    }
                }
                false
            }
            _ => return Ok(None),
        }))
    }

    fn dispatch_fallback_cmds(
        &mut self,
        command: SlashCommand,
    ) -> Result<Option<bool>, Box<dyn std::error::Error>> {
        Ok(Some(match command {
            SlashCommand::Exit => return Ok(Some(true)),
            SlashCommand::BackfillSessions
            | SlashCommand::ExtraUsage
            | SlashCommand::PerfIssue
            | SlashCommand::Statusline
            | SlashCommand::AntTrace
            | SlashCommand::Dump { .. }
            | SlashCommand::Restart
            | SlashCommand::Hunks
            | SlashCommand::New => {
                eprintln!(
                    "This command is available in TUI mode. Use the persistent TUI for full support."
                );
                false
            }
            SlashCommand::Remote { .. } => {
                // Remote pairing needs the live TUI session: it runs a background
                // gateway, renders a QR/pairing code, and drives an interactive
                // approval loop — none of which exist on the headless REPL.
                eprintln!(
                    "/remote pairs a phone with a live session and needs the interactive TUI (background gateway + QR + approval). Launch `zo` and run /remote there."
                );
                false
            }
            SlashCommand::Name { name } => {
                // Ok (show/set) and Err (usage) both go to stderr headlessly.
                match self.set_display_name(name.as_deref()) {
                    Ok(report) | Err(report) => eprintln!("{report}"),
                }
                false
            }
            SlashCommand::Deep { check } => {
                // Same deep-lane gate the TUI installs; it routes the *next* turn
                // of this REPL session through plan → implement → verify → retry.
                let (config, message) = crate::deep_gate_directive(check.as_deref());
                self.runtime.set_deep_gate(config);
                eprintln!("{message}");
                false
            }
            SlashCommand::Auto { arg } => {
                let (config, message) = crate::auto_gate_directive(arg.as_deref());
                self.runtime.set_deep_gate(config);
                eprintln!("{message}");
                false
            }
            SlashCommand::Connect { provider } => {
                use crate::session::slash_dispatch::{ConnectReport, connect_preset};
                match provider.as_deref() {
                    Some(prov) => match connect_preset(prov) {
                        Some(ConnectReport::Info(message) | ConnectReport::Warn(message)) => {
                            println!("{message}");
                        }
                        Some(ConnectReport::Error(message)) => eprintln!("{message}"),
                        None => eprintln!(
                            "'{prov}' is not a writable preset. Presets: deepseek, kimi, qwen, ollama, lmstudio, or an OpenAI-compatible URL (https://host/v1). OAuth providers (openai/google/claude) use the TUI or /login."
                        ),
                    },
                    None => eprintln!(
                        "Usage: /connect <provider>  (deepseek, kimi, qwen, ollama, lmstudio, or a https://host/v1 URL)"
                    ),
                }
                false
            }
            SlashCommand::Providers => {
                // No modal in the headless REPL: print the same tree as text so
                // the registry is still inspectable from a pipe or a script.
                println!("{}", crate::session::slash_dispatch::providers_text_report());
                false
            }
            SlashCommand::Unknown { name, args } => {
                if let Some(command) = self.runtime.prompt_command(&name).cloned() {
                    self.run_prompt_command(&command, args.as_deref().unwrap_or_default())?;
                    return Ok(Some(false));
                }
                // Plugin-contributed slash commands execute against the loaded
                // plugin registry; fall back to the not-found message otherwise.
                match self
                    .runtime
                    .plugin_registry
                    .run_slash_command(&name, args.as_deref().unwrap_or_default())
                {
                    Ok(output) => println!("{output}"),
                    Err(plugins::PluginError::NotFound(_)) => {
                        eprintln!("{}", format_unknown_slash_command(&name));
                    }
                    Err(error) => eprintln!("{error}"),
                }
                false
            }
            _ => return Ok(None),
        }))
    }
}
