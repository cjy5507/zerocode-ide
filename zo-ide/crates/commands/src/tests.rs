use super::{
    handle_plugins_slash_command, handle_slash_command, render_plugins_report,
    render_slash_command_help, render_slash_command_help_detail, resume_supported_slash_commands,
    slash_command_metadata, slash_command_specs, suggest_slash_commands,
    validate_slash_command_input, DeepTierAction, DurationSpec, GoalCommand, GoalOptions,
    LoopCommand, SlashCommand, WorkspaceRewindAction,
};
use crate::plugins_agents::{
    discover_definition_roots, discover_skill_roots_from, load_agents_from_roots,
    load_skills_from_roots, render_agents_report, render_skills_report, DefinitionSource,
    SkillOrigin, SkillRoot,
};
use plugins::{PluginKind, PluginManager, PluginManagerConfig, PluginMetadata, PluginSummary};
use runtime::{
    CompactionConfig, ConfigLoader, ContentBlock, ConversationMessage, MessageRole, Session,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("commands-plugin-{label}-{nanos}"))
}

fn write_external_plugin(root: &Path, name: &str, version: &str) {
    fs::create_dir_all(root.join(".claude-plugin")).expect("manifest dir");
    fs::write(
        root.join(".claude-plugin").join("plugin.json"),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"commands plugin\"\n}}"
        ),
    )
    .expect("write manifest");
}

fn write_bundled_plugin(root: &Path, name: &str, version: &str, default_enabled: bool) {
    fs::create_dir_all(root.join(".claude-plugin")).expect("manifest dir");
    fs::write(
        root.join(".claude-plugin").join("plugin.json"),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"description\": \"bundled commands plugin\",\n  \"defaultEnabled\": {}\n}}",
            if default_enabled { "true" } else { "false" }
        ),
    )
    .expect("write bundled manifest");
}

fn write_agent(root: &Path, name: &str, description: &str, model: &str, reasoning: &str) {
    fs::create_dir_all(root).expect("agent root");
    fs::write(
        root.join(format!("{name}.toml")),
        format!(
            "name = \"{name}\"\ndescription = \"{description}\"\nmodel = \"{model}\"\nmodel_reasoning_effort = \"{reasoning}\"\n"
        ),
    )
    .expect("write agent");
}

fn write_markdown_agent(
    root: &Path,
    name: &str,
    description: &str,
    model: &str,
    reasoning: &str,
) {
    fs::create_dir_all(root).expect("agent root");
    fs::write(
        root.join(format!("{name}.md")),
        format!(
            "---\nname: Display {name}\ndescription: {description}\ntools: read_file, grep_search\nmodel: {model}\nreasoningEffort: {reasoning}\npermissionMode: read-only\n---\n\n# {name}\n"
        ),
    )
    .expect("write markdown agent");
}

fn write_skill(root: &Path, name: &str, description: &str) {
    let skill_root = root.join(name);
    fs::create_dir_all(&skill_root).expect("skill root");
    fs::write(
        skill_root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )
    .expect("write skill");
}

fn write_legacy_command(root: &Path, name: &str, description: &str) {
    fs::create_dir_all(root).expect("commands root");
    fs::write(
        root.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )
    .expect("write command");
}

fn parse_error_message(input: &str) -> String {
    SlashCommand::parse(input)
        .expect_err("slash command should be rejected")
        .to_string()
}

