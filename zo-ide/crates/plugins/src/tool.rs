//! `PluginTool` — a resolved tool entry that can be invoked by the
//! tool dispatcher.
//!
//! Each [`PluginTool`] knows which plugin it belongs to (`plugin_id` /
//! `plugin_name`), its `inputSchema`, the spawned command + args, the
//! required permission level, and the on-disk root (so child
//! processes get a `ZO_PLUGIN_ROOT` env). `execute` runs the
//! external command with the tool input piped on stdin and returns the
//! trimmed stdout on success.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use super::process_runner::run_plugin_process;
use super::{PluginError, PluginToolDefinition, PluginToolPermission};

#[derive(Debug, Clone, PartialEq)]
pub struct PluginTool {
    plugin_id: String,
    plugin_name: String,
    definition: PluginToolDefinition,
    command: String,
    args: Vec<String>,
    required_permission: PluginToolPermission,
    root: Option<PathBuf>,
}

impl PluginTool {
    #[must_use]
    pub fn new(
        plugin_id: impl Into<String>,
        plugin_name: impl Into<String>,
        definition: PluginToolDefinition,
        command: impl Into<String>,
        args: Vec<String>,
        required_permission: PluginToolPermission,
        root: Option<PathBuf>,
    ) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            plugin_name: plugin_name.into(),
            definition,
            command: command.into(),
            args,
            required_permission,
            root,
        }
    }

    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    #[must_use]
    pub fn definition(&self) -> &PluginToolDefinition {
        &self.definition
    }

    #[must_use]
    pub fn required_permission(&self) -> &str {
        self.required_permission.as_str()
    }

    #[must_use]
    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub fn execute(&self, input: &Value) -> Result<String, PluginError> {
        let input_json = input.to_string();
        let mut process = Command::new(&self.command);
        process
            .args(&self.args)
            .env("ZO_PLUGIN_ID", &self.plugin_id)
            .env("ZO_PLUGIN_NAME", &self.plugin_name)
            .env("ZO_TOOL_NAME", &self.definition.name)
            .env("ZO_TOOL_INPUT", &input_json);
        if let Some(root) = &self.root {
            process
                .current_dir(root)
                .env("ZO_PLUGIN_ROOT", root.display().to_string());
        }

        // The shared runner bounds the wall-clock wait and the captured
        // stdout/stderr and kills a hung child, so a plugin tool cannot freeze
        // the agent or exhaust memory.
        let context = format!(
            "plugin tool `{}` from `{}`",
            self.definition.name, self.plugin_id
        );
        let output = run_plugin_process(process, Some(input_json.as_bytes()), &context)?;
        if output.success {
            Ok(output.stdout.trim().to_string())
        } else {
            let stderr = output.stderr.trim().to_string();
            Err(PluginError::CommandFailed(format!(
                "plugin tool `{}` from `{}` failed for `{}`: {}",
                self.definition.name,
                self.plugin_id,
                self.command,
                if stderr.is_empty() {
                    format!("exit status {}", output.status)
                } else {
                    stderr
                }
            )))
        }
    }
}
