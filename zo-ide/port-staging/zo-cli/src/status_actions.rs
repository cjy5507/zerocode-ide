use std::env;
use std::path::PathBuf;

use compat_harness::{extract_manifest, UpstreamPaths};
use runtime::{
    load_system_prompt, resolve_sandbox_status, ConfigLoader, PermissionMode, TokenUsage,
};
use tools::GlobalToolRegistry;

use crate::resume::StatusUsage;
use crate::session::build_runtime_plugin_state_with_loader;
use crate::{format_status_report, render_version_report, status_context};

pub(crate) fn current_tool_registry() -> Result<GlobalToolRegistry, String> {
    let cwd = crate::current_cli_cwd().map_err(|error| error.to_string())?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader.load().map_err(|error| error.to_string())?;
    let state = build_runtime_plugin_state_with_loader(&cwd, &loader, &runtime_config, None)
        .map_err(|error| error.to_string())?;
    let registry = state.tool_registry.clone();
    if let Some(mcp_state) = state.mcp_state {
        mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown()
            .map_err(|error| error.to_string())?;
    }
    Ok(registry)
}

pub(crate) fn dump_manifests() {
    let workspace_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let paths = UpstreamPaths::from_workspace_dir(&workspace_dir);
    match extract_manifest(&paths) {
        Ok(manifest) => {
            println!("commands: {}", manifest.commands.entries().len());
            println!("tools: {}", manifest.tools.entries().len());
            println!("bootstrap phases: {}", manifest.bootstrap.phases().len());
        }
        Err(error) => {
            eprintln!("failed to extract manifests: {error}");
            std::process::exit(1);
        }
    }
}

pub(crate) fn print_bootstrap_plan() {
    for phase in runtime::BootstrapPlan::claude_code_default().phases() {
        println!("- {phase:?}");
    }
}

pub(crate) fn print_system_prompt(cwd: PathBuf, date: String, model: Option<&str>) {
    match load_system_prompt(cwd, date, env::consts::OS, "unknown", model) {
        Ok(sections) => println!("{}", sections.join("\n\n")),
        Err(error) => {
            eprintln!("failed to build system prompt: {error}");
            std::process::exit(1);
        }
    }
}

pub(crate) fn print_version() {
    println!("{}", render_version_report());
}

pub(crate) fn print_status_snapshot(
    model: &str,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        format_status_report(
            model,
            StatusUsage {
                message_count: 0,
                turns: 0,
                latest: TokenUsage::default(),
                cumulative: TokenUsage::default(),
                estimated_tokens: 0,
            },
            permission_mode.as_str(),
            &status_context(None)?,
        )
    );
    Ok(())
}

pub(crate) fn print_sandbox_status_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader
        .load()
        .unwrap_or_else(|_| runtime::RuntimeConfig::empty());
    println!(
        "{}",
        format_sandbox_status_snapshot(&resolve_sandbox_status(runtime_config.sandbox(), &cwd))
    );
    Ok(())
}

fn format_sandbox_status_snapshot(status: &runtime::SandboxStatus) -> String {
    format!(
        "{}\n  HOME/TMPDIR redirected {}\n  Outside writes blocked {}\n  macOS Seatbelt opt-in {}",
        crate::format_sandbox_report(status),
        status.home_tmp_redirected,
        status.filesystem_write_blocking_active,
        status.macos_seatbelt_opt_in,
    )
}

#[cfg(test)]
mod tests {
    use super::{format_sandbox_status_snapshot, print_version};
    use crate::render_version_report;

    #[test]
    fn print_version_uses_shared_renderer_text() {
        assert_eq!(render_version_report(), crate::render_version_report());
        let _ = print_version as fn();
    }

    #[test]
    fn sandbox_snapshot_distinguishes_redirection_from_write_blocking() {
        let status = runtime::SandboxStatus {
            enabled: true,
            home_tmp_redirected: true,
            filesystem_active: false,
            filesystem_write_blocking_active: false,
            macos_seatbelt_opt_in: false,
            ..runtime::SandboxStatus::default()
        };
        let report = format_sandbox_status_snapshot(&status);

        assert!(report.contains("HOME/TMPDIR redirected true"));
        assert!(report.contains("Outside writes blocked false"));
        assert!(report.contains("macOS Seatbelt opt-in false"));
    }

    #[test]
    fn sandbox_snapshot_reports_active_seatbelt_write_blocking() {
        let status = runtime::SandboxStatus {
            enabled: true,
            home_tmp_redirected: true,
            filesystem_active: true,
            filesystem_write_blocking_active: true,
            macos_seatbelt_opt_in: true,
            ..runtime::SandboxStatus::default()
        };
        let report = format_sandbox_status_snapshot(&status);

        assert!(report.contains("HOME/TMPDIR redirected true"));
        assert!(report.contains("Outside writes blocked true"));
        assert!(report.contains("macOS Seatbelt opt-in true"));
    }
}
