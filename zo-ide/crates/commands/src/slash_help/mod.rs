//! Slash command catalogue + query / render / suggest API.
//!
//! [`specs`] hosts the 1,400-line static table plus the
//! `LOW_VALUE_DEFERRED` / `HIDDEN` denylists. The rest of this file
//! (query helpers, help-text renderer, fuzzy suggester) layers on top
//! of those tables.

mod specs;

use core_types::CommandCategory;

use self::specs::{HIDDEN_SLASH_COMMANDS, LOW_VALUE_DEFERRED_SLASH_COMMANDS, SLASH_COMMAND_SPECS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    pub argument_hint: Option<&'static str>,
    pub resume_supported: bool,
    pub category: CommandCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommandMetadata {
    pub availability: &'static str,
    pub side_effect: &'static str,
    pub risk: &'static str,
    pub examples: &'static [&'static str],
    pub related: &'static [&'static str],
}

fn is_public_slash_command(name: &str) -> bool {
    !HIDDEN_SLASH_COMMANDS.contains(&name) && !LOW_VALUE_DEFERRED_SLASH_COMMANDS.contains(&name)
}

#[must_use]
pub fn slash_command_specs() -> &'static [SlashCommandSpec] {
    SLASH_COMMAND_SPECS
}

pub fn public_slash_command_specs_iter() -> impl Iterator<Item = &'static SlashCommandSpec> {
    slash_command_specs()
        .iter()
        .filter(|spec| is_public_slash_command(spec.name))
}

#[must_use]
pub fn public_slash_command_specs() -> Vec<&'static SlashCommandSpec> {
    public_slash_command_specs_iter().collect()
}

pub fn slash_command_names(spec: &SlashCommandSpec) -> impl Iterator<Item = &'static str> + '_ {
    std::iter::once(spec.name).chain(spec.aliases.iter().copied())
}

#[must_use]
pub fn resume_supported_slash_commands() -> Vec<&'static SlashCommandSpec> {
    public_slash_command_specs_iter()
        .filter(|spec| spec.resume_supported)
        .collect()
}

pub(crate) fn find_slash_command_spec(name: &str) -> Option<&'static SlashCommandSpec> {
    public_slash_command_specs_iter().find(|spec| {
        slash_command_names(spec).any(|command_name| command_name.eq_ignore_ascii_case(name))
    })
}

#[must_use]
pub fn slash_command_usage(spec: &SlashCommandSpec) -> String {
    match spec.argument_hint {
        Some(argument_hint) => format!("/{} {argument_hint}", spec.name),
        None => format!("/{}", spec.name),
    }
}

#[must_use]
pub fn slash_command_metadata(name: &str) -> Option<SlashCommandMetadata> {
    find_slash_command_spec(name).map(metadata_for_spec)
}

