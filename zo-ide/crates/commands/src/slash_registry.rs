//! Dynamic registry of slash commands.
//!
//! Lane B replaces the previous static enum dispatch with a
//! [`SlashCommandRegistry`] that owns both built-in and plugin-contributed
//! top-level commands. The registry is the single source of truth for name
//! lookup, did-you-mean suggestions, and "command not found" errors.
//!
//! The registry is intentionally layered on top of the existing
//! [`validate_slash_command_input`](crate::validate_slash_command_input)
//! parser rather than replacing it: parsing structured arguments for
//! built-ins still lives in `slash_commands.rs`, while plugin-contributed
//! commands are free-form and surface through [`SlashCommandEntry::Plugin`].
//!
//! ## Plugin contribution
//!
//! Plugins can register a top-level slash command via
//! [`SlashCommandRegistry::register`]. The registry rejects names that
//! collide with built-ins so plugins cannot shadow first-party behavior.
//! See the [`PluginSlashCommandSource`] trait for the intended integration
//! point with the `plugins` crate's `PluginCommandManifest`.

use std::collections::BTreeMap;

use plugins::PluginCommandManifest;

use crate::prompt_commands::PromptCommandDef;
use crate::slash_help::{
    public_slash_command_specs_iter, slash_command_names, slash_command_specs,
    suggest_slash_commands, SlashCommandSpec,
};

/// Source of a registered slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandEntry {
    /// First-party command backed by a [`SlashCommandSpec`] in the built-in
    /// table. Parsing and execution go through the existing dispatcher in
    /// `lib.rs` / `slash_commands.rs`.
    Builtin(&'static SlashCommandSpec),
    /// Plugin-contributed command. The registry owns enough metadata to
    /// resolve the name and surface it in help; actual execution is the
    /// plugin runtime's responsibility.
    Plugin(PluginSlashCommand),
    /// Project-local Markdown prompt command. Execution expands the prompt
    /// body and queues/runs it as the next user turn.
    PromptCommand(PromptCommandDef),
}

/// Metadata the registry stores for a plugin-contributed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSlashCommand {
    /// Plugin id that owns the command (stable identifier).
    pub plugin_id: String,
    /// User-visible command name without the leading `/`.
    pub name: String,
    /// One-line summary for help output.
    pub summary: String,
}

impl PluginSlashCommand {
    #[must_use]
    pub fn new(
        plugin_id: impl Into<String>,
        name: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            name: name.into(),
            summary: summary.into(),
        }
    }
}

/// Trait implemented by types that can contribute slash commands to the
/// registry — typically the plugin loader in the `plugins` crate.
///
/// This is deliberately separate from `plugins::Plugin` so that the
/// existing plugin loading path is untouched; integrators only need to
/// iterate their manifests and hand them to the registry.
pub trait PluginSlashCommandSource {
    /// Return the plugin id and the flat list of command manifests this
    /// source contributes.
    fn plugin_slash_commands(&self) -> (String, Vec<PluginCommandManifest>);
}

/// Error returned when a dynamic slash command collides with an existing
/// built-in, plugin, or prompt-command entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandRegistrationError {
    /// Name collides with a first-party built-in command. Plugins cannot
    /// shadow built-ins.
    BuiltinCollision { name: String },
    /// Name collides with an already-registered plugin command.
    DuplicatePluginCommand {
        name: String,
        existing_plugin: String,
    },
    /// Name collides with an already-registered prompt command.
    DuplicatePromptCommand { name: String },
    /// Name is empty or starts with `/` (registry strips the prefix).
    InvalidName { name: String },
}

impl std::fmt::Display for SlashCommandRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BuiltinCollision { name } => write!(
                f,
                "slash command '/{name}' is a built-in and cannot be overridden",
            ),
            Self::DuplicatePluginCommand {
                name,
                existing_plugin,
            } => write!(
                f,
                "slash command '/{name}' is already registered by plugin '{existing_plugin}'",
            ),
            Self::InvalidName { name } => {
                write!(f, "invalid slash command name '{name}'")
            }
            Self::DuplicatePromptCommand { name } => {
                write!(
                    f,
                    "slash command '/{name}' is already registered as a prompt command"
                )
            }
        }
    }
}

impl std::error::Error for SlashCommandRegistrationError {}

/// Registry of all slash commands visible to the REPL.
///
/// Use [`SlashCommandRegistry::with_builtins`] to seed the registry from
/// the static spec table, then feed plugin commands via [`Self::register`]
/// or [`Self::register_from_source`].
#[derive(Debug, Clone, Default)]
pub struct SlashCommandRegistry {
    entries: BTreeMap<String, SlashCommandEntry>,
}

