use std::path::{Path, PathBuf};

use commands::SlashCommand;
use runtime::{CompactionConfig, Session, TokenUsage, UsageTracker};

use crate::cli_args::format_unknown_slash_command;
use crate::formatting::{format_compact_report, format_cost_report};
use crate::git_helpers::GitWorkspaceSummary;
use crate::{
    default_permission_mode, format_status_report, init_context_md,
    render_config_report, render_diff_report_for, render_export_text, render_memory_report,
    render_repl_help, render_version_report, resolve_export_path, status_context,
    write_session_clear_backup,
};

#[derive(Debug, Clone)]
pub(crate) struct ResumeCommandOutcome {
    pub(crate) session: Session,
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StatusContext {
    pub(crate) cwd: PathBuf,
    pub(crate) session_path: Option<PathBuf>,
    pub(crate) loaded_config_files: usize,
    pub(crate) discovered_config_files: usize,
    pub(crate) instruction_file_count: usize,
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) git_branch: Option<String>,
    pub(crate) git_summary: GitWorkspaceSummary,
    pub(crate) sandbox_status: runtime::SandboxStatus,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StatusUsage {
    pub(crate) message_count: usize,
    pub(crate) turns: u32,
    pub(crate) latest: TokenUsage,
    pub(crate) cumulative: TokenUsage,
    pub(crate) estimated_tokens: usize,
}

pub(crate) fn resume_session(session_path: &Path, from_turn: Option<u32>, commands: &[String]) {
    let resolved_path = if session_path.exists() {
        session_path.to_path_buf()
    } else {
        match crate::resolve_session_reference(&session_path.display().to_string()) {
            Ok(handle) => handle.path,
            Err(error) => {
                eprintln!("failed to restore session: {error}");
                std::process::exit(1);
            }
        }
    };

    let session = match Session::load_from_path_from_turn(&resolved_path, from_turn) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("failed to restore session: {error}");
            std::process::exit(1);
        }
    };

    if commands.is_empty() {
        println!(
            "Restored session from {} ({} messages).",
            resolved_path.display(),
            session.messages.len()
        );
        return;
    }

    let mut session = session;
    for raw_command in commands {
        let command = match SlashCommand::parse(raw_command) {
            Ok(Some(command)) => command,
            Ok(None) => {
                eprintln!("unsupported resumed command: {raw_command}");
                std::process::exit(2);
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        };
        match run_resume_command(&resolved_path, &session, &command) {
            Ok(ResumeCommandOutcome {
                session: next_session,
                message,
            }) => {
                session = next_session;
                if let Some(message) = message {
                    println!("{message}");
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }
}

#[allow(clippy::too_many_lines)] // session resume orchestration, cohesive
pub(crate) fn run_resume_command(
    session_path: &Path,
    session: &Session,
    command: &SlashCommand,
) -> Result<ResumeCommandOutcome, Box<dyn std::error::Error>> {
    match command {
        SlashCommand::Help => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_repl_help()),
        }),
        SlashCommand::Audit => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(
                "/audit is only available in a live session (no tool ledger in a resumed view)"
                    .to_string(),
            ),
        }),
        SlashCommand::Compact { instructions } => {
            let config = CompactionConfig {
                max_estimated_tokens: 0,
                ..CompactionConfig::default()
            };
            let result = match instructions
                .as_deref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                Some(focus) => runtime::compact_session_with(
                    session,
                    config,
                    &runtime::FocusSummarizer { focus },
                ),
                None => runtime::compact_session(session, config),
            };
            let removed = result.removed_message_count;
            let kept = result.compacted_session.messages.len();
            let skipped = removed == 0;
            result.compacted_session.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: result.compacted_session,
                message: Some(format_compact_report(removed, kept, skipped)),
            })
        }
        SlashCommand::Clear { confirm } => {
            if !confirm {
                return Ok(ResumeCommandOutcome {
                    session: session.clone(),
                    message: Some(
                        "clear: confirmation required; rerun with /clear --confirm".to_string(),
                    ),
                });
            }
            let backup_path = write_session_clear_backup(session, session_path)?;
            let previous_session_id = session.session_id.clone();
            let cleared = Session::new();
            let new_session_id = cleared.session_id.clone();
            cleared.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: cleared,
                message: Some(format!(
                    "Session cleared\n  Mode             resumed session reset\n  Previous session {previous_session_id}\n  Backup           {}\n  Resume previous  zo --resume {}\n  New session      {new_session_id}\n  Session file     {}",
                    backup_path.display(),
                    backup_path.display(),
                    session_path.display()
                )),
            })
        }
        SlashCommand::Status => {
            let tracker = UsageTracker::from_session(session);
            let usage = tracker.cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_status_report(
                    "restored-session",
                    StatusUsage {
                        message_count: session.messages.len(),
                        turns: tracker.turns(),
                        latest: tracker.current_turn_usage(),
                        cumulative: usage,
                        estimated_tokens: 0,
                    },
                    default_permission_mode().as_str(),
                    &status_context(Some(session_path))?,
                )),
            })
        }
        SlashCommand::Name { name } => {
            let Some(name) = name.as_deref() else {
                return Ok(ResumeCommandOutcome {
                    session: session.clone(),
                    message: Some(format!(
                        "Session name\n  Name             {}\n  Usage            /name <name>",
                        session.name.as_deref().unwrap_or("(unnamed)")
                    )),
                });
            };
            let name = name.trim();
            if name.is_empty() || name.chars().count() > commands::MAX_SESSION_NAME_CHARS {
                return Err(format!(
                    "Usage: /name <name> (maximum {} characters)",
                    commands::MAX_SESSION_NAME_CHARS
                )
                .into());
            }
            let mut named = session.clone();
            named.name = Some(name.to_string());
            named.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: named,
                message: Some(format!("Session name\n  Name             ● {name}")),
            })
        }
        SlashCommand::Cost => {
            let usage = UsageTracker::from_session(session).cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_cost_report(usage)),
            })
        }
        SlashCommand::Config { section } => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_config_report(section.as_deref())?),
        }),
        SlashCommand::Mcp { action, target } => {
            let cwd = crate::current_cli_cwd()?;
            let args = match (action.as_deref(), target.as_deref()) {
                (None, None) => None,
                (Some(action), None) => Some(action.to_string()),
                (Some(action), Some(target)) => Some(format!("{action} {target}")),
                (None, Some(target)) => Some(target.to_string()),
            };
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(commands::handle_mcp_slash_command(args.as_deref(), &cwd)?),
            })
        }
        SlashCommand::Memory => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_memory_report()?),
        }),
        SlashCommand::Dream => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some({
                let cwd = crate::current_cli_cwd()?;
                runtime::dream_at_cwd(&cwd)
                    .map(|report| format!("Dream\n  Result           {}", report.summary_line()))?
            }),
        }),
        SlashCommand::Refine => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some({
                let cwd = crate::current_cli_cwd()?;
                crate::session::refine::run_refine(&cwd, &session.session_id, None)
            }),
        }),
        SlashCommand::Init => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(init_context_md()?),
        }),
        SlashCommand::Diff => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_diff_report_for(
                session_path.parent().unwrap_or_else(|| Path::new(".")),
            )?),
        }),
        SlashCommand::Version => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_version_report()),
        }),
        SlashCommand::Export { path } => {
            let export_path = resolve_export_path(path.as_deref(), session)?;
            let text = render_export_text(session);
            crate::write_atomic(&export_path, text.as_bytes())?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format!(
                    "Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}",
                    export_path.display(),
                    session.messages.len(),
                )),
            })
        }
        SlashCommand::Copy { target } => {
            let payload = match target.as_deref() {
                None | Some("last") => session
                    .messages
                    .iter()
                    .rev()
                    .find_map(|message| {
                        message
                            .blocks
                            .iter()
                            .rev()
                            .map(|block| match block {
                                runtime::ContentBlock::Text { text } => text.clone(),
                                runtime::ContentBlock::ToolResult { output, .. } => output.clone(),
                                runtime::ContentBlock::ToolUse { input, .. } => input.clone(),
                                runtime::ContentBlock::Image { media_type, .. } => {
                                    format!("[image: {media_type}]")
                                }
                                runtime::ContentBlock::Thinking { .. } => "[thinking]".to_string(),
                                runtime::ContentBlock::RedactedThinking { .. } => {
                                    "[redacted thinking]".to_string()
                                }
                            })
                            .next()
                    })
                    .ok_or("no session content available to copy")?,
                Some("all") => render_export_text(session),
                Some(other) => {
                    return Err(
                        format!("Unknown copy target: {other}\nUsage: /copy [last|all]").into(),
                    );
                }
            };
            crate::session::write_to_clipboard(&payload)?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format!(
                    "Copied to clipboard\n  Target           {}\n  Characters       {}",
                    target.as_deref().unwrap_or("last"),
                    payload.chars().count()
                )),
            })
        }
        SlashCommand::Agents { args } => {
            let cwd = crate::current_cli_cwd()?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(commands::handle_agents_slash_command(
                    args.as_deref(),
                    &cwd,
                )?),
            })
        }
        SlashCommand::Inbox { args } => {
            let cwd = crate::current_cli_cwd()?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(crate::session::report_services::inbox_command(
                    &cwd,
                    &session.session_id,
                    args.as_deref(),
                )),
            })
        }
        SlashCommand::Skills { args } => {
            let cwd = crate::current_cli_cwd()?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(commands::handle_skills_slash_command(
                    args.as_deref(),
                    &cwd,
                )?),
            })
        }
        SlashCommand::DeepTier { action } => {
            let cwd = crate::current_cli_cwd()?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(
                    crate::session::smart_settings::execute_deep_tier_command(&cwd, action)
                        .map_err(std::io::Error::other)?,
                ),
            })
        }
        SlashCommand::Unknown { name, .. } => Err(format_unknown_slash_command(name).into()),
        SlashCommand::Bughunter { .. }
        | SlashCommand::Commit
        | SlashCommand::Pr { .. }
        | SlashCommand::Issue { .. }
        | SlashCommand::Ultraplan { .. }
        | SlashCommand::Teleport { .. }
        | SlashCommand::DebugToolCall
        | SlashCommand::Resume { .. }
        | SlashCommand::Model { .. }
        | SlashCommand::Permissions { .. }
        | SlashCommand::Tools
        | SlashCommand::Session { .. }
        | SlashCommand::Plugins { .. }
        | SlashCommand::Doctor
        | SlashCommand::Login { .. }
        | SlashCommand::Logout { .. }
        | SlashCommand::Upgrade
        | SlashCommand::Restart
        | SlashCommand::Share { .. }
        | SlashCommand::Unshare { .. }
        | SlashCommand::Feedback
        | SlashCommand::Files
        | SlashCommand::Fast { .. }
        | SlashCommand::Smart { .. }
        | SlashCommand::Exit
        | SlashCommand::Desktop
        | SlashCommand::Brief
        | SlashCommand::Advisor
        | SlashCommand::Insights
        | SlashCommand::Thinkback
        | SlashCommand::ReleaseNotes
        | SlashCommand::SecurityReview
        | SlashCommand::Keybindings
        | SlashCommand::PrivacySettings
        | SlashCommand::Plan { .. }
        | SlashCommand::Deep { .. }
        | SlashCommand::Auto { .. }
        | SlashCommand::Review { .. }
        | SlashCommand::Hunks
        | SlashCommand::Tasks { .. }
        | SlashCommand::Theme { .. }
        | SlashCommand::Usage { .. }
        | SlashCommand::Rename { .. }
        | SlashCommand::Hooks { .. }
        | SlashCommand::ReloadContext
        | SlashCommand::Context { .. }
        | SlashCommand::Effort { .. }
        | SlashCommand::Branch { .. }
        | SlashCommand::Rewind { .. }
        | SlashCommand::Undo { .. }
        | SlashCommand::Redo { .. }
        | SlashCommand::Cache
        | SlashCommand::Fork { .. }
        | SlashCommand::Focus
        | SlashCommand::Ide { .. }
        | SlashCommand::OutputStyle { .. }
        | SlashCommand::AddDir { .. }
        | SlashCommand::PrComments { .. }
        | SlashCommand::Ship { .. }
        | SlashCommand::CommitPushPr
        | SlashCommand::BackfillSessions
        | SlashCommand::ExtraUsage
        | SlashCommand::PerfIssue
        | SlashCommand::Statusline
        | SlashCommand::AntTrace
        | SlashCommand::Council { .. }
        | SlashCommand::Distill { .. }
        | SlashCommand::SelfImprove { .. }
        | SlashCommand::Remote { .. }
        | SlashCommand::Goal { .. }
        | SlashCommand::Loop { .. }
        | SlashCommand::New
        | SlashCommand::Dump { .. }
        | SlashCommand::Connect { .. }
        | SlashCommand::Providers => Err("unsupported resumed slash command".into()),
    }
}
