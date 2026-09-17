//! Path validation + lifecycle-command runner for resolved plugins.
//!
//! Once a plugin has been parsed into its resolved manifest the
//! manager still has to ensure every referenced hook / lifecycle /
//! tool path actually exists on disk before activating the plugin.
//! The three `validate_*_paths` helpers do that. [`run_lifecycle_commands`]
//! is the runtime counterpart: it spawns each `init` / `shutdown`
//! command inside the plugin root, surfacing stderr in the
//! [`PluginError`] payload.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{PluginError, PluginHooks, PluginLifecycle, PluginMetadata, PluginTool};

pub(crate) fn validate_hook_paths(
    root: Option<&Path>,
    hooks: &PluginHooks,
) -> Result<(), PluginError> {
    let Some(root) = root else {
        return Ok(());
    };
    for entry in hooks
        .pre_tool_use
        .iter()
        .chain(hooks.post_tool_use.iter())
        .chain(hooks.post_tool_use_failure.iter())
    {
        validate_command_path(root, entry, "hook")?;
    }
    Ok(())
}

pub(crate) fn validate_lifecycle_paths(
    root: Option<&Path>,
    lifecycle: &PluginLifecycle,
) -> Result<(), PluginError> {
    let Some(root) = root else {
        return Ok(());
    };
    for entry in lifecycle.init.iter().chain(lifecycle.shutdown.iter()) {
        validate_command_path(root, entry, "lifecycle command")?;
    }
    Ok(())
}

pub(crate) fn validate_tool_paths(
    root: Option<&Path>,
    tools: &[PluginTool],
) -> Result<(), PluginError> {
    let Some(root) = root else {
        return Ok(());
    };
    for tool in tools {
        validate_command_path(root, tool.command(), "tool")?;
    }
    Ok(())
}

pub(crate) fn validate_command_path(
    root: &Path,
    entry: &str,
    kind: &str,
) -> Result<(), PluginError> {
    if is_literal_command(entry) {
        return Err(PluginError::InvalidManifest(format!(
            "plugin {kind} `{entry}` must be a script path, not a shell command"
        )));
    }
    // NOTE: `entry` here is already resolved against the plugin root by
    // `resolve_hooks`/`resolve_lifecycle` (i.e. it is typically absolute), so a
    // lexical "reject absolute" check would wrongly reject every legitimate
    // plugin. Lexical rejection of absolute / `..` entries lives one layer up in
    // `manifest_io::validate_command_entry`, which sees the *raw* relative
    // manifest string. This layer enforces the same guarantee on the resolved
    // path via canonical containment.
    let path = if Path::new(entry).is_absolute() {
        PathBuf::from(entry)
    } else {
        root.join(entry)
    };
    if !path.exists() {
        return Err(PluginError::InvalidManifest(format!(
            "{kind} path `{}` does not exist",
            path.display()
        )));
    }
    if !path.is_file() {
        return Err(PluginError::InvalidManifest(format!(
            "{kind} path `{}` must point to a file",
            path.display()
        )));
    }
    // Containment gate: the resolved script must stay under the plugin root once
    // both sides are canonicalized. This rejects a manifest whose entry escaped
    // the root (`../../etc/x`, an absolute path outside the tree) as well as an
    // in-tree symlink that points out of it — all of which would otherwise be
    // executed by the hook runner / `run_lifecycle_commands`.
    match (root.canonicalize(), path.canonicalize()) {
        (Ok(canonical_root), Ok(canonical_path)) => {
            if !canonical_path.starts_with(&canonical_root) {
                return Err(PluginError::InvalidManifest(format!(
                    "{kind} path `{}` resolves outside the plugin root",
                    path.display()
                )));
            }
        }
        _ => {
            return Err(PluginError::InvalidManifest(format!(
                "{kind} path `{}` could not be resolved for containment check",
                path.display()
            )));
        }
    }
    Ok(())
}

pub(crate) fn resolve_hook_entry(root: &Path, entry: &str) -> String {
    if is_literal_command(entry) {
        entry.to_string()
    } else {
        root.join(entry).display().to_string()
    }
}

pub(crate) fn is_literal_command(entry: &str) -> bool {
    !entry.starts_with("./") && !entry.starts_with("../") && !Path::new(entry).is_absolute()
}