impl SlashCommandRegistry {
    /// Create an empty registry with no entries.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry pre-populated with every public built-in spec
    /// (including the Lane B catalog stubs).
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        // Include *all* specs (public, hidden, and low-value deferred). The
        // registry is the source of truth for name resolution; visibility
        // filters (HIDDEN_SLASH_COMMANDS, LOW_VALUE_DEFERRED_SLASH_COMMANDS)
        // only affect help rendering, not whether a command parses.
        for spec in slash_command_specs() {
            for name in slash_command_names(spec) {
                registry
                    .entries
                    .insert(name.to_string(), SlashCommandEntry::Builtin(spec));
            }
        }
        registry
    }

    /// Iterator over only the specs surfaced in public help.
    pub fn public_builtin_specs() -> impl Iterator<Item = &'static SlashCommandSpec> {
        public_slash_command_specs_iter()
    }

    /// Register a plugin-contributed command. Returns an error if the name
    /// collides with a built-in or another plugin entry.
    pub fn register(
        &mut self,
        command: PluginSlashCommand,
    ) -> Result<(), SlashCommandRegistrationError> {
        let name = command.name.trim().trim_start_matches('/').to_string();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return Err(SlashCommandRegistrationError::InvalidName {
                name: command.name.clone(),
            });
        }

        match self.entries.get(&name) {
            Some(SlashCommandEntry::Builtin(_)) => {
                return Err(SlashCommandRegistrationError::BuiltinCollision { name });
            }
            Some(SlashCommandEntry::Plugin(existing)) => {
                return Err(SlashCommandRegistrationError::DuplicatePluginCommand {
                    name,
                    existing_plugin: existing.plugin_id.clone(),
                });
            }
            Some(SlashCommandEntry::PromptCommand(_)) => {
                return Err(SlashCommandRegistrationError::DuplicatePromptCommand { name });
            }
            None => {}
        }

        let entry = SlashCommandEntry::Plugin(PluginSlashCommand {
            name: name.clone(),
            ..command
        });
        self.entries.insert(name, entry);
        Ok(())
    }

    /// Register a project-local Markdown prompt command. Prompt commands are
    /// dynamic like plugins but cannot shadow built-ins or plugin commands.
    pub fn register_prompt_command(
        &mut self,
        command: PromptCommandDef,
    ) -> Result<(), SlashCommandRegistrationError> {
        let name = command.name.trim().trim_start_matches('/').to_string();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return Err(SlashCommandRegistrationError::InvalidName {
                name: command.name.clone(),
            });
        }

        match self.entries.get(&name) {
            Some(SlashCommandEntry::Builtin(_)) => {
                return Err(SlashCommandRegistrationError::BuiltinCollision { name });
            }
            Some(SlashCommandEntry::Plugin(existing)) => {
                return Err(SlashCommandRegistrationError::DuplicatePluginCommand {
                    name,
                    existing_plugin: existing.plugin_id.clone(),
                });
            }
            Some(SlashCommandEntry::PromptCommand(_)) => {
                return Err(SlashCommandRegistrationError::DuplicatePromptCommand { name });
            }
            None => {}
        }

        let entry = SlashCommandEntry::PromptCommand(PromptCommandDef {
            name: name.clone(),
            ..command
        });
        self.entries.insert(name, entry);
        Ok(())
    }

    /// Register every discovered project prompt command. Errors are
    /// collected and returned together; successful registrations are kept.
    pub fn register_prompt_commands(
        &mut self,
        commands: &[PromptCommandDef],
    ) -> Vec<SlashCommandRegistrationError> {
        let mut errors = Vec::new();
        for command in commands {
            if let Err(err) = self.register_prompt_command(command.clone()) {
                errors.push(err);
            }
        }
        errors
    }

    /// Register every command contributed by a [`PluginSlashCommandSource`].
    /// Errors are collected and returned together; successful registrations
    /// are retained.
    pub fn register_from_source<S: PluginSlashCommandSource>(
        &mut self,
        source: &S,
    ) -> Vec<SlashCommandRegistrationError> {
        let (plugin_id, manifests) = source.plugin_slash_commands();
        let mut errors = Vec::new();
        for manifest in manifests {
            let command = PluginSlashCommand::new(
                plugin_id.clone(),
                manifest.name.clone(),
                manifest.description.clone(),
            );
            if let Err(err) = self.register(command) {
                errors.push(err);
            }
        }
        errors
    }

    /// Look up an entry by name (case-insensitive, leading `/` optional).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&SlashCommandEntry> {
        let key = name.trim().trim_start_matches('/').to_ascii_lowercase();
        // Fast path: exact key.
        if let Some(entry) = self.entries.get(&key) {
            return Some(entry);
        }
        // Fallback: case-insensitive scan. BTreeMap keys are normalized to
        // their as-registered form, so do one linear pass for parity.
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&key))
            .map(|(_, v)| v)
    }

    /// `true` if `name` resolves to any entry.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// All registered names, sorted.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// Total number of registered names (including aliases).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the registry has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Format a "command not found" error with up to three did-you-mean
    /// suggestions drawn from the built-in spec table.
    #[must_use]
    pub fn unknown_command_message(&self, input: &str) -> String {
        let name = input.trim().trim_start_matches('/');
        let suggestions = suggest_slash_commands(name, 3);
        if suggestions.is_empty() {
            format!("Unknown slash command '/{name}'. Use /help to list available slash commands.",)
        } else {
            format!(
                "Unknown slash command '/{name}'. Did you mean: {}?",
                suggestions.join(", ")
            )
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn prompt_command(name: &str) -> PromptCommandDef {
        PromptCommandDef {
            name: name.to_string(),
            description: Some("Prompt command".to_string()),
            argument_hint: Some("<topic>".to_string()),
            model: None,
            effort: None,
            body: "Do $ARGUMENTS".to_string(),
            allowed_tools: Vec::new(),
            path: PathBuf::from(format!(".zo/commands/{name}.md")),
        }
    }

    #[test]
    fn builtin_registry_contains_core_commands() {
        let registry = SlashCommandRegistry::with_builtins();
        assert!(registry.contains("help"));
        assert!(registry.contains("/help"));
        assert!(registry.contains("HELP"));
        assert!(registry.contains("commit"));
    }

    #[test]
    fn plugin_registration_rejects_builtin_collision() {
        let mut registry = SlashCommandRegistry::with_builtins();
        let err = registry
            .register(PluginSlashCommand::new("demo", "help", "Demo"))
            .expect_err("registering /help should fail");
        assert!(matches!(
            err,
            SlashCommandRegistrationError::BuiltinCollision { .. }
        ));
    }

    #[test]
    fn plugin_registration_succeeds_for_new_command() {
        let mut registry = SlashCommandRegistry::with_builtins();
        registry
            .register(PluginSlashCommand::new(
                "demo",
                "foo",
                "Plugin-contributed foo",
            ))
            .expect("new plugin command should register");
        assert!(registry.contains("foo"));
        match registry.get("foo") {
            Some(SlashCommandEntry::Plugin(plugin)) => {
                assert_eq!(plugin.plugin_id, "demo");
            }
            other => panic!("expected plugin entry, got {other:?}"),
        }
    }

    #[test]
    fn plugin_registration_rejects_duplicate_plugin_commands() {
        let mut registry = SlashCommandRegistry::with_builtins();
        registry
            .register(PluginSlashCommand::new("alpha", "foo", "first"))
            .unwrap();
        let err = registry
            .register(PluginSlashCommand::new("beta", "foo", "second"))
            .expect_err("duplicate should fail");
        assert!(matches!(
            err,
            SlashCommandRegistrationError::DuplicatePluginCommand { .. }
        ));
    }

    #[test]
    fn prompt_command_registration_succeeds_for_project_command() {
        let mut registry = SlashCommandRegistry::with_builtins();
        let command = prompt_command("review-local");
        let errors = registry.register_prompt_commands(std::slice::from_ref(&command));

        assert!(
            errors.is_empty(),
            "unexpected registration errors: {errors:?}"
        );
        assert!(registry.contains("review-local"));
        match registry.get("/review-local") {
            Some(SlashCommandEntry::PromptCommand(registered)) => {
                assert_eq!(registered.name, command.name);
                assert_eq!(registered.argument_hint, command.argument_hint);
            }
            other => panic!("expected prompt command entry, got {other:?}"),
        }
    }

    #[test]
    fn prompt_command_registration_rejects_builtin_collision() {
        let mut registry = SlashCommandRegistry::with_builtins();
        let errors = registry.register_prompt_commands(&[prompt_command("help")]);

        assert!(matches!(
            errors.as_slice(),
            [SlashCommandRegistrationError::BuiltinCollision { name }] if name == "help"
        ));
    }

    #[test]
    fn unknown_command_message_includes_suggestions() {
        let registry = SlashCommandRegistry::with_builtins();
        let msg = registry.unknown_command_message("/comit");
        assert!(msg.contains("/commit"), "expected did-you-mean: {msg}");
    }
}