fn render_slash_command_aliases(spec: &SlashCommandSpec) -> String {
    spec.aliases
        .iter()
        .map(|alias| format!("/{alias}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn slash_command_detail_lines(spec: &SlashCommandSpec) -> Vec<String> {
    let mut lines = vec![format!("/{}", spec.name)];
    let metadata = metadata_for_spec(spec);
    lines.push(format!("  Summary          {}", spec.summary));
    lines.push(format!("  Usage            {}", slash_command_usage(spec)));
    lines.push(format!("  Category         {}", spec.category));
    lines.push(format!("  Availability    {}", metadata.availability));
    lines.push(format!("  Side effect      {}", metadata.side_effect));
    lines.push(format!("  Risk             {}", metadata.risk));
    if !spec.aliases.is_empty() {
        lines.push(format!(
            "  Aliases          {}",
            render_slash_command_aliases(spec)
        ));
    }
    if !metadata.examples.is_empty() {
        lines.push(format!(
            "  Examples         {}",
            metadata.examples.join(", ")
        ));
    }
    if !metadata.related.is_empty() {
        lines.push(format!(
            "  Related          {}",
            metadata
                .related
                .iter()
                .map(|name| format!("/{name}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if spec.resume_supported {
        lines.push("  Resume           Supported with --resume SESSION.jsonl".to_string());
    }
    lines
}

fn metadata_for_spec(spec: &SlashCommandSpec) -> SlashCommandMetadata {
    let override_entry = METADATA_OVERRIDES
        .iter()
        .find(|entry| entry.name == spec.name);
    metadata(
        spec,
        override_entry.map_or_else(|| default_side_effect(spec), |entry| entry.side_effect),
        override_entry.map_or_else(|| default_risk(spec), |entry| entry.risk),
        override_entry.map_or(&[], |entry| entry.examples),
        override_entry.map_or(&[], |entry| entry.related),
    )
}

struct SlashCommandMetadataOverride {
    name: &'static str,
    side_effect: &'static str,
    risk: &'static str,
    examples: &'static [&'static str],
    related: &'static [&'static str],
}

const METADATA_OVERRIDES: &[SlashCommandMetadataOverride] = &[
    SlashCommandMetadataOverride {
        name: "help",
        side_effect: "Reads the command catalogue and renders help text",
        risk: "Low: no workspace or session mutation",
        examples: &["/help", "/help model"],
        related: &["status", "keybindings"],
    },
    SlashCommandMetadataOverride {
        name: "status",
        side_effect: "Reads current session, workspace, and runtime status",
        risk: "Low: read-only status report",
        examples: &["/status"],
        related: &["cost", "usage", "doctor"],
    },
    SlashCommandMetadataOverride {
        name: "model",
        side_effect: "With an argument, rebuilds the live runtime for the selected model",
        risk: "Medium: changes subsequent model/provider behavior",
        examples: &["/model", "/model anthropic-latest"],
        related: &["connect", "login", "usage"],
    },
    SlashCommandMetadataOverride {
        name: "permissions",
        side_effect: "With an argument, changes the active tool permission mode",
        risk: "Medium: can allow or restrict future workspace changes",
        examples: &["/permissions", "/permissions read-only"],
        related: &["tools", "sandbox", "status"],
    },
    SlashCommandMetadataOverride {
        name: "diff",
        side_effect: "Reads git diff for the active workspace",
        risk: "Low: read-only workspace inspection",
        examples: &["/diff"],
        related: &["review", "commit", "rewind"],
    },
    SlashCommandMetadataOverride {
        name: "review",
        side_effect: "Reads current diff and queues a code-reviewer pass",
        risk: "Low: review-only unless the follow-up turn edits files",
        examples: &["/review", "/review staged changes"],
        related: &["diff", "commit", "council"],
    },
    SlashCommandMetadataOverride {
        name: "hunks",
        side_effect: "Accepts ledger entries or reverses selected workspace hunks",
        risk: "Medium: rejecting a hunk atomically rewrites its file after a context check",
        examples: &["/hunks"],
        related: &["diff", "review", "rewind"],
    },
    SlashCommandMetadataOverride {
        name: "commit",
        side_effect: "Stages selected changes and creates a git commit after confirmation",
        risk: "High: mutates git index and repository history",
        examples: &["/commit"],
        related: &["diff", "review", "pr"],
    },
    SlashCommandMetadataOverride {
        name: "ship",
        side_effect: "Runs configured gates, stages captured paths, commits, pushes, and may deploy",
        risk: "High: executes trusted user commands and publishes git history",
        examples: &["/ship release parser fix"],
        related: &["diff", "review", "commit"],
    },
    SlashCommandMetadataOverride {
        name: "pr",
        side_effect: "Drafts or creates pull request text from the current conversation",
        risk: "Medium: may call gh and publish branch context",
        examples: &["/pr", "/pr ready for review"],
        related: &["commit", "review", "issue"],
    },
    SlashCommandMetadataOverride {
        name: "mcp",
        side_effect: "Reads MCP server state; auth/logout subcommands change credentials",
        risk: "Medium: auth subcommands can open or revoke provider access",
        examples: &["/mcp list", "/mcp auth github"],
        related: &["tools", "config", "doctor"],
    },
    SlashCommandMetadataOverride {
        name: "tools",
        side_effect: "Opens runtime tool toggles and persists disabled tools",
        risk: "Medium: changes which tools the model can call next",
        examples: &["/tools"],
        related: &["permissions", "mcp", "status"],
    },
    SlashCommandMetadataOverride {
        name: "reload-context",
        side_effect: "Reloads project instructions, memory, skills, and runtime config",
        risk: "Medium: changes the prompt and available runtime capabilities",
        examples: &["/reload-context"],
        related: &["memory", "skills", "config"],
    },
    SlashCommandMetadataOverride {
        name: "council",
        side_effect: "Queues a multi-candidate comparison workflow",
        risk: "Medium: can spend extra model budget through subagents",
        examples: &["/council choose an API design"],
        related: &["ultraplan", "review", "agents"],
    },
    SlashCommandMetadataOverride {
        name: "distill",
        side_effect: "Queues a skill distillation turn that may write a proposed skill",
        risk: "Medium: may create a proposed file under .zo/skills",
        examples: &["/distill review workflow"],
        related: &["skills", "memory", "review"],
    },
    SlashCommandMetadataOverride {
        name: "rewind",
        side_effect: "Restores guarded file writes to the state before a selected turn",
        risk: "High: can undo local workspace changes",
        examples: &["/rewind", "/rewind 12", "/rewind 12 force"],
        related: &["diff", "review", "status"],
    },
];

const fn metadata(
    spec: &SlashCommandSpec,
    side_effect: &'static str,
    risk: &'static str,
    examples: &'static [&'static str],
    related: &'static [&'static str],
) -> SlashCommandMetadata {
    SlashCommandMetadata {
        availability: if spec.resume_supported {
            "Live session and --resume"
        } else {
            "Live session only"
        },
        side_effect,
        risk,
        examples,
        related,
    }
}

const fn default_side_effect(spec: &SlashCommandSpec) -> &'static str {
    match spec.category {
        CommandCategory::Session => "Reads or updates session state",
        CommandCategory::Workspace => "Reads or updates workspace state",
        CommandCategory::Discovery => "Reads project or runtime discovery state",
        CommandCategory::Analysis => "Queues or runs analysis against current context",
        CommandCategory::Appearance => "Updates local UI preferences",
        CommandCategory::Control => "Controls local CLI/session behavior",
    }
}

const fn default_risk(spec: &SlashCommandSpec) -> &'static str {
    if spec.resume_supported {
        "Low: supported from saved sessions"
    } else {
        "Medium: requires live runtime/session state"
    }
}

