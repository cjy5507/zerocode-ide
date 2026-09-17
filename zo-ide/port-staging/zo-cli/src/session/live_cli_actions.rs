//! Command-action handlers for [`LiveCli`].
//!
//! A behaviour-preserving SRP split of the `/teleport`, `/debug-tool-call`,
//! and `/commit` action handlers out of `live_cli_commands.rs`. Each renders
//! a report (and, for `/commit`, inspects the git workspace) and prints it.
//! They stay `impl LiveCli` methods, so the dispatcher's `self.run_*(…)`
//! call sites are unchanged. (`/bughunter`, `/ultraplan`, `/pr`, and
//! `/issue` no longer print reports — they queue real turns from the
//! dispatcher via the `build_*_prompt` builders.)

use super::live_cli::LiveCli;
use crate::git_helpers::{parse_git_status_branch, parse_git_workspace_summary};
use crate::{
    format_commit_preflight_report, format_commit_skipped_report, git_output,
    render_last_tool_debug_report, render_teleport_report, validate_no_args,
};

impl LiveCli {
    pub(super) fn run_teleport(target: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let Some(target) = target.map(str::trim).filter(|value| !value.is_empty()) else {
            println!("Usage: /teleport <symbol-or-path>");
            return Ok(());
        };

        println!("{}", render_teleport_report(target)?);
        Ok(())
    }

    pub(super) fn run_debug_tool_call(
        &self,
        args: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        validate_no_args("/debug-tool-call", args)?;
        println!("{}", render_last_tool_debug_report(self.runtime.session())?);
        Ok(())
    }

    #[allow(clippy::unused_self)]
    pub(super) fn run_commit(
        &mut self,
        args: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        validate_no_args("/commit", args)?;
        let status = git_output(&["status", "--short", "--branch"])?;
        let summary = parse_git_workspace_summary(Some(&status));
        let branch = parse_git_status_branch(Some(&status));
        if summary.is_clean() {
            println!("{}", format_commit_skipped_report());
            return Ok(());
        }

        println!(
            "{}",
            format_commit_preflight_report(branch.as_deref(), summary)
        );
        Ok(())
    }
}