pub(crate) fn run_lifecycle_commands(
    metadata: &PluginMetadata,
    lifecycle: &PluginLifecycle,
    phase: &str,
    commands: &[String],
) -> Result<(), PluginError> {
    if lifecycle.is_empty() || commands.is_empty() {
        return Ok(());
    }

    for command in commands {
        let path = Path::new(command);
        if !path.exists() {
            return Err(PluginError::InvalidManifest(format!(
                "plugin lifecycle command path `{}` does not exist",
                path.display()
            )));
        }
        if !path.is_file() {
            return Err(PluginError::InvalidManifest(format!(
                "plugin lifecycle command path `{}` must point to a file",
                path.display()
            )));
        }

        let mut process = if cfg!(windows) {
            let mut process = Command::new("cmd");
            process.arg("/C").arg(command);
            process
        } else {
            let mut process = Command::new("sh");
            process.arg(command);
            process
        };
        if let Some(root) = &metadata.root {
            process
                .current_dir(root)
                .env("ZO_PLUGIN_ROOT", root.display().to_string());
        }
        process
            .env("ZO_PLUGIN_ID", &metadata.id)
            .env("ZO_PLUGIN_NAME", &metadata.name)
            .env("ZO_PLUGIN_LIFECYCLE_PHASE", phase)
            .env("ZO_PLUGIN_LIFECYCLE_COMMAND", command);

        // Bounded wall-clock + output via the shared plugin process runner so a
        // hung or chatty lifecycle script cannot freeze startup/shutdown or
        // exhaust memory.
        let context = format!("plugin `{}` {phase}", metadata.id);
        let output = super::process_runner::run_plugin_process(process, None, &context)?;

        if !output.success {
            let stderr = output.stderr.trim().to_string();
            return Err(PluginError::CommandFailed(format!(
                "plugin `{}` {} failed for `{}`: {}",
                metadata.id,
                phase,
                command,
                if stderr.is_empty() {
                    format!("exit status {}", output.status)
                } else {
                    stderr
                }
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod path_containment_tests {
    use super::validate_command_path;
    use std::fs;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "zo-plugin-path-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        // macOS `temp_dir` (`/var/...`) is a symlink to `/private/var/...`;
        // canonicalize so containment comparisons in the validator line up.
        dir.canonicalize().unwrap()
    }

    #[test]
    fn accepts_script_inside_root() {
        let root = temp_root("inside");
        fs::create_dir_all(root.join("hooks")).unwrap();
        fs::write(root.join("hooks/pre.sh"), "#!/bin/sh\n").unwrap();
        // Script paths use the `./` prefix convention; a bare `hooks/pre.sh`
        // is treated as a literal shell command by `is_literal_command`.
        assert!(validate_command_path(&root, "./hooks/pre.sh", "hook").is_ok());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        let root = temp_root("traversal");
        // An entry that escapes the (canonicalized) root via `..` must be
        // rejected by the containment gate even though the target file exists.
        let err = validate_command_path(&root, "../../../../../../etc/hosts", "hook")
            .expect_err("`..` escape must be rejected");
        assert!(
            format!("{err}").contains("outside the plugin root"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_absolute_path_outside_root() {
        // `entry` reaches this layer already resolved (often absolute), so an
        // absolute path is only rejected when it lands *outside* the plugin
        // root — enforced by canonical containment, not a lexical check.
        let root = temp_root("absolute");
        let err = validate_command_path(&root, "/etc/hosts", "lifecycle command")
            .expect_err("absolute path outside root must be rejected");
        assert!(
            format!("{err}").contains("outside the plugin root"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn accepts_absolute_path_inside_root() {
        // The real call path passes an absolute, root-resolved entry; that must
        // still be accepted.
        let root = temp_root("absolute-inside");
        fs::create_dir_all(root.join("tools")).unwrap();
        fs::write(root.join("tools/echo.sh"), "#!/bin/sh\n").unwrap();
        let absolute = root.join("tools/echo.sh");
        assert!(validate_command_path(&root, absolute.to_str().unwrap(), "tool").is_ok());
        fs::remove_dir_all(&root).ok();
    }
}