#[allow(clippy::too_many_lines)]
#[test]
fn parses_supported_slash_commands() {
    assert_eq!(SlashCommand::parse("/help"), Ok(Some(SlashCommand::Help)));
    assert_eq!(SlashCommand::parse("/audit"), Ok(Some(SlashCommand::Audit)));
    assert_eq!(
        SlashCommand::parse(" /status "),
        Ok(Some(SlashCommand::Status))
    );
    // `/stats` and `/summary` fold into the `/status` card; `/sandbox` into
    // `/permissions`.
    assert_eq!(SlashCommand::parse("/stats"), Ok(Some(SlashCommand::Status)));
    assert_eq!(
        SlashCommand::parse("/summary"),
        Ok(Some(SlashCommand::Status))
    );
    assert_eq!(
        SlashCommand::parse("/sandbox"),
        Ok(Some(SlashCommand::Permissions { mode: None }))
    );
    assert_eq!(
        SlashCommand::parse("/bughunter runtime"),
        Ok(Some(SlashCommand::Bughunter {
            scope: Some("runtime".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/commit"),
        Ok(Some(SlashCommand::Commit))
    );
    assert_eq!(
        SlashCommand::parse("/pr ready for review"),
        Ok(Some(SlashCommand::Pr {
            context: Some("ready for review".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/issue flaky test"),
        Ok(Some(SlashCommand::Issue {
            context: Some("flaky test".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/ultraplan ship both features"),
        Ok(Some(SlashCommand::Ultraplan {
            task: Some("ship both features".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/council choose an API design"),
        Ok(Some(SlashCommand::Council {
            task: Some("choose an API design".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/distill review loop"),
        Ok(Some(SlashCommand::Distill {
            topic: Some("review loop".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/teleport conversation.rs"),
        Ok(Some(SlashCommand::Teleport {
            target: Some("conversation.rs".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/debug-tool-call"),
        Ok(Some(SlashCommand::DebugToolCall))
    );
    assert_eq!(
        SlashCommand::parse("/inbox"),
        Ok(Some(SlashCommand::Inbox { args: None }))
    );
    assert_eq!(
        SlashCommand::parse("/teaminbox ack u1"),
        Ok(Some(SlashCommand::Inbox {
            args: Some("ack u1".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/bughunter runtime"),
        Ok(Some(SlashCommand::Bughunter {
            scope: Some("runtime".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/commit"),
        Ok(Some(SlashCommand::Commit))
    );
    assert_eq!(
        SlashCommand::parse("/pr ready for review"),
        Ok(Some(SlashCommand::Pr {
            context: Some("ready for review".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/issue flaky test"),
        Ok(Some(SlashCommand::Issue {
            context: Some("flaky test".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/ultraplan ship both features"),
        Ok(Some(SlashCommand::Ultraplan {
            task: Some("ship both features".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/council"),
        Ok(Some(SlashCommand::Council { task: None }))
    );
    assert_eq!(
        SlashCommand::parse("/distill"),
        Ok(Some(SlashCommand::Distill { topic: None }))
    );
    assert_eq!(
        SlashCommand::parse("/goal ship the parser refactor"),
        Ok(Some(SlashCommand::Goal {
            command: GoalCommand::Start {
                goal: "ship the parser refactor".to_string(),
                options: GoalOptions::default(),
            }
        }))
    );
    assert_eq!(
        SlashCommand::parse("/goal"),
        Ok(Some(SlashCommand::Goal {
            command: GoalCommand::Status
        }))
    );
    assert_eq!(
        SlashCommand::parse(
            "/goal \"fix clippy\" --check cargo:clippy --check cargo:test --max-turns 4"
        ),
        Ok(Some(SlashCommand::Goal {
            command: GoalCommand::Start {
                goal: "fix clippy".to_string(),
                options: GoalOptions {
                    checks: vec!["cargo:clippy".to_string(), "cargo:test".to_string()],
                    max_turns: Some(4),
                    token_budget: None,
                    allow_writes: false,
                    no_plan: false,
                    until: None,
                    every: None,
                },
            }
        }))
    );
    assert_eq!(
        SlashCommand::parse("/loop 3 \"check CI status\""),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartFixedCount {
                count: 3,
                prompt: "check CI status".to_string(),
            }
        }))
    );
    assert_eq!(
        SlashCommand::parse("/loop every 5m \"poll tests\""),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartInterval {
                every: DurationSpec {
                    raw: "5m".to_string(),
                    duration: Duration::from_secs(300),
                },
                prompt: "poll tests".to_string(),
            }
        }))
    );
    assert_eq!(
        SlashCommand::parse("/loop watch 'crates/**/*.rs' \"rerun checks\""),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartWatch {
                glob: "crates/**/*.rs".to_string(),
                prompt: "rerun checks".to_string(),
            }
        }))
    );
    assert_eq!(
        SlashCommand::parse("/teleport conversation.rs"),
        Ok(Some(SlashCommand::Teleport {
            target: Some("conversation.rs".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/debug-tool-call"),
        Ok(Some(SlashCommand::DebugToolCall))
    );
    assert_eq!(
        SlashCommand::parse("/reload-context"),
        Ok(Some(SlashCommand::ReloadContext))
    );
    assert!(SlashCommand::parse("/reload-context now").is_err());
    assert_eq!(
        SlashCommand::parse("/model claude-opus"),
        Ok(Some(SlashCommand::Model {
            model: Some("claude-opus".to_string()),
        }))
    );
    assert_eq!(
        SlashCommand::parse("/model"),
        Ok(Some(SlashCommand::Model { model: None }))
    );
    assert_eq!(
        SlashCommand::parse("/permissions read-only"),
        Ok(Some(SlashCommand::Permissions {
            mode: Some("read-only".to_string()),
        }))
    );
    assert_eq!(
        SlashCommand::parse("/clear"),
        Ok(Some(SlashCommand::Clear { confirm: false }))
    );
    assert_eq!(
        SlashCommand::parse("/clear --confirm"),
        Ok(Some(SlashCommand::Clear { confirm: true }))
    );
    assert_eq!(SlashCommand::parse("/cost"), Ok(Some(SlashCommand::Cost)));
    assert_eq!(
        SlashCommand::parse("/usage"),
        Ok(Some(SlashCommand::Usage { scope: None }))
    );
    assert!(SlashCommand::parse("/usage day").is_err());
    assert_eq!(
        SlashCommand::parse("/resume session.json"),
        Ok(Some(SlashCommand::Resume {
            session_path: Some("session.json".to_string()),
        }))
    );
    assert_eq!(
        SlashCommand::parse("/config"),
        Ok(Some(SlashCommand::Config { section: None }))
    );
    assert_eq!(
        SlashCommand::parse("/config env"),
        Ok(Some(SlashCommand::Config {
            section: Some("env".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/mcp"),
        Ok(Some(SlashCommand::Mcp {
            action: None,
            target: None
        }))
    );
    assert_eq!(
        SlashCommand::parse("/mcp show remote"),
        Ok(Some(SlashCommand::Mcp {
            action: Some("show".to_string()),
            target: Some("remote".to_string())
        }))
    );
    assert_eq!(SlashCommand::parse("/tools"), Ok(Some(SlashCommand::Tools)));
    assert!(SlashCommand::parse("/tools now").is_err());
    assert_eq!(
        SlashCommand::parse("/memory"),
        Ok(Some(SlashCommand::Memory))
    );
    assert_eq!(SlashCommand::parse("/init"), Ok(Some(SlashCommand::Init)));
    assert_eq!(SlashCommand::parse("/diff"), Ok(Some(SlashCommand::Diff)));
    assert_eq!(
        SlashCommand::parse("/version"),
        Ok(Some(SlashCommand::Version))
    );
    assert_eq!(
        SlashCommand::parse("/export notes.txt"),
        Ok(Some(SlashCommand::Export {
            path: Some("notes.txt".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/dump"),
        Ok(Some(SlashCommand::Dump { edit: false }))
    );
    assert_eq!(
        SlashCommand::parse("/dump edit"),
        Ok(Some(SlashCommand::Dump { edit: true }))
    );
    // Anything other than a bare `/dump` or `/dump edit` is a usage error,
    // not a silent fallback to the pager.
    assert!(SlashCommand::parse("/dump nonsense").is_err());
    assert_eq!(
        SlashCommand::parse("/session switch abc123"),
        Ok(Some(SlashCommand::Session {
            action: Some("switch".to_string()),
            target: Some("abc123".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/plugins install demo"),
        Ok(Some(SlashCommand::Plugins {
            action: Some("install".to_string()),
            target: Some("demo".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/plugins list"),
        Ok(Some(SlashCommand::Plugins {
            action: Some("list".to_string()),
            target: None
        }))
    );
    assert_eq!(
        SlashCommand::parse("/plugins enable demo"),
        Ok(Some(SlashCommand::Plugins {
            action: Some("enable".to_string()),
            target: Some("demo".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/skills install ./fixtures/help-skill"),
        Ok(Some(SlashCommand::Skills {
            args: Some("install ./fixtures/help-skill".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/plugins disable demo"),
        Ok(Some(SlashCommand::Plugins {
            action: Some("disable".to_string()),
            target: Some("demo".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/session fork incident-review"),
        Ok(Some(SlashCommand::Session {
            action: Some("fork".to_string()),
            target: Some("incident-review".to_string())
        }))
    );
}

#[test]
fn goal_rejects_unknown_typed_check_at_parse_time() {
    // A `cargo:`/`git:` prefix with a typo must fail fast, not silently become a
    // free-text model rubric (which would strip objective validation from the goal).
    assert!(
        SlashCommand::parse("/goal fix it --check cargo:tests").is_err(),
        "cargo:tests typo must be rejected"
    );
    assert!(
        SlashCommand::parse("/goal fix it --check git:status").is_err(),
        "unknown git: check must be rejected"
    );
    // Known typed checks and free-text rubric labels still parse.
    assert!(SlashCommand::parse("/goal fix it --check cargo:test").is_ok());
    assert!(SlashCommand::parse("/goal fix it --check grep:TODO").is_ok());
    assert!(
        SlashCommand::parse("/goal fix it --check looks-correct").is_ok(),
        "free-text rubric label must still be allowed"
    );
}

#[test]
fn loop_and_goal_prompts_preserve_free_text() {
    // Apostrophes must not be shell-parsed into an "unclosed quote" usage error
    // (the original defect): a loop prompt is taken verbatim.
    assert_eq!(
        SlashCommand::parse("/loop 3 don't stop until tests pass"),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartFixedCount {
                count: 3,
                prompt: "don't stop until tests pass".to_string(),
            }
        }))
    );
    // Unquoted multi-word prompts work without any quoting.
    assert_eq!(
        SlashCommand::parse("/loop 5 fix the failing test"),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartFixedCount {
                count: 5,
                prompt: "fix the failing test".to_string(),
            }
        }))
    );
    // Internal spacing and embedded double quotes are preserved verbatim.
    assert_eq!(
        SlashCommand::parse(r#"/loop 2 say "hi"  now"#),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartFixedCount {
                count: 2,
                prompt: r#"say "hi"  now"#.to_string(),
            }
        }))
    );
    // A single wrapping pair of quotes is stripped for ergonomics.
    assert_eq!(
        SlashCommand::parse("/loop 3 \"check CI status\""),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartFixedCount {
                count: 3,
                prompt: "check CI status".to_string(),
            }
        }))
    );
    // every/watch keep their structured first token; the prompt stays free text.
    assert_eq!(
        SlashCommand::parse("/loop every 10m don't quit early"),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartInterval {
                every: DurationSpec {
                    raw: "10m".to_string(),
                    duration: Duration::from_secs(600),
                },
                prompt: "don't quit early".to_string(),
            }
        }))
    );
    assert_eq!(
        SlashCommand::parse("/loop watch src/*.rs don't break it"),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartWatch {
                glob: "src/*.rs".to_string(),
                prompt: "don't break it".to_string(),
            }
        }))
    );
    // A quoted glob with inner spaces is still honored.
    assert_eq!(
        SlashCommand::parse("/loop watch 'crates/**/*.rs' \"rerun checks\""),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartWatch {
                glob: "crates/**/*.rs".to_string(),
                prompt: "rerun checks".to_string(),
            }
        }))
    );
    // A goal prompt with an apostrophe must parse (goal shared the same defect).
    assert_eq!(
        SlashCommand::parse("/goal fix the bug it's clearly broken"),
        Ok(Some(SlashCommand::Goal {
            command: GoalCommand::Start {
                goal: "fix the bug it's clearly broken".to_string(),
                options: GoalOptions::default(),
            }
        }))
    );
    // A bare prompt starts dynamic scheduling after its immediate first turn.
    assert_eq!(
        SlashCommand::parse("/loop just keep going"),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartDynamic {
                prompt: "just keep going".to_string(),
            }
        }))
    );
    // A bare prompt wrapped in one quote pair is unquoted and stays dynamic.
    assert_eq!(
        SlashCommand::parse("/loop \"do the thing\""),
        Ok(Some(SlashCommand::Loop {
            command: LoopCommand::StartDynamic {
                prompt: "do the thing".to_string(),
            }
        }))
    );
}

#[test]
fn rejects_unexpected_arguments_for_no_arg_commands() {
    // /help takes no arguments, so trailing text is a usage error.
    let error = parse_error_message("/help now");
    assert!(error.contains("Unexpected arguments for /help."));
    assert!(error.contains("  Usage            /help"));
}

#[test]
fn compact_accepts_optional_focus_instructions() {
    // CC parity: /compact now carries an optional focus directive instead of
    // rejecting arguments.
    assert_eq!(
        SlashCommand::parse("/compact"),
        Ok(Some(SlashCommand::Compact { instructions: None }))
    );
    assert_eq!(
        SlashCommand::parse("/compact focus on auth"),
        Ok(Some(SlashCommand::Compact {
            instructions: Some("focus on auth".to_string())
        }))
    );
}

#[test]
fn rejects_invalid_argument_values() {
    // given
    let input = "/permissions admin";

    // when
    let error = parse_error_message(input);

    // then
    assert!(error.contains(
        "Unsupported /permissions mode 'admin'. Use read-only, workspace-write, or danger-full-access."
    ));
    assert!(error.contains(
        "  Usage            /permissions [read-only|workspace-write|danger-full-access]"
    ));
}

#[test]
fn rejects_missing_required_arguments() {
    // given
    let input = "/teleport";

    // when
    let error = parse_error_message(input);

    // then
    assert!(error.contains("Usage: /teleport <symbol-or-path>"));
    assert!(error.contains("  Category         Discovery & debugging"));
}

#[test]
fn rejects_invalid_session_and_plugin_shapes() {
    // given
    let session_input = "/session switch";
    let plugin_input = "/plugins list extra";

    // when
    let session_error = parse_error_message(session_input);
    let plugin_error = parse_error_message(plugin_input);

    // then
    assert!(session_error.contains("Usage: /session switch <session-id>"));
    assert!(session_error.contains("/session"));
    assert!(plugin_error.contains("Usage: /plugin list"));
    assert!(plugin_error.contains("Aliases          /plugins, /marketplace"));
}

#[test]
fn rejects_invalid_agents_and_skills_arguments() {
    // given
    let agents_input = "/agents show planner";
    let skills_input = "/skills show help";

    // when
    let agents_error = parse_error_message(agents_input);
    let skills_error = parse_error_message(skills_input);

    // then
    assert!(agents_error.contains(
        "Unexpected arguments for /agents: show planner. Use /agents, /agents list, or /agents help."
    ));
    assert!(agents_error.contains("  Usage            /agents [list|help]"));
    assert!(skills_error.contains(
        "Unexpected arguments for /skills: show help. Use /skills, /skills list, /skills install <path>, or /skills help."
    ));
    assert!(skills_error.contains("  Usage            /skills [list|install <path>|help]"));
}

#[test]
fn rejects_invalid_mcp_arguments() {
    let show_error = parse_error_message("/mcp show alpha beta");
    assert!(show_error.contains("Unexpected arguments after /mcp show."));
    assert!(show_error.contains("  Usage            /mcp show <server>"));

    let action_error = parse_error_message("/mcp inspect alpha");
    assert!(action_error.contains("Unknown /mcp action 'inspect'."));
    assert!(action_error.contains(
        "  Usage            /mcp [list|show <server>|auth [list|<server>]|logout <server>|help]"
    ));

    let logout_error = parse_error_message("/mcp logout");
    assert!(logout_error.contains("/mcp logout needs a <server> argument."));
    assert!(logout_error.contains("  Usage            /mcp logout <server>"));
}

#[test]
fn parses_mcp_auth_and_logout_forms() {
    assert_eq!(
        SlashCommand::parse("/mcp auth list"),
        Ok(Some(SlashCommand::Mcp {
            action: Some("auth".to_string()),
            target: Some("list".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/mcp auth demo"),
        Ok(Some(SlashCommand::Mcp {
            action: Some("auth".to_string()),
            target: Some("demo".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/mcp logout demo"),
        Ok(Some(SlashCommand::Mcp {
            action: Some("logout".to_string()),
            target: Some("demo".to_string())
        }))
    );
}

#[test]
fn parses_smart_command_with_optional_action() {
    assert_eq!(
        SlashCommand::parse("/smart")
            .expect("parse should succeed")
            .expect("command expected"),
        SlashCommand::Smart { arg: None }
    );
    assert_eq!(
        SlashCommand::parse("/smart status")
            .expect("parse should succeed")
            .expect("command expected"),
        SlashCommand::Smart {
            arg: Some("status".to_string())
        }
    );
}

#[test]
fn parses_deep_tier_management_commands() {
    let parse = |input| {
        SlashCommand::parse(input)
            .expect("parse should succeed")
            .expect("command expected")
    };
    assert_eq!(
        parse("/tier"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Show
        }
    );
    assert_eq!(
        parse("/tier add opus-5.0"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Add {
                model: "opus-5.0".to_string()
            }
        }
    );
    assert_eq!(
        parse("/tier remove 2"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Remove {
                target: "2".to_string()
            }
        }
    );
    assert_eq!(
        parse("/tier remove claude-opus-5"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Remove {
                target: "claude-opus-5".to_string()
            }
        }
    );
    assert_eq!(
        parse("/tier set 2 claude-opus-5"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Set {
                target: "2".to_string(),
                model: "claude-opus-5".to_string()
            }
        }
    );
    assert_eq!(
        parse("/tier set gpt-5.6-sol claude-opus-5"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Set {
                target: "gpt-5.6-sol".to_string(),
                model: "claude-opus-5".to_string()
            }
        }
    );
    assert_eq!(
        parse("/tier move 3 1"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Move { from: 3, to: 1 }
        }
    );
    assert_eq!(
        parse("/tier reset"),
        SlashCommand::DeepTier {
            action: DeepTierAction::Reset
        }
    );

    for input in ["/tier replace opus-5", "/tier move 0 1", "/tier move one 2"] {
        let error = SlashCommand::parse(input).expect_err("invalid move should be a usage error");
        assert_eq!(
            error.to_string(),
            crate::DEEP_TIER_USAGE
        );
    }
}

/// The palette's argument hint is the only place most users learn what `/tier`
/// can do, so an action the parser accepts but the hint omits is a feature
/// nobody finds — `move` shipped that way. Pinned against the parser itself,
/// not against a second hand-written list.
#[test]
fn tier_argument_hint_lists_every_parsed_action() {
    let hint = crate::slash_help::slash_command_specs()
        .iter()
        .find(|spec| spec.name == "tier")
        .expect("/tier has a spec")
        .argument_hint
        .expect("/tier takes arguments");

    for (action, sample) in [
        ("add", "/tier add claude-opus-5"),
        ("set", "/tier set 2 claude-opus-5"),
        ("remove", "/tier remove 2"),
        ("move", "/tier move 2 1"),
        ("reset", "/tier reset"),
    ] {
        assert!(
            SlashCommand::parse(sample).is_ok_and(|parsed| parsed.is_some()),
            "parser should accept {sample:?}"
        );
        assert!(
            hint.contains(action),
            "argument hint {hint:?} omits the parsed action {action:?}"
        );
        assert!(
            crate::DEEP_TIER_USAGE.contains(action),
            "usage {:?} omits the parsed action {action:?}",
            crate::DEEP_TIER_USAGE
        );
    }
}

#[test]
fn parses_workspace_rewind_commands() {
    let parse = |input| {
        SlashCommand::parse(input)
            .expect("parse should succeed")
            .expect("command expected")
    };
    assert_eq!(
        parse("/rewind"),
        SlashCommand::Rewind {
            action: WorkspaceRewindAction::List
        }
    );
    assert_eq!(
        parse("/rewind 12"),
        SlashCommand::Rewind {
            action: WorkspaceRewindAction::Restore {
                turn_index: 12,
                force: false,
            }
        }
    );
    assert_eq!(
        parse("/rewind 12 force"),
        SlashCommand::Rewind {
            action: WorkspaceRewindAction::Restore {
                turn_index: 12,
                force: true,
            }
        }
    );
    for invalid in [
        "/rewind 0",
        "/rewind nope",
        "/rewind 12 forced",
        "/rewind 12 force extra",
    ] {
        let error = SlashCommand::parse(invalid).expect_err("invalid rewind must fail");
        assert_eq!(error.to_string(), "Usage: /rewind [<turn> [force]]");
    }
}

#[test]
fn registers_ship_with_required_verbatim_commit_message() {
    assert_eq!(
        SlashCommand::parse(r#"/ship release candidate with "quotes""#),
        Ok(Some(SlashCommand::Ship {
            message: r#"release candidate with "quotes""#.to_string()
        }))
    );
    let error = SlashCommand::parse("/ship")
        .expect_err("ship requires a commit message")
        .to_string();
    assert!(error.contains("  Usage            /ship <commit-message>"));

    let spec = crate::slash_help::find_slash_command_spec("ship").expect("ship registered");
    assert_eq!(spec.category, crate::CommandCategory::Workspace);
    assert_eq!(spec.argument_hint, Some("<commit-message>"));
    assert_eq!(spec.summary, "Run gates, commit captured changes, and push");
    assert!(render_slash_command_help().contains("/ship <commit-message>"));
}

#[test]
fn registers_hunks_without_changing_review() {
    assert_eq!(SlashCommand::parse("/hunks"), Ok(Some(SlashCommand::Hunks)));
    let error = SlashCommand::parse("/hunks extra")
        .expect_err("hunks must reject arguments")
        .to_string();
    assert!(error.contains("Unexpected arguments for /hunks."));
    assert!(error.contains("  Usage            /hunks"));
    assert_eq!(
        SlashCommand::parse("/review"),
        Ok(Some(SlashCommand::Review { scope: None }))
    );
    assert_eq!(
        SlashCommand::parse("/review staged changes"),
        Ok(Some(SlashCommand::Review {
            scope: Some("staged changes".to_string())
        }))
    );
    let spec = crate::slash_help::find_slash_command_spec("hunks").expect("hunks registered");
    assert_eq!(spec.category, crate::CommandCategory::Analysis);
    assert_eq!(spec.argument_hint, None);
    assert_eq!(
        spec.summary,
        "Review hunk attribution and accept/reject changes"
    );
}

#[test]
fn renders_help_from_shared_specs() {
    let help = render_slash_command_help();
    assert!(help.contains("Start here        /status, /diff, /agents, /skills, /commit"));
    assert!(help.contains("[resume]          also works with --resume SESSION.jsonl"));
    assert!(help.contains("Session & visibility"));
    assert!(help.contains("Workspace & git"));
    assert!(help.contains("Discovery & debugging"));
    assert!(help.contains("Analysis & automation"));
    assert!(help.contains("/help"));
    assert!(help.contains("/status"));
    assert!(help.contains("/sandbox"));
    assert!(help.contains("/compact"));
    assert!(help.contains("/bughunter [scope]"));
    assert!(help.contains("/commit"));
    assert!(help.contains("/pr [context]"));
    assert!(help.contains("/issue [context]"));
    assert!(help.contains("/ultraplan [task]"));
    assert!(help.contains("/council [task]"));
    assert!(help.contains("/distill [topic]"));
    assert!(help.contains("/teleport <symbol-or-path>"));
    assert!(help.contains("/debug-tool-call"));
    assert!(help.contains("/model [model]"));
    assert!(help.contains("/permissions [read-only|workspace-write|danger-full-access]"));
    assert!(help.contains("/clear [--confirm]"));
    assert!(help.contains("/cost"));
    assert!(help.contains("/resume <session-path>"));
    assert!(help.contains("/config [env|hooks|model|plugins]"));
    assert!(help.contains("/mcp [list|show <server>|auth [list|<server>]|logout <server>|help]"));
    assert!(help.contains("/memory"));
    assert!(help.contains("/init"));
    assert!(help.contains("/diff"));
    assert!(help.contains("/version"));
    assert!(help.contains("/export [file]"));
    assert!(help.contains("/session [list|switch <session-id>|fork [branch-name]]"));
    assert!(help.contains("/name [name]"));
    assert!(help.contains("/sandbox"));
    assert!(help.contains(
        "/plugin [list|install <path>|enable <name>|disable <name>|uninstall <id>|update <id>]"
    ));
    assert!(help.contains("aliases: /plugins, /marketplace"));
    assert!(help.contains("/agents [list|help]"));
    assert!(help.contains("/inbox"));
    assert!(help.contains("aliases: /teaminbox"));
    assert!(help.contains("/skills [list|install <path>|help]"));
    assert!(!help.contains("/rename <name>"));
    assert!(!help.contains("/desktop"));
    // Lane B added 23 catalog-parity stubs to the spec table.
    // +1 for the /goal session command.
    // +1 for the /reload-context command.
    // +1 for the /council orchestration command.
    // +1 for the /distill skill draft command.
    // +1 for the /tools runtime toggle modal.
    // +1 for the /audit tool-ledger view.
    // +1 for the /deep deep-lane gate toggle.
    // +1 for the /auto reactive auto-verify toggle.
    // +1 for the /loop session-local scheduler command.
    // +1 for the /dream between-sessions memory curation command.
    // +1 for the /smart Smart Model Router command.
    // +1 for the /dump transcript pager/editor escape hatch.
    // +1 for the /inbox TeamInbox viewer/report.
    // +1 for the /hunks attribution review modal.
    assert!(help.contains("/dream"));
    assert!(help.contains(
        "/smart [status|agents|doctor|on|off|pin|auto|reset|explore|learned|feedback|diversity|providers]"
    ));
    assert!(help.contains("/tier [add <model>|set <N> <model>|remove <model|N>|move <N> <M>|reset]"));
    assert!(help.contains("/dump [edit]"));
    // +1 for the /restart re-exec command (Control category).
    assert!(help.contains("/restart"));
    assert!(help.contains("/remote [start|status|qr|rotate|stop|approve <code>|deny <code>]"));
    // -23 in 2026-07: the never-implemented catalog stubs were removed
    // (/voice /color /tag /stickers + 16 Lane-B leftovers), /stats and
    // /summary folded into /status, and /sandbox into /permissions. +1 for
    // /providers, the register/edit/delete manager split out of /connect.
    assert!(help.contains("/providers"));
    // +1 in 2026-08: /refine, the evidence-backed tuning report that seals
    // the attest ledger, shadow routing, and the /improve pipeline.
    assert!(help.contains("/refine"));
    assert_eq!(slash_command_specs().len(), 162);
    assert!(resume_supported_slash_commands().len() >= 25);
}

#[test]
fn renders_per_command_help_detail() {
    // given
    let command = "plugins";

    // when
    let help = render_slash_command_help_detail(command).expect("detail help should exist");

    // then
    assert!(help.contains("/plugin"));
    assert!(help.contains("Summary          Manage installed plugins"));
    assert!(help.contains("Aliases          /plugins, /marketplace"));
    assert!(help.contains("Category         Workspace & git"));
    assert!(help.contains("Availability    "));
    assert!(help.contains("Side effect      "));
    assert!(help.contains("Risk             "));
}

#[test]
fn renders_per_command_help_detail_for_mcp() {
    let help = render_slash_command_help_detail("mcp").expect("detail help should exist");
    assert!(help.contains("/mcp"));
    assert!(help.contains("Summary          Inspect configured MCP servers"));
    assert!(help.contains("Category         Discovery & debugging"));
    assert!(help.contains("Examples         /mcp list, /mcp auth github"));
    assert!(help.contains("Related          /tools, /config, /doctor"));
    assert!(help.contains("Resume           Supported with --resume SESSION.jsonl"));
}

#[test]
fn command_metadata_exposes_risk_examples_and_related_actions() {
    let metadata = slash_command_metadata("commit").expect("commit metadata");

    assert!(metadata.risk.contains("git index"));
    assert_eq!(metadata.examples, &["/commit"]);
    assert_eq!(metadata.related, &["diff", "review", "pr"]);
}

#[test]
fn validate_slash_command_input_rejects_extra_single_value_arguments() {
    // given
    let session_input = "/session switch current next";
    let plugin_input = "/plugin enable demo extra";

    // when
    let session_error = validate_slash_command_input(session_input)
        .expect_err("session input should be rejected")
        .to_string();
    let plugin_error = validate_slash_command_input(plugin_input)
        .expect_err("plugin input should be rejected")
        .to_string();

    // then
    assert!(session_error.contains("Unexpected arguments for /session switch."));
    assert!(session_error.contains("  Usage            /session switch <session-id>"));
    assert!(plugin_error.contains("Unexpected arguments for /plugin enable."));
    assert!(plugin_error.contains("  Usage            /plugin enable <name>"));
}

#[test]
fn parses_share_and_unshare_targets() {
    // Bare /share keeps the local-only behavior (target None).
    assert_eq!(
        SlashCommand::parse("/share"),
        Ok(Some(SlashCommand::Share { target: None }))
    );
    // /share gist opts into the hosted upload, case-insensitively.
    assert_eq!(
        SlashCommand::parse("/share GIST"),
        Ok(Some(SlashCommand::Share {
            target: Some("gist".to_string())
        }))
    );
    // Any other token is rejected loudly — never a silent local fallback.
    assert!(SlashCommand::parse("/share public").is_err());
    // /unshare captures the gist id.
    assert_eq!(
        SlashCommand::parse("/unshare abc123"),
        Ok(Some(SlashCommand::Unshare {
            id: Some("abc123".to_string())
        }))
    );
}

#[test]
fn suggests_closest_slash_commands_for_typos_and_aliases() {
    // "stats" is an alias of /status now; the suggestion resolves to the
    // canonical spec name.
    let suggestions = suggest_slash_commands("stats", 3);
    assert!(suggestions.contains(&"/status".to_string()));
    assert!(suggestions.len() <= 3);
    let plugin_suggestions = suggest_slash_commands("/plugns", 3);
    assert!(plugin_suggestions.contains(&"/plugin".to_string()));
    assert!(!suggest_slash_commands("rename", 5).contains(&"/rename".to_string()));
    assert_eq!(suggest_slash_commands("zzz", 3), Vec::<String>::new());
}

#[test]
fn hidden_commands_do_not_render_public_help_detail() {
    assert!(render_slash_command_help_detail("rename").is_none());
    assert!(render_slash_command_help_detail("share").is_none());
    assert!(render_slash_command_help_detail("security-review").is_none());
    assert!(render_slash_command_help_detail("copy").is_none());
    assert!(render_slash_command_help_detail("vim").is_none());
    assert!(render_slash_command_help_detail("upgrade").is_none());
    assert!(render_slash_command_help_detail("desktop").is_none());
    assert!(render_slash_command_help_detail("brief").is_none());
}

#[test]
fn low_value_deferred_commands_do_not_render_public_help_or_suggestions() {
    let help = render_slash_command_help();
    assert!(!help.contains("/share"));
    assert!(!help.contains("/copy [last|all]"));
    assert!(!help.contains("/security-review"));
    assert!(!help.contains("/privacy-settings"));
    assert!(!help.contains("/vim"));
    assert!(!help.contains("/upgrade"));
    assert!(!help.contains("/desktop"));
    assert!(!help.contains("/brief"));
    assert!(!help.contains("/advisor"));
    assert!(!help.contains("/stickers"));
    assert!(!help.contains("/insights"));
    assert!(!help.contains("/thinkback"));
    assert!(!help.contains("/release-notes"));
    assert!(!help.contains("/keybindings"));
    // Platform-impossible / REPL-only toggles and Lane B parity stubs are kept
    // out of the default help surface (honest catalog, no over-advertising).
    assert!(!help.contains("/voice"));
    assert!(!help.contains("/color"));
    assert!(!help.contains("/ant-trace"));
    assert!(!help.contains("/commit-push-pr"));
    assert!(!help.contains("/install-github-app"));
    assert!(!help.contains("/statusline"));

    assert!(!suggest_slash_commands("share", 5).contains(&"/share".to_string()));
    assert!(!suggest_slash_commands("copy", 5).contains(&"/copy".to_string()));
    assert!(!suggest_slash_commands("rename", 5).contains(&"/rename".to_string()));
    assert!(!suggest_slash_commands("vim", 5).contains(&"/vim".to_string()));
    assert!(!suggest_slash_commands("upgrade", 5).contains(&"/upgrade".to_string()));
    assert!(!suggest_slash_commands("brief", 5).contains(&"/brief".to_string()));
    assert!(!suggest_slash_commands("advisor", 5).contains(&"/advisor".to_string()));
    assert!(!suggest_slash_commands("voice", 5).contains(&"/voice".to_string()));
    assert!(!suggest_slash_commands("color", 5).contains(&"/color".to_string()));
    assert!(!suggest_slash_commands("ant-trace", 5).contains(&"/ant-trace".to_string()));
    assert!(!suggest_slash_commands("commit-push-pr", 5).contains(&"/commit-push-pr".to_string()));
}

#[test]
fn compacts_sessions_via_slash_command() {
    let mut session = Session::new();
    session.messages = std::sync::Arc::new(vec![
        ConversationMessage::user_text("a ".repeat(200)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "b ".repeat(200),
        }]),
        ConversationMessage::user_text("c ".repeat(200)),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "recent".to_string(),
        }]),
    ]);

    let result = handle_slash_command(
        "/compact",
        &session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 1,
        },
    )
    .expect("slash command should be handled");

    assert!(result.message.contains("Compacted 2 messages"));
    assert_eq!(result.session.messages[0].role, MessageRole::System);
}

#[test]
fn help_command_is_non_mutating() {
    let session = Session::new();
    let result = handle_slash_command("/help", &session, CompactionConfig::default())
        .expect("help command should be handled");
    assert_eq!(result.session, session);
    assert!(result.message.contains("Slash commands"));
}

#[test]
fn ignores_unknown_or_runtime_bound_slash_commands() {
    let session = Session::new();
    // Lane B: unknown commands now route through the registry so the
    // REPL can show a did-you-mean message instead of silently passing
    // the input through as plain chat.
    let unknown = handle_slash_command("/unknown", &session, CompactionConfig::default())
        .expect("unknown command should surface a registry error");
    assert!(unknown.message.contains("Unknown slash command '/unknown'"));
    assert_eq!(unknown.session, session);
    assert!(handle_slash_command("/status", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/sandbox", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/bughunter", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/commit", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/pr", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/issue", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/ultraplan", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/council", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/teleport foo", &session, CompactionConfig::default()).is_none());
    assert!(
        handle_slash_command("/debug-tool-call", &session, CompactionConfig::default()).is_none()
    );
    assert!(handle_slash_command("/model claude", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command(
        "/permissions read-only",
        &session,
        CompactionConfig::default()
    )
    .is_none());
    assert!(handle_slash_command("/clear", &session, CompactionConfig::default()).is_none());
    assert!(
        handle_slash_command("/clear --confirm", &session, CompactionConfig::default()).is_none()
    );
    assert!(handle_slash_command("/cost", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command(
        "/resume session.json",
        &session,
        CompactionConfig::default()
    )
    .is_none());
    assert!(handle_slash_command(
        "/resume session.jsonl",
        &session,
        CompactionConfig::default()
    )
    .is_none());
    assert!(handle_slash_command("/config", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/config env", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/mcp list", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/diff", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/version", &session, CompactionConfig::default()).is_none());
    assert!(
        handle_slash_command("/export note.txt", &session, CompactionConfig::default()).is_none()
    );
    assert!(handle_slash_command("/session list", &session, CompactionConfig::default()).is_none());
    assert!(handle_slash_command("/plugins list", &session, CompactionConfig::default()).is_none());
}

#[test]
fn renders_plugins_report_with_name_version_and_status() {
    let rendered = render_plugins_report(&[
        PluginSummary {
            metadata: PluginMetadata {
                id: "demo@external".to_string(),
                name: "demo".to_string(),
                version: "1.2.3".to_string(),
                description: "demo plugin".to_string(),
                kind: PluginKind::External,
                source: "demo".to_string(),
                default_enabled: false,
                root: None,
            },
            enabled: true,
        },
        PluginSummary {
            metadata: PluginMetadata {
                id: "sample@external".to_string(),
                name: "sample".to_string(),
                version: "0.9.0".to_string(),
                description: "sample plugin".to_string(),
                kind: PluginKind::External,
                source: "sample".to_string(),
                default_enabled: false,
                root: None,
            },
            enabled: false,
        },
    ]);

    assert!(rendered.contains("demo"));
    assert!(rendered.contains("v1.2.3"));
    assert!(rendered.contains("enabled"));
    assert!(rendered.contains("sample"));
    assert!(rendered.contains("v0.9.0"));
    assert!(rendered.contains("disabled"));
}

#[test]
fn lists_agents_from_project_and_user_roots() {
    let workspace = temp_dir("agents-workspace");
    let project_zo_agents = workspace.join(".zo").join("agents");
    let user_home = temp_dir("agents-home");
    let user_agents = user_home.join(".zo").join("agents");

    write_markdown_agent(
        &project_zo_agents,
        "reviewer",
        "Zo reviewer",
        "gpt-5.5",
        "high",
    );
    fs::write(
        project_zo_agents.join("code-reviewer.md"),
        "---\ndescription: Should not shadow built-in\ntools: bash\n---\nShadow built-in.",
    )
    .expect("write built-in shadow agent");
    fs::write(
        project_zo_agents.join("invalid-permission.md"),
        "---\ndescription: Invalid native agent\npermissionMode: readonly-but-typo\n---\nBody.",
    )
    .expect("write invalid native agent");
    write_agent(
        &project_zo_agents,
        "planner",
        "Project planner",
        "gpt-5.6-sol",
        "medium",
    );
    write_agent(
        &user_agents,
        "planner",
        "User planner",
        "gpt-5.6-luna",
        "high",
    );
    write_agent(
        &user_agents,
        "verifier",
        "Verification agent",
        "gpt-5.6-luna",
        "high",
    );

    let roots = vec![
        (DefinitionSource::ProjectZo, project_zo_agents),
        (DefinitionSource::UserZo, user_agents),
    ];
    let report =
        render_agents_report(&load_agents_from_roots(&roots).expect("agent roots should load"));

    assert!(report.contains("Agents"));
    assert!(report.contains("3 active agents"));
    assert!(report.contains("Project (.zo):"));
    assert!(report.contains("reviewer · Zo reviewer · gpt-5.5"));
    assert!(!report.contains("Display reviewer"));
    assert!(!report.contains("reviewer · Zo reviewer · gpt-5.5 · high"));
    assert!(!report.contains("code-reviewer"));
    assert!(!report.contains("invalid-permission"));
    assert!(report.contains("planner · Project planner · gpt-5.6-sol · medium"));
    assert!(report.contains("User (~/.zo):"));
    assert!(report.contains("(shadowed by Project (.zo)) planner · User planner"));
    assert!(report.contains("verifier · Verification agent · gpt-5.6-luna · high"));

    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(user_home);
}

#[test]
fn discovers_only_zo_project_agent_roots() {
    let workspace = temp_dir("agents-zo-only");
    let cwd = workspace.join("nested");
    fs::create_dir_all(cwd.join(".zo").join("agents")).expect("create Zo agent root");
    let roots = discover_definition_roots(&cwd, "agents");
    let project_roots = roots
        .iter()
        .filter(|(source, _)| *source == DefinitionSource::ProjectZo)
        .map(|(_, path)| path)
        .collect::<Vec<_>>();

    assert!(!project_roots.is_empty());
    assert!(project_roots
        .iter()
        .all(|path| path.ends_with(Path::new(".zo").join("agents"))));
    assert!(roots.iter().all(|(_, path)| {
        !path
            .components()
            .any(|component| component.as_os_str() == ".codex")
    }));
}

#[test]
fn empty_global_skill_home_overrides_do_not_add_relative_roots() {
    let workspace = temp_dir("skills-empty-homes");
    let cwd = workspace.join("nested");
    fs::create_dir_all(cwd.join(".zo").join("skills")).expect("create project skill root");
    fs::create_dir_all(cwd.join(".zo").join("commands")).expect("create project command root");

    let roots = discover_skill_roots_from(
        &cwd,
        Some(PathBuf::new()),
        Some(PathBuf::new()),
        Some(PathBuf::new()),
    );

    assert!(!roots.is_empty());
    assert!(roots
        .iter()
        .all(|root| root.source == DefinitionSource::ProjectZo));
    assert!(roots.iter().all(|root| root.path.is_absolute()));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn load_agents_skips_unreadable_root_and_keeps_others() {
    // C6 회귀: 한 root 의 read_dir 실패(존재하지 않는 디렉터리)가 다른 정상
    // root 의 에이전트까지 잃게 만들면 안 된다.
    let workspace = temp_dir("agents-robust-workspace");
    let user_agents = workspace.join(".zo").join("agents");
    write_agent(
        &user_agents,
        "planner",
        "Project planner",
        "gpt-5.6-sol",
        "medium",
    );

    let missing = workspace.join("does-not-exist").join("agents");
    let roots = vec![
        (DefinitionSource::ProjectZo, missing),
        (DefinitionSource::UserZo, user_agents),
    ];
    let report = render_agents_report(
        &load_agents_from_roots(&roots).expect("missing root must not fail the load"),
    );
    assert!(report.contains("planner · Project planner"));

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn lists_skills_from_project_and_user_roots() {
    let workspace = temp_dir("skills-workspace");
    let project_skills = workspace.join(".zo").join("skills");
    let project_commands = workspace.join(".zo").join("commands");
    let user_home = temp_dir("skills-home");
    let user_skills = user_home.join(".zo").join("skills");

    write_skill(&project_skills, "plan", "Project planning guidance");
    write_legacy_command(&project_commands, "deploy", "Legacy deployment guidance");
    write_skill(&user_skills, "plan", "User planning guidance");
    write_skill(&user_skills, "help", "Help guidance");

    let roots = vec![
        SkillRoot {
            source: DefinitionSource::ProjectZo,
            path: project_skills,
            origin: SkillOrigin::SkillsDir,
        },
        SkillRoot {
            source: DefinitionSource::ProjectZo,
            path: project_commands,
            origin: SkillOrigin::LegacyCommandsDir,
        },
        SkillRoot {
            source: DefinitionSource::UserZo,
            path: user_skills,
            origin: SkillOrigin::SkillsDir,
        },
    ];
    let report =
        render_skills_report(&load_skills_from_roots(&roots).expect("skill roots should load"));

    assert!(report.contains("Skills"));
    assert!(report.contains("3 available skills"));
    assert!(report.contains("Project (.zo):"));
    assert!(report.contains("plan · Project planning guidance"));
    assert!(report.contains("deploy · Legacy deployment guidance · legacy /commands"));
    assert!(report.contains("User (~/.zo):"));
    assert!(report.contains("(shadowed by Project (.zo)) plan · User planning guidance"));
    assert!(report.contains("help · Help guidance"));

    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(user_home);
}

#[test]
fn agents_and_skills_usage_support_help_and_unexpected_args() {
    let cwd = temp_dir("slash-usage");

    let agents_help = crate::handle_agents_slash_command(Some("help"), &cwd).expect("agents help");
    assert!(agents_help.contains("Usage            /agents [list|help]"));
    assert!(agents_help.contains("Direct CLI       zo agents"));

    let agents_unexpected =
        crate::handle_agents_slash_command(Some("show planner"), &cwd).expect("agents usage");
    assert!(agents_unexpected.contains("Unexpected       show planner"));

    let skills_help =
        crate::handle_skills_slash_command(Some("--help"), &cwd).expect("skills help");
    assert!(skills_help.contains("Usage            /skills [list|install <path>|help]"));
    assert!(skills_help.contains(
        "Install root     $ZO_CONFIG_HOME/skills, $ZO_HOME/skills, or ~/.zo/skills"
    ));
    assert!(skills_help.contains("Sources          .zo/skills and .zo/commands"));

    let skills_unexpected =
        crate::handle_skills_slash_command(Some("show help"), &cwd).expect("skills usage");
    assert!(skills_unexpected.contains("Unexpected       show help"));

    let _ = fs::remove_dir_all(cwd);
}

#[test]
fn mcp_usage_supports_help_and_unexpected_args() {
    let cwd = temp_dir("mcp-usage");

    let help = crate::handle_mcp_slash_command(Some("help"), &cwd).expect("mcp help");
    assert!(help.contains(
        "Usage            /mcp [list|show <server>|auth [list|<server>]|logout <server>|help]"
    ));
    assert!(help.contains(
        "Direct CLI       zo mcp [list|show <server>|auth [list|<server>]|logout <server>|help]"
    ));

    let unexpected =
        crate::handle_mcp_slash_command(Some("show alpha beta"), &cwd).expect("mcp usage");
    assert!(unexpected.contains("Unexpected       show alpha beta"));

    let _ = fs::remove_dir_all(cwd);
}

#[test]
fn mcp_auth_list_and_logout_render_against_config() {
    // This is the only commands-crate test that touches ZO_CONFIG_HOME, and
    // no sibling test reads the global config home or stored credentials, so the
    // env mutation below cannot race another test.
    let workspace = temp_dir("mcp-auth-workspace");
    let config_home = temp_dir("mcp-auth-home");
    fs::create_dir_all(workspace.join(".zo")).expect("workspace config dir");
    fs::create_dir_all(&config_home).expect("config home");
    fs::write(
        workspace.join(".zo").join("settings.json"),
        r#"{
          "mcpServers": {
            "stdioonly": { "command": "uvx", "args": ["x"] },
            "remote": {
              "url": "https://remote.example/mcp",
              "oauth": { "clientId": "demo" }
            }
          }
        }"#,
    )
    .expect("write settings");
    // Project-scoped `mcpServers` are gated behind the supply-chain trust check,
    // so trust these explicitly for this rendering test (mirrors the config
    // crate's own gated tests).
    fs::write(
        workspace.join(".zo").join("trusted-mcp-servers.json"),
        r#"["stdioonly","remote"]"#,
    )
    .expect("trust project servers");

    std::env::set_var("ZO_CONFIG_HOME", &config_home);

    // auth list: only the OAuth-capable server appears, unauthenticated.
    let auth_list = crate::handle_mcp_slash_command(Some("auth list"), &workspace)
        .expect("mcp auth list report");
    assert!(auth_list.contains("MCP auth"));
    assert!(auth_list.contains("remote"));
    assert!(auth_list.contains("not authenticated"));
    assert!(!auth_list.contains("stdioonly"));

    // auth <server> for a non-remote (stdio) transport reports the unsupported
    // case without attempting discovery or a browser flow.
    let auth_stdio = crate::handle_mcp_slash_command(Some("auth stdioonly"), &workspace)
        .expect("mcp auth stdio report");
    assert!(auth_stdio.contains("does not support MCP OAuth"));

    // auth <server> for an unknown server reports not-configured.
    let auth_missing = crate::handle_mcp_slash_command(Some("auth ghost"), &workspace)
        .expect("mcp auth missing report");
    assert!(auth_missing.contains("is not configured"));

    // logout with no stored credentials is a graceful no-op.
    let logout = crate::handle_mcp_slash_command(Some("logout remote"), &workspace)
        .expect("mcp logout report");
    assert!(logout.contains("no stored credentials to remove"));

    std::env::remove_var("ZO_CONFIG_HOME");
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(config_home);
}

#[test]
fn renders_mcp_reports_from_loaded_config() {
    let workspace = temp_dir("mcp-config-workspace");
    let config_home = temp_dir("mcp-config-home");
    fs::create_dir_all(workspace.join(".zo")).expect("workspace config dir");
    fs::create_dir_all(&config_home).expect("config home");
    fs::write(
        workspace.join(".zo").join("settings.json"),
        r#"{
          "mcpServers": {
            "alpha": {
              "command": "uvx",
              "args": ["alpha-server"],
              "env": {"ALPHA_TOKEN": "secret"},
              "toolCallTimeoutMs": 1200
            },
            "remote": {
              "type": "http",
              "url": "https://remote.example/mcp",
              "headers": {"Authorization": "Bearer secret"},
              "headersHelper": "./bin/headers",
              "oauth": {
                "clientId": "remote-client",
                "callbackPort": 7878
              }
            }
          }
        }"#,
    )
    .expect("write settings");
    fs::write(
        workspace.join(".zo").join("settings.local.json"),
        r#"{
          "mcpServers": {
            "remote": {
              "type": "ws",
              "url": "wss://remote.example/mcp"
            }
          }
        }"#,
    )
    .expect("write local settings");
    // Project-scoped `mcpServers` (`alpha`, the project `remote`) are gated behind
    // the supply-chain trust check; trust them so this report renders them as
    // before. The `remote` override in `settings.local.json` is operator-authored
    // (Local scope), so it is never gated.
    fs::write(
        workspace.join(".zo").join("trusted-mcp-servers.json"),
        r#"["alpha","remote"]"#,
    )
    .expect("trust project servers");

    let loader = ConfigLoader::new(&workspace, &config_home);
    let list = crate::plugins_agents::render_mcp_report_for(&loader, &workspace, None)
        .expect("mcp list report should render");
    assert!(list.contains("Configured servers 2"));
    assert!(list.contains("alpha"));
    assert!(list.contains("stdio"));
    assert!(list.contains("project"));
    assert!(list.contains("uvx alpha-server"));
    assert!(list.contains("remote"));
    assert!(list.contains("ws"));
    assert!(list.contains("local"));
    assert!(list.contains("wss://remote.example/mcp"));

    let show =
        crate::plugins_agents::render_mcp_report_for(&loader, &workspace, Some("show alpha"))
            .expect("mcp show report should render");
    assert!(show.contains("Name              alpha"));
    assert!(show.contains("Command           uvx"));
    assert!(show.contains("Args              alpha-server"));
    assert!(show.contains("Env keys          ALPHA_TOKEN"));
    assert!(show.contains("Tool timeout      1200 ms"));

    let remote =
        crate::plugins_agents::render_mcp_report_for(&loader, &workspace, Some("show remote"))
            .expect("mcp show remote report should render");
    assert!(remote.contains("Transport         ws"));
    assert!(remote.contains("URL               wss://remote.example/mcp"));

    let missing =
        crate::plugins_agents::render_mcp_report_for(&loader, &workspace, Some("show missing"))
            .expect("missing report should render");
    assert!(missing.contains("server `missing` is not configured"));

    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(config_home);
}

#[test]
fn parses_quoted_skill_frontmatter_values() {
    let contents = "---\nname: \"hud\"\ndescription: 'Quoted description'\n---\n";
    let (name, description) = crate::plugins_agents::parse_skill_frontmatter(contents);
    assert_eq!(name.as_deref(), Some("hud"));
    assert_eq!(description.as_deref(), Some("Quoted description"));
}

#[test]
fn installs_skill_into_user_registry_and_preserves_nested_files() {
    let workspace = temp_dir("skills-install-workspace");
    let source_root = workspace.join("source").join("help");
    let install_root = temp_dir("skills-install-root");
    write_skill(
        source_root.parent().expect("parent"),
        "help",
        "Helpful skill",
    );
    let script_dir = source_root.join("scripts");
    fs::create_dir_all(&script_dir).expect("script dir");
    fs::write(script_dir.join("run.sh"), "#!/bin/sh\necho help\n").expect("write script");

    let installed = crate::plugins_agents::install_skill_into(
        source_root.to_str().expect("utf8 skill path"),
        &workspace,
        &install_root,
    )
    .expect("skill should install");

    assert_eq!(installed.invocation_name, "help");
    assert_eq!(installed.display_name.as_deref(), Some("help"));
    assert!(installed.installed_path.ends_with(Path::new("help")));
    assert!(installed.installed_path.join("SKILL.md").is_file());
    assert!(installed
        .installed_path
        .join("scripts")
        .join("run.sh")
        .is_file());

    let report = crate::plugins_agents::render_skill_install_report(&installed);
    assert!(report.contains("Result           installed help"));
    assert!(report.contains("Invoke as        $help"));
    assert!(report.contains(&install_root.display().to_string()));

    let roots = vec![SkillRoot {
        source: DefinitionSource::UserZoConfigHome,
        path: install_root.clone(),
        origin: SkillOrigin::SkillsDir,
    }];
    let listed = render_skills_report(
        &load_skills_from_roots(&roots).expect("installed skills should load"),
    );
    assert!(listed.contains("User ($ZO_CONFIG_HOME):"));
    assert!(listed.contains("help · Helpful skill"));

    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(install_root);
}

#[test]
fn installs_plugin_from_path_and_lists_it() {
    let config_home = temp_dir("home");
    let source_root = temp_dir("source");
    write_external_plugin(&source_root, "demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install = handle_plugins_slash_command(
        Some("install"),
        Some(source_root.to_str().expect("utf8 path")),
        &mut manager,
    )
    .expect("install command should succeed");
    assert!(install.reload_runtime);
    assert!(install.message.contains("installed demo@external"));
    assert!(install.message.contains("Name             demo"));
    assert!(install.message.contains("Version          1.0.0"));
    assert!(install.message.contains("Status           enabled"));

    let list = handle_plugins_slash_command(Some("list"), None, &mut manager)
        .expect("list command should succeed");
    assert!(!list.reload_runtime);
    assert!(list.message.contains("demo"));
    assert!(list.message.contains("v1.0.0"));
    assert!(list.message.contains("enabled"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn enables_and_disables_plugin_by_name() {
    let config_home = temp_dir("toggle-home");
    let source_root = temp_dir("toggle-source");
    write_external_plugin(&source_root, "demo", "1.0.0");

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    handle_plugins_slash_command(
        Some("install"),
        Some(source_root.to_str().expect("utf8 path")),
        &mut manager,
    )
    .expect("install command should succeed");

    let disable = handle_plugins_slash_command(Some("disable"), Some("demo"), &mut manager)
        .expect("disable command should succeed");
    assert!(disable.reload_runtime);
    assert!(disable.message.contains("disabled demo@external"));
    assert!(disable.message.contains("Name             demo"));
    assert!(disable.message.contains("Status           disabled"));

    let list = handle_plugins_slash_command(Some("list"), None, &mut manager)
        .expect("list command should succeed");
    assert!(list.message.contains("demo"));
    assert!(list.message.contains("disabled"));

    let enable = handle_plugins_slash_command(Some("enable"), Some("demo"), &mut manager)
        .expect("enable command should succeed");
    assert!(enable.reload_runtime);
    assert!(enable.message.contains("enabled demo@external"));
    assert!(enable.message.contains("Name             demo"));
    assert!(enable.message.contains("Status           enabled"));

    let list = handle_plugins_slash_command(Some("list"), None, &mut manager)
        .expect("list command should succeed");
    assert!(list.message.contains("demo"));
    assert!(list.message.contains("enabled"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
fn lists_auto_installed_bundled_plugins_with_status() {
    let config_home = temp_dir("bundled-home");
    let bundled_root = temp_dir("bundled-root");
    let bundled_plugin = bundled_root.join("starter");
    write_bundled_plugin(&bundled_plugin, "starter", "0.1.0", false);

    let mut config = PluginManagerConfig::new(&config_home);
    config.bundled_root = Some(bundled_root.clone());
    let mut manager = PluginManager::new(config);

    let list = handle_plugins_slash_command(Some("list"), None, &mut manager)
        .expect("list command should succeed");
    assert!(!list.reload_runtime);
    assert!(list.message.contains("starter"));
    assert!(list.message.contains("v0.1.0"));
    assert!(list.message.contains("disabled"));

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(bundled_root);
}

/// The `/loop ci|pr|audit` recipe presets expand to a recurring interval loop
/// with a curated prompt. Each preset carries its default interval, and user
/// modifiers are prepended so the host's leading-flag parser still consumes them.
#[test]
fn loop_recipe_presets_expand_to_recurring_loops() {
    // ci → every 5m, template mentions CI + the digest channel + overnight protocol.
    let ci = SlashCommand::parse("/loop ci")
        .expect("parse should succeed")
        .expect("slash command expected");
    let SlashCommand::Loop {
        command: LoopCommand::StartInterval { every, prompt },
    } = ci
    else {
        panic!("/loop ci must expand to a recurring interval loop");
    };
    assert_eq!(every.duration, Duration::from_secs(300), "ci runs every 5m");
    assert!(prompt.contains("gh run list"), "ci template drives the CI check");
    assert!(
        prompt.contains("channel: \"digest\""),
        "ci records into the digest channel"
    );
    assert!(
        prompt.contains("send_to_user"),
        "the overnight protocol footer is appended"
    );
    assert!(
        prompt.contains("read-only by default"),
        "presets are read-only + propose by default"
    );

    // Intervals per preset.
    for (alias, secs) in [("ci", 300), ("pr", 600), ("audit", 1800)] {
        let parsed = SlashCommand::parse(&format!("/loop {alias}"))
            .expect("parse should succeed")
            .expect("slash command expected");
        let SlashCommand::Loop {
            command: LoopCommand::StartInterval { every, .. },
        } = parsed
        else {
            panic!("/loop {alias} must expand to an interval loop");
        };
        assert_eq!(
            every.duration,
            Duration::from_secs(secs),
            "{alias} default interval"
        );
    }
}

/// A preset merges user modifiers (e.g. `--max-runs 10`) BEFORE the template so
/// the host's `split_loop_budget_flags` (leading-flags only) still consumes them.
#[test]
fn loop_preset_merges_user_modifiers_before_the_template() {
    let parsed = SlashCommand::parse("/loop audit --max-runs 10")
        .expect("parse should succeed")
        .expect("slash command expected");
    let SlashCommand::Loop {
        command: LoopCommand::StartInterval { prompt, .. },
    } = parsed
    else {
        panic!("/loop audit must expand to an interval loop");
    };
    assert!(
        prompt.starts_with("--max-runs 10 "),
        "modifiers are prepended so the leading-flag parser consumes them: {prompt}"
    );
    assert!(prompt.contains("Audit the workspace"), "the template follows the modifiers");
}

/// `/goal <text> --allow-writes` sets the opt-in flag; an omitted flag defaults
/// to read-only (`false`).
#[test]
fn goal_allow_writes_opt_in_is_parsed() {
    let opted = SlashCommand::parse("/goal make tests pass --check cargo:test --allow-writes")
        .expect("parse should succeed")
        .expect("slash command expected");
    let SlashCommand::Goal {
        command: GoalCommand::Start { goal, options },
    } = opted
    else {
        panic!("expected a goal start command");
    };
    assert_eq!(goal, "make tests pass");
    assert!(options.allow_writes, "--allow-writes sets the opt-in");
    assert_eq!(options.checks, vec!["cargo:test".to_string()]);

    let default = SlashCommand::parse("/goal make tests pass --check cargo:test")
        .expect("parse should succeed")
        .expect("slash command expected");
    let SlashCommand::Goal {
        command: GoalCommand::Start { options, .. },
    } = default
    else {
        panic!("expected a goal start command");
    };
    assert!(
        !options.allow_writes,
        "an omitted --allow-writes defaults to read-only"
    );
}

#[test]
fn mcp_auth_list_includes_http_servers_without_explicit_oauth() {
    use crate::plugins_agents::render_mcp_auth_list;
    use runtime::{ConfigSource, McpRemoteServerConfig, McpServerConfig, ScopedMcpServerConfig};
    use std::collections::BTreeMap;

    let mut servers = BTreeMap::new();
    servers.insert(
        "remote-http".to_string(),
        ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: McpServerConfig::Http(McpRemoteServerConfig {
                url: "https://mcp.example.test/mcp".to_string(),
                headers: BTreeMap::new(),
                headers_helper: None,
                // No explicit `oauth`: still discoverable, so it must be listed.
                oauth: None,
            }),
        },
    );

    let report = render_mcp_auth_list(Path::new("/tmp"), &servers);
    assert!(
        report.contains("remote-http"),
        "a discoverable HTTP server should be listed: {report}"
    );
    assert!(
        !report.contains("No OAuth-capable"),
        "must not report none-capable when a remote server is present: {report}"
    );
}
