mod goal_checks;
mod goal_flags;
mod mcp_command;
mod plugins_agents;
mod prompt_commands;
mod remote_command;
mod slash_commands;
mod slash_help;
mod slash_registry;

use runtime::{compact_session, CompactionConfig, Session};

pub use core_types::CommandCategory;
pub use plugins_agents::{
    handle_agents_slash_command, handle_mcp_slash_command, handle_plugins_slash_command,
    handle_skills_slash_command, PluginsCommandResult, SlashCommandResult,
};
pub use goal_checks::GoalCheck;
pub use remote_command::RemoteAction;
pub use slash_commands::{
    validate_slash_command_input, DeepTierAction, DurationSpec, GoalCommand, GoalOptions,
    LoopCommand, SelfImproveAction, SlashCommand, SlashCommandParseError, WorkspaceRewindAction,
    DEEP_TIER_USAGE, MAX_SESSION_NAME_CHARS,
};
pub use slash_help::{
    public_slash_command_specs, public_slash_command_specs_iter, render_slash_command_help,
    render_slash_command_help_detail, resume_supported_slash_commands, slash_command_metadata,
    slash_command_names, slash_command_specs, slash_command_usage, suggest_slash_commands,
    SlashCommandMetadata, SlashCommandSpec,
};
pub use slash_registry::{
    PluginSlashCommand, PluginSlashCommandSource, SlashCommandEntry, SlashCommandRegistrationError,
    SlashCommandRegistry,
};

// Re-export render_plugins_report as it was public in the original module.
pub use plugins_agents::render_plugins_report;
pub use prompt_commands::{discover_prompt_commands, find_prompt_command, PromptCommandDef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandManifestEntry {
    pub name: String,
    pub source: CommandSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    InternalOnly,
    FeatureGated,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandRegistry {
    entries: Vec<CommandManifestEntry>,
}

impl CommandRegistry {
    #[must_use]
    pub fn new(entries: Vec<CommandManifestEntry>) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &[CommandManifestEntry] {
        &self.entries
    }
}

#[must_use]
pub fn handle_slash_command(
    input: &str,
    session: &Session,
    compaction: CompactionConfig,
) -> Option<SlashCommandResult> {
    let command = match SlashCommand::parse(input) {
        Ok(Some(command)) => command,
        Ok(None) => return None,
        Err(error) => {
            return Some(SlashCommandResult {
                message: error.to_string(),
                session: session.clone(),
            });
        }
    };

    handle_local_slash_command(&command, session, compaction)
}

fn handle_local_slash_command(
    command: &SlashCommand,
    session: &Session,
    compaction: CompactionConfig,
) -> Option<SlashCommandResult> {
    match command {
        SlashCommand::Compact { .. } => {
            let result = compact_session(session, compaction);
            let message = if result.removed_message_count == 0 {
                "Compaction skipped: session is below the compaction threshold.".to_string()
            } else {
                format!(
                    "Compacted {} messages into a resumable system summary.",
                    result.removed_message_count
                )
            };
            Some(SlashCommandResult {
                message,
                session: result.compacted_session,
            })
        }
        SlashCommand::Help => Some(SlashCommandResult {
            message: render_slash_command_help(),
            session: session.clone(),
        }),
        SlashCommand::Unknown { name, .. } => {
            let registry = SlashCommandRegistry::with_builtins();
            Some(SlashCommandResult {
                message: registry.unknown_command_message(name),
                session: session.clone(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