#[must_use]
pub fn render_slash_command_help_detail(name: &str) -> Option<String> {
    find_slash_command_spec(name).map(|spec| slash_command_detail_lines(spec).join("\n"))
}

fn format_slash_command_help_line(spec: &SlashCommandSpec) -> String {
    let name = slash_command_usage(spec);
    let alias_suffix = if spec.aliases.is_empty() {
        String::new()
    } else {
        format!(" (aliases: {})", render_slash_command_aliases(spec))
    };
    let resume = if spec.resume_supported {
        " [resume]"
    } else {
        ""
    };
    format!("  {name:<66} {}{alias_suffix}{resume}", spec.summary)
}

#[must_use]
pub fn suggest_slash_commands(input: &str, limit: usize) -> Vec<String> {
    let query = input.trim().trim_start_matches('/').to_ascii_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut suggestions = public_slash_command_specs_iter()
        .filter_map(|spec| {
            let best = slash_command_names(spec)
                .map(str::to_ascii_lowercase)
                .map(|candidate| {
                    let prefix_rank =
                        if candidate.starts_with(&query) || query.starts_with(&candidate) {
                            0
                        } else if candidate.contains(&query) || query.contains(&candidate) {
                            1
                        } else {
                            2
                        };
                    let distance = core_types::text::levenshtein_distance(&candidate, &query);
                    (prefix_rank, distance)
                })
                .min();

            best.and_then(|(prefix_rank, distance)| {
                if prefix_rank <= 1 || distance <= 2 {
                    Some((prefix_rank, distance, spec.name.len(), spec.name))
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();

    suggestions.sort_unstable();
    suggestions
        .into_iter()
        .map(|(_, _, _, name)| format!("/{name}"))
        .take(limit)
        .collect()
}

#[must_use]
pub fn render_slash_command_help() -> String {
    let mut lines = vec![
        "Slash commands".to_string(),
        "  Start here        /status, /diff, /agents, /skills, /commit".to_string(),
        "  [resume]          also works with --resume SESSION.jsonl".to_string(),
        String::new(),
    ];

    // 6개 카테고리 전부 렌더 — Appearance/Control이 빠져 있던 동안 /doctor,
    // /restart 같은 공개 명령이 /help에 아예 안 보이는 발견성 구멍이 있었다
    // (denylist HIDDEN/LOW_VALUE_DEFERRED 필터는 public iter가 계속 담당).
    let categories = [
        CommandCategory::Session,
        CommandCategory::Workspace,
        CommandCategory::Discovery,
        CommandCategory::Analysis,
        CommandCategory::Appearance,
        CommandCategory::Control,
    ];

    for category in categories {
        lines.push(category.display_name().to_string());
        for spec in public_slash_command_specs_iter().filter(|spec| spec.category == category) {
            lines.push(format_slash_command_help_line(spec));
        }
        lines.push(String::new());
    }

    lines
        .into_iter()
        .rev()
        .skip_while(String::is_empty)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}
