use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;

use commands::{public_slash_command_specs, slash_command_specs, SlashCommand};
use runtime::PermissionMode;

use crate::{
    default_permission_mode, default_prompt_date, resolved_tui_mode_is_inline,
    interactive_default_permission_mode, DEFAULT_MODEL,
};

pub(crate) type AllowedToolSet = BTreeSet<String>;
pub(crate) type DisallowedToolSet = BTreeSet<String>;

pub(crate) const CLI_OPTION_SUGGESTIONS: &[&str] = &[
    "--help",
    "-h",
    "--version",
    "-V",
    "--update",
    "update",
    "--model",
    "--output-format",
    "--permission-mode",
    "--dangerously-skip-permissions",
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--resume",
    "--fork-session",
    "--continue",
    "--print",
    "-p",
    "--max-turns",
    "--max-tool-calls",
    "--system-prompt",
    "--append-system-prompt",
    "--verbose",
    "--input-format",
    "--mcp-config",
    "--prefill",
    "--no-follow",
    "--cwd",
    "--bind",
    "--port",
    "--plain",
    "--inline",
    "--fullscreen",
    "--add-dir",
    "--settings",
    "--session-id",
    "--check",
    "--fallback-model",
    "--from-turn",
    "--strict-mcp-config",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliAction {
    DumpManifests,
    BootstrapPlan,
    Agents {
        args: Option<String>,
    },
    Mcp {
        args: Option<String>,
    },
    Skills {
        args: Option<String>,
    },
    PrintSystemPrompt {
        cwd: PathBuf,
        date: String,
        /// `--model`, so the dump shows the `Model family:` line a session on
        /// that model would actually get. `None` renders the default frontier
        /// label.
        model: Option<String>,
    },
    Version,
    Update {
        check: bool,
    },
    BackgroundUpdate,
    /// Diagnose the local environment and, by default, apply only safe,
    /// reversible local repairs. `check` selects strictly read-only mode.
    Doctor {
        check: bool,
    },
    ResumeSession {
        session_path: PathBuf,
        from_turn: Option<u32>,
        fork_session: bool,
        commands: Vec<String>,
    },
    Status {
        model: String,
        permission_mode: PermissionMode,
    },
    Sandbox,
    Prompt {
        prompt: String,
        model: String,
        /// `true` when the model came from an explicit `--model` flag.
        model_pinned: bool,
        output_format: CliOutputFormat,
        allowed_tools: Option<AllowedToolSet>,
        disallowed_tools: Option<DisallowedToolSet>,
        permission_mode: PermissionMode,
        max_turns: Option<usize>,
        max_tool_calls: Option<usize>,
        system_prompt: Option<String>,
        append_system_prompt: Option<String>,
        verbose: bool,
        input_format: CliInputFormat,
        mcp_config: Option<PathBuf>,
        prefill: Option<String>,
        no_follow: bool,
        /// Explicit session id for this one-shot (`--session-id`, CC parity).
        session_id: Option<String>,
        /// Retry-once model when the primary fails on overload/rate-limit
        /// (`--fallback-model`, CC parity).
        fallback_model: Option<String>,
        /// Objective check command for the deep gate (`--check`, first-class
        /// door for the engine previously reachable only through the
        /// `ZO_AUTO_VERIFY_CMD` env var): the run's edits are verified against
        /// this command exiting 0.
        check_command: Option<String>,
    },
    Login {
        provider: Option<String>,
    },
    Logout,
    Init,
    Repl {
        model: String,
        /// `true` when the model came from an explicit `--model` flag.
        model_pinned: bool,
        allowed_tools: Option<AllowedToolSet>,
        disallowed_tools: Option<DisallowedToolSet>,
        permission_mode: PermissionMode,
        max_turns: Option<usize>,
        max_tool_calls: Option<usize>,
        system_prompt: Option<String>,
        append_system_prompt: Option<String>,
        verbose: bool,
        mcp_config: Option<PathBuf>,
        /// `--inline`: pin primary-screen inline rendering. Redundant now that
        /// inline is the default, kept so existing invocations keep working.
        inline: bool,
        /// `--fullscreen`: pin the alternate-screen presentation. The escape
        /// hatch for the persistent right-hand panel.
        fullscreen: bool,
    },
    SlashCommand {
        command: SlashCommand,
        model: String,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
    },
    /// Run the persistent session server (`zo serve`). Holds a pool of live
    /// sessions that survive client disconnects; `zo attach` connects to it.
    Serve {
        bind_addr: String,
        model: String,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
    },
    /// Run Zo as an Agent Client Protocol agent over stdio.
    Acp {
        model: String,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
    },
    /// Attach to a running [`Serve`](CliAction::Serve) server (`zo attach`).
    /// `session_id` is `None` to create a fresh session and attach to it.
    /// `plain` selects the line client over the default rich TUI client.
    Attach {
        bind_addr: String,
        session_id: Option<String>,
        plain: bool,
    },
    // prompt-mode formatting is only supported for non-interactive runs
    Help,
    /// Print a subcommand-specific help/usage block, then exit successfully.
    HelpText(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliOutputFormat {
    Text,
    Json,
    /// Newline-delimited JSON: one serialized `RenderBlock` per line.
    /// Mirrors the Claude Code `stream-json` output format.
    Ndjson,
}

/// Input format for non-interactive runs.
///
/// [`CliInputFormat::Text`] takes the prompt from the command line;
/// [`CliInputFormat::StreamJson`] reads newline-delimited JSON user messages
/// from stdin for Claude Code SDK-compatible headless automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliInputFormat {
    /// The prompt is taken verbatim from the command line (the default).
    Text,
    /// Newline-delimited JSON user messages from stdin.
    StreamJson,
}

impl CliInputFormat {
    /// Parse a `--input-format` value.
    ///
    /// # Errors
    /// Returns an "unsupported value" message for unknown values while keeping
    /// `json` and `ndjson` as stream-json aliases for SDK-compatible stdin.
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "stream-json" | "json" | "ndjson" => Ok(Self::StreamJson),
            other => Err(format!(
                "unsupported value for --input-format: {other} (expected text or stream-json)"
            )),
        }
    }
}

impl CliOutputFormat {
    /// Whether this run's visible text is consumed by a program rather than
    /// read by a person. `text` is excluded: it prints prose to a terminal a
    /// human may be watching, so it keeps the interactive output contract.
    ///
    /// Latched once at session start (never re-evaluated mid-session): the
    /// headless output contract it gates rides in the system prefix, and a
    /// mid-conversation prefix change re-bills the whole cached context.
    pub(crate) const fn machine_consumed(self) -> bool {
        matches!(self, Self::Json | Self::Ndjson)
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "ndjson" | "stream-json" => Ok(Self::Ndjson),
            other => Err(format!(
                "unsupported value for --output-format: {other} (expected text, json, or ndjson)"
            )),
        }
    }
}

/// Strip the process-wide flags (`--settings <file>`, `--strict-mcp-config`,
/// `--add-dir <path>`) out of `args` and install them as runtime overrides.
/// They apply to *every* action (interactive, `-p`, serve, …), so handling
/// them before the action parser keeps each action arm untouched.
fn extract_config_override_flags(args: &[String]) -> Result<Vec<String>, String> {
    let mut overrides = runtime::CliConfigOverrides::default();
    let mut add_dirs: Vec<PathBuf> = Vec::new();
    let mut remaining = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--strict-mcp-config" => {
                overrides.strict_mcp_config = true;
                index += 1;
            }
            "--settings" => {
                let value = next_value(args, index, "--settings")?;
                overrides.settings_file = Some(PathBuf::from(value));
                index += 2;
            }
            flag if flag.starts_with("--settings=") => {
                overrides.settings_file = Some(PathBuf::from(&flag[11..]));
                index += 1;
            }
            "--add-dir" => {
                let value = next_value(args, index, "--add-dir")?;
                add_dirs.push(PathBuf::from(value));
                index += 2;
            }
            flag if flag.starts_with("--add-dir=") => {
                add_dirs.push(PathBuf::from(&flag[10..]));
                index += 1;
            }
            other => {
                remaining.push(other.to_string());
                index += 1;
            }
        }
    }
    if let Some(path) = &overrides.settings_file {
        if !path.is_file() {
            return Err(format!("--settings file not found: {}", path.display()));
        }
    }
    runtime::ConfigLoader::set_cli_overrides(overrides);
    if !add_dirs.is_empty() {
        let mut roots = Vec::with_capacity(add_dirs.len());
        for dir in add_dirs {
            // Canonicalize so symlinked spellings can't dodge the boundary
            // comparison.
            let canonical = std::fs::canonicalize(&dir)
                .map_err(|error| format!("--add-dir {}: {error}", dir.display()))?;
            if !canonical.is_dir() {
                return Err(format!(
                    "--add-dir must name a directory: {}",
                    dir.display()
                ));
            }
            roots.push(canonical);
        }
        runtime::file_ops::set_additional_workspace_roots(roots);
    }
    Ok(remaining)
}

#[allow(clippy::too_many_lines)] // declarative flag→action table; splitting would scatter the CLI surface
/// Fetch the value argument following a space-separated flag at `index`, or a
/// uniform `missing value for {flag}` error. Shared by the value-taking arms of
/// [`parse_args`] so the lookup and error message live in one place. Returns
/// `&String` so callers' existing post-processing (clone/parse/push) is
/// unchanged.
fn next_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a String, String> {
    args.get(index + 1)
        .ok_or_else(|| format!("missing value for {flag}"))
}

#[allow(clippy::too_many_lines)]
pub(crate) fn parse_args(args: &[String]) -> Result<CliAction, String> {
    let args = extract_config_override_flags(args)?;
    let args = args.as_slice();
    let mut model = DEFAULT_MODEL.to_string();
    let mut model_pinned = false;
    let mut output_format = CliOutputFormat::Text;
    let mut input_format = CliInputFormat::Text;
    let mut permission_mode_override = None;
    let mut wants_help = false;
    let mut wants_version = false;
    let mut allowed_tool_values = Vec::new();
    let mut disallowed_tool_values = Vec::new();
    let mut max_turns: Option<usize> = None;
    let mut max_tool_calls: Option<usize> = None;
    let mut system_prompt_override: Option<String> = None;
    let mut append_system_prompt: Option<String> = None;
    let mut verbose = false;
    let mut mcp_config: Option<PathBuf> = None;
    let mut prefill: Option<String> = None;
    let mut no_follow = false;
    let mut inline = false;
    let mut fullscreen = false;
    let mut session_id_override: Option<String> = None;
    let mut fallback_model: Option<String> = None;
    let mut check_command: Option<String> = None;
    let mut fork_session = false;
    let mut _cwd_override: Option<PathBuf> = None;
    let mut rest = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" if rest.is_empty() => {
                wants_help = true;
                index += 1;
            }
            "--version" | "-V" => {
                wants_version = true;
                index += 1;
            }
            "--update" if rest.is_empty() => {
                rest.push("update".to_string());
                index += 1;
            }
            "--model" => {
                let value = next_value(args, index, "--model")?;
                model = resolve_model_alias(value);
                model_pinned = true;
                index += 2;
            }
            flag if flag.starts_with("--model=") => {
                model = resolve_model_alias(&flag[8..]);
                model_pinned = true;
                index += 1;
            }
            "--session-id" => {
                let value = next_value(args, index, "--session-id")?;
                session_id_override = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--session-id=") => {
                session_id_override = Some(flag[13..].to_string());
                index += 1;
            }
            // `update --check` / `doctor --check` keep their bare boolean
            // meaning (read-only check mode) — the value-taking objective
            // check applies to prompt runs only, so once one of those
            // subcommands is on `rest` this flag falls through to them.
            "--check"
                if !rest
                    .iter()
                    .any(|token| token == "update" || token == "doctor") =>
            {
                let value = next_value(args, index, "--check")?;
                check_command = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--check=") => {
                check_command = Some(flag[8..].to_string());
                index += 1;
            }
            "--fallback-model" => {
                let value = next_value(args, index, "--fallback-model")?;
                fallback_model = Some(resolve_model_alias(value));
                index += 2;
            }
            flag if flag.starts_with("--fallback-model=") => {
                fallback_model = Some(resolve_model_alias(&flag[17..]));
                index += 1;
            }
            "--output-format" => {
                let value = next_value(args, index, "--output-format")?;
                output_format = CliOutputFormat::parse(value)?;
                index += 2;
            }
            "--permission-mode" => {
                let value = next_value(args, index, "--permission-mode")?;
                permission_mode_override = Some(parse_permission_mode_arg(value)?);
                index += 2;
            }
            flag if flag.starts_with("--output-format=") => {
                output_format = CliOutputFormat::parse(&flag[16..])?;
                index += 1;
            }
            flag if flag.starts_with("--permission-mode=") => {
                permission_mode_override = Some(parse_permission_mode_arg(&flag[18..])?);
                index += 1;
            }
            "--dangerously-skip-permissions" => {
                permission_mode_override = Some(PermissionMode::DangerFullAccess);
                index += 1;
            }
            "-p" => {
                if fork_session {
                    return Err("--fork-session requires --resume".to_string());
                }
                if inline {
                    return Err(
                        "--inline is only supported by the main interactive command".to_string(),
                    );
                }
                // Claude Code compat: -p "prompt" = one-shot prompt. For CC SDK
                // parity, allow format/budget flags after `-p` as well so
                // `zo -p --input-format stream-json` reads stdin instead of
                // treating the flags as prompt text.
                let mut prompt_parts = Vec::new();
                let mut tail = index + 1;
                while tail < args.len() {
                    match args[tail].as_str() {
                        "--input-format" => {
                            let value = next_value(args, tail, "--input-format")?;
                            input_format = CliInputFormat::parse(value)?;
                            tail += 2;
                        }
                        flag if flag.starts_with("--input-format=") => {
                            input_format = CliInputFormat::parse(&flag[15..])?;
                            tail += 1;
                        }
                        "--output-format" => {
                            let value = next_value(args, tail, "--output-format")?;
                            output_format = CliOutputFormat::parse(value)?;
                            tail += 2;
                        }
                        flag if flag.starts_with("--output-format=") => {
                            output_format = CliOutputFormat::parse(&flag[16..])?;
                            tail += 1;
                        }
                        "--max-turns" => {
                            let value = next_value(args, tail, "--max-turns")?;
                            max_turns = Some(parse_positive_usize_flag("--max-turns", value)?);
                            tail += 2;
                        }
                        flag if flag.starts_with("--max-turns=") => {
                            max_turns =
                                Some(parse_positive_usize_flag("--max-turns", &flag[12..])?);
                            tail += 1;
                        }
                        "--inline" => {
                            return Err(
                                "--inline is only supported by the main interactive command"
                                    .to_string(),
                            );
                        }
                        other => {
                            prompt_parts.push(other.to_string());
                            tail += 1;
                        }
                    }
                }
                let prompt = prompt_parts.join(" ");
                if prompt.trim().is_empty() && input_format == CliInputFormat::Text {
                    return Err("-p requires a prompt string".to_string());
                }
                if system_prompt_override.is_some() && append_system_prompt.is_some() {
                    return Err(
                        "--system-prompt and --append-system-prompt cannot be used together"
                            .to_string(),
                    );
                }
                return Ok(CliAction::Prompt {
                    prompt,
                    model: resolve_model_alias(&model),
                    model_pinned,
                    output_format,
                    allowed_tools: normalize_allowed_tools(&allowed_tool_values)?,
                    disallowed_tools: normalize_disallowed_tools(&disallowed_tool_values)?,
                    permission_mode: permission_mode_override
                        .unwrap_or_else(default_permission_mode),
                    max_turns,
                    max_tool_calls,
                    system_prompt: system_prompt_override,
                    append_system_prompt,
                    verbose,
                    input_format,
                    mcp_config,
                    prefill: prefill.clone(),
                    no_follow,
                    session_id: session_id_override,
                    fallback_model,
                    check_command,
                });
            }
            "--print" => {
                // Claude Code compat: --print makes output non-interactive
                output_format = CliOutputFormat::Text;
                index += 1;
            }
            "--resume" if rest.is_empty() => {
                rest.push("--resume".to_string());
                index += 1;
            }
            "--fork-session" => {
                fork_session = true;
                index += 1;
            }
            flag if rest.is_empty() && flag.starts_with("--resume=") => {
                rest.push("--resume".to_string());
                rest.push(flag[9..].to_string());
                index += 1;
            }
            "--allowedTools" | "--allowed-tools" => {
                let value = next_value(args, index, "--allowedTools")?;
                allowed_tool_values.push(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--allowedTools=") => {
                allowed_tool_values.push(flag[15..].to_string());
                index += 1;
            }
            flag if flag.starts_with("--allowed-tools=") => {
                allowed_tool_values.push(flag[16..].to_string());
                index += 1;
            }
            "--disallowedTools" | "--disallowed-tools" => {
                let value = next_value(args, index, "--disallowedTools")?;
                disallowed_tool_values.push(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--disallowedTools=") => {
                disallowed_tool_values.push(flag[18..].to_string());
                index += 1;
            }
            flag if flag.starts_with("--disallowed-tools=") => {
                disallowed_tool_values.push(flag[19..].to_string());
                index += 1;
            }
            "--max-turns" => {
                let value = next_value(args, index, "--max-turns")?;
                max_turns = Some(parse_positive_usize_flag("--max-turns", value)?);
                index += 2;
            }
            flag if flag.starts_with("--max-turns=") => {
                let value = &flag[12..];
                max_turns = Some(parse_positive_usize_flag("--max-turns", value)?);
                index += 1;
            }
            "--max-tool-calls" => {
                let value = next_value(args, index, "--max-tool-calls")?;
                max_tool_calls = Some(parse_positive_usize_flag("--max-tool-calls", value)?);
                index += 2;
            }
            flag if flag.starts_with("--max-tool-calls=") => {
                let value = &flag[17..];
                max_tool_calls = Some(parse_positive_usize_flag("--max-tool-calls", value)?);
                index += 1;
            }
            "--system-prompt" => {
                let value = next_value(args, index, "--system-prompt")?;
                system_prompt_override = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--system-prompt=") => {
                system_prompt_override = Some(flag[16..].to_string());
                index += 1;
            }
            "--append-system-prompt" => {
                let value = next_value(args, index, "--append-system-prompt")?;
                append_system_prompt = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--append-system-prompt=") => {
                append_system_prompt = Some(flag[23..].to_string());
                index += 1;
            }
            "--continue" => {
                rest.push("--resume".to_string());
                rest.push("latest".to_string());
                index += 1;
            }
            "--verbose" => {
                verbose = true;
                index += 1;
            }
            "--input-format" => {
                let value = next_value(args, index, "--input-format")?;
                input_format = CliInputFormat::parse(value)?;
                index += 2;
            }
            flag if flag.starts_with("--input-format=") => {
                input_format = CliInputFormat::parse(&flag[15..])?;
                index += 1;
            }
            "--mcp-config" => {
                let value = next_value(args, index, "--mcp-config")?;
                mcp_config = Some(PathBuf::from(value));
                index += 2;
            }
            flag if flag.starts_with("--mcp-config=") => {
                mcp_config = Some(PathBuf::from(&flag[13..]));
                index += 1;
            }
            "--prefill" => {
                let value = next_value(args, index, "--prefill")?;
                prefill = Some(value.clone());
                index += 2;
            }
            flag if flag.starts_with("--prefill=") => {
                prefill = Some(flag[10..].to_string());
                index += 1;
            }
            "--no-follow" => {
                no_follow = true;
                index += 1;
            }
            "--inline" => {
                inline = true;
                index += 1;
            }
            "--fullscreen" => {
                fullscreen = true;
                index += 1;
            }
            "--cwd" if rest.is_empty() => {
                let value = next_value(args, index, "--cwd")?;
                _cwd_override = Some(PathBuf::from(value));
                index += 2;
            }
            flag if rest.is_empty() && flag.starts_with("--cwd=") => {
                _cwd_override = Some(PathBuf::from(&flag[6..]));
                index += 1;
            }
            other if rest.is_empty() && other.starts_with('-') => {
                return Err(format_unknown_option(other));
            }
            other => {
                rest.push(other.to_string());
                index += 1;
            }
        }
    }

    if wants_help {
        return Ok(CliAction::Help);
    }

    if wants_version {
        return Ok(CliAction::Version);
    }

    if system_prompt_override.is_some() && append_system_prompt.is_some() {
        return Err(
            "--system-prompt and --append-system-prompt cannot be used together".to_string(),
        );
    }

    let allowed_tools = normalize_allowed_tools(&allowed_tool_values)?;
    let disallowed_tools = normalize_disallowed_tools(&disallowed_tool_values)?;

    if rest.is_empty() {
        if fork_session {
            return Err("--fork-session requires --resume".to_string());
        }
        // The bare `zo` entry is the interactive TUI: route the cwd fallback
        // through the trust gate so a first visit to an untrusted folder is
        // asked about (CC parity). An explicit `--permission-mode`/env override
        // bypasses this (the `unwrap_or_else` closure never runs).
        let trust_inline = resolved_tui_mode_is_inline(inline, fullscreen);
        let permission_mode = permission_mode_override
            .unwrap_or_else(|| interactive_default_permission_mode(trust_inline));
        return Ok(CliAction::Repl {
            model,
            model_pinned,
            allowed_tools,
            disallowed_tools,
            permission_mode,
            max_turns,
            max_tool_calls,
            system_prompt: system_prompt_override,
            append_system_prompt,
            verbose,
            mcp_config,
            inline,
            fullscreen,
        });
    }
    if inline {
        return Err("--inline is only supported by the main interactive command".to_string());
    }
    if fullscreen {
        return Err("--fullscreen is only supported by the main interactive command".to_string());
    }
    if rest.first().map(String::as_str) == Some("--resume") {
        return parse_resume_args(&rest[1..], fork_session);
    }
    if fork_session {
        return Err("--fork-session requires --resume".to_string());
    }
    if let Some(action) = parse_single_word_command_alias(&rest, &model, permission_mode_override) {
        return action;
    }

    let permission_mode = permission_mode_override.unwrap_or_else(default_permission_mode);

    match rest[0].as_str() {
        "dump-manifests" => Ok(CliAction::DumpManifests),
        "bootstrap-plan" => Ok(CliAction::BootstrapPlan),
        "agents" => Ok(CliAction::Agents {
            args: join_optional_args(&rest[1..]),
        }),
        "mcp" => Ok(CliAction::Mcp {
            args: join_optional_args(&rest[1..]),
        }),
        "skills" => Ok(CliAction::Skills {
            args: join_optional_args(&rest[1..]),
        }),
        // Only an explicit `--model` selects the dumped `Model family:` line;
        // the unpinned default would just restate the built-in fallback.
        "system-prompt" => parse_system_prompt_args(&rest[1..], model_pinned.then_some(model)),
        "login" => Ok(CliAction::Login {
            provider: rest.get(1).cloned(),
        }),
        "logout" => Ok(CliAction::Logout),
        "init" => Ok(CliAction::Init),
        "doctor" => parse_doctor_args(&rest[1..]),
        "update" => parse_update_args(&rest[1..]),
        "__self-update-background" if rest.len() == 1 => Ok(CliAction::BackgroundUpdate),
        "serve" => parse_serve_args(&rest[1..], model, allowed_tools, permission_mode),
        "acp" => parse_acp_args(&rest[1..], model, allowed_tools, permission_mode),
        "attach" => parse_attach_args(&rest[1..]),
        "prompt" => {
            let prompt = rest[1..].join(" ");
            if prompt.trim().is_empty() {
                return Err("prompt subcommand requires a prompt string".to_string());
            }
            Ok(CliAction::Prompt {
                prompt,
                model,
                model_pinned,
                output_format,
                allowed_tools,
                disallowed_tools,
                permission_mode,
                max_turns,
                max_tool_calls,
                system_prompt: system_prompt_override,
                append_system_prompt,
                verbose,
                input_format,
                mcp_config,
                prefill: prefill.clone(),
                no_follow,
                session_id: session_id_override.clone(),
                fallback_model: fallback_model.clone(),
                check_command: check_command.clone(),
            })
        }
        other if other.starts_with('/') => {
            parse_direct_slash_cli_action(&rest, model, allowed_tools, permission_mode)
        }
        _other => Ok(CliAction::Prompt {
            prompt: rest.join(" "),
            model,
            model_pinned,
            output_format,
            allowed_tools,
            disallowed_tools,
            permission_mode,
            max_turns,
            max_tool_calls,
            system_prompt: system_prompt_override,
            append_system_prompt,
            verbose,
            input_format,
            mcp_config,
            prefill,
            no_follow,
            session_id: session_id_override,
            fallback_model,
            check_command,
        }),
    }
}

/// Resolve the `--bind <addr>` / `--port <n>` flags shared by the `serve` and
/// `attach` subcommands into one `host:port` string, collecting any non-flag
/// positional arguments (e.g. an attach session id) into the returned vec.
///
/// Precedence: an explicit `--bind` wins; otherwise `--port` sets the port on
/// loopback; otherwise [`DEFAULT_BIND_ADDR`](crate::serve_protocol::DEFAULT_BIND_ADDR).
/// Unknown flags are rejected so a typo fails loudly instead of being dropped.
fn parse_bind_and_positionals(args: &[String]) -> Result<(String, Vec<String>), String> {
    let mut bind: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--bind" => {
                bind = Some(next_value(args, index, "--bind")?.clone());
                index += 2;
            }
            flag if flag.starts_with("--bind=") => {
                bind = Some(flag[7..].to_string());
                index += 1;
            }
            "--port" => {
                let value = next_value(args, index, "--port")?;
                port = Some(parse_port(value)?);
                index += 2;
            }
            flag if flag.starts_with("--port=") => {
                port = Some(parse_port(&flag[7..])?);
                index += 1;
            }
            other if other.starts_with('-') => return Err(format_unknown_option(other)),
            other => {
                positionals.push(other.to_string());
                index += 1;
            }
        }
    }
    let bind_addr = match (bind, port) {
        (Some(addr), _) => addr,
        (None, Some(port)) => format!("127.0.0.1:{port}"),
        (None, None) => crate::serve_protocol::DEFAULT_BIND_ADDR.to_string(),
    };
    Ok((bind_addr, positionals))
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| format!("invalid value for --port: {value} (expected 1..=65535)"))
}

/// Parse `zo serve [--bind ADDR] [--port N]`.
fn parse_serve_args(
    args: &[String],
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<CliAction, String> {
    let (bind_addr, positionals) = parse_bind_and_positionals(args)?;
    if let Some(extra) = positionals.first() {
        return Err(format!(
            "unexpected argument to serve: {extra} (usage: zo serve [--bind ADDR] [--port N])"
        ));
    }
    Ok(CliAction::Serve {
        bind_addr,
        model,
        allowed_tools,
        permission_mode,
    })
}

/// Doctor-specific usage, shown for `zo doctor --help` / `zo doctor -h`.
pub(crate) const DOCTOR_HELP: &str = "\
zo doctor — diagnose the local environment and apply only safe, reversible repairs

Usage: zo doctor [--check]

By default, `zo doctor` diagnoses the environment and automatically applies safe,
reversible local repairs (creating missing Zo-owned config/state directories and
tightening owner-only permissions on Zo-owned directories and settings files).
It never edits configuration content, auth credentials, PATH, or MCP servers, and
makes no network requests.

Options:
  --check      Strictly read-only: diagnose without mutating the filesystem.
  -h, --help   Show this help.";

/// Parse `zo doctor [--check]`. Default mode diagnoses and applies only safe,
/// reversible local repairs; `--check` selects strictly read-only diagnosis.
/// `--help` / `-h` route to doctor-specific usage.
fn parse_doctor_args(args: &[String]) -> Result<CliAction, String> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => return Ok(CliAction::HelpText(DOCTOR_HELP.to_string())),
            "--check" => check = true,
            other => {
                return Err(format!(
                    "unexpected argument to doctor: {other} (usage: zo doctor [--check])"
                ));
            }
        }
    }
    Ok(CliAction::Doctor { check })
}

pub(crate) const UPDATE_HELP: &str = "\
zo update — update an installer-managed official stable release

Usage: zo update [--check]

Options:
  --check      Check for a newer release without installing it.
  -h, --help   Show this help.";

fn parse_update_args(args: &[String]) -> Result<CliAction, String> {
    let mut check = false;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => return Ok(CliAction::HelpText(UPDATE_HELP.to_string())),
            "--check" => check = true,
            other => {
                return Err(format!(
                    "unexpected argument to update: {other} (usage: zo update [--check])"
                ));
            }
        }
    }
    Ok(CliAction::Update { check })
}

fn parse_acp_args(
    args: &[String],
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<CliAction, String> {
    if let Some(extra) = args.first() {
        return Err(format!(
            "unexpected argument to acp: {extra} (usage: zo acp)"
        ));
    }
    Ok(CliAction::Acp {
        model,
        allowed_tools,
        permission_mode,
    })
}

/// Parse `zo attach [SESSION_ID] [--plain] [--bind ADDR] [--port N]`. With no
/// session id, the client creates a fresh session on the server and attaches to
/// it. `--plain` selects the line client instead of the default rich TUI.
fn parse_attach_args(args: &[String]) -> Result<CliAction, String> {
    // `--plain` is attach-specific (not a serve/bind flag), so peel it off
    // before the shared bind/positional parse rejects it as unknown.
    let mut plain = false;
    let mut rest: Vec<String> = Vec::with_capacity(args.len());
    for arg in args {
        if arg == "--plain" {
            plain = true;
        } else {
            rest.push(arg.clone());
        }
    }
    let (bind_addr, positionals) = parse_bind_and_positionals(&rest)?;
    if positionals.len() > 1 {
        return Err(format!(
            "attach takes at most one session id (got {}): zo attach [SESSION_ID] [--plain] [--bind ADDR]",
            positionals.len()
        ));
    }
    Ok(CliAction::Attach {
        bind_addr,
        session_id: positionals.into_iter().next(),
        plain,
    })
}

/// Extract `--cwd <path>` from the argument list without full parsing.
/// Called before `parse_args` so the working directory is set before
/// any action that depends on it. Only matches `--cwd` that appears
/// before any positional/subcommand argument (i.e. only top-level usage).
pub(crate) fn extract_cwd_override(args: &[String]) -> Option<PathBuf> {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--cwd" => return args.get(index + 1).map(PathBuf::from),
            flag if flag.starts_with("--cwd=") => return Some(PathBuf::from(&flag[6..])),
            arg if !arg.starts_with('-') => return None, // hit a positional arg; stop
            flag if flag == "--model"
                || flag == "--output-format"
                || flag == "--permission-mode"
                || flag == "--allowedTools"
                || flag == "--allowed-tools"
                || flag == "--disallowedTools"
                || flag == "--disallowed-tools"
                || flag == "--max-turns"
                || flag == "--max-tool-calls"
                || flag == "--system-prompt"
                || flag == "--append-system-prompt"
                || flag == "--input-format"
                || flag == "--mcp-config"
                || flag == "--prefill" =>
            {
                index += 2; // skip flag + value
            }
            _ => index += 1, // boolean flags like --verbose, --no-follow, --print, etc.
        }
    }
    None
}

pub(crate) fn parse_single_word_command_alias(
    rest: &[String],
    model: &str,
    permission_mode_override: Option<PermissionMode>,
) -> Option<Result<CliAction, String>> {
    if rest.len() != 1 {
        return None;
    }

    match rest[0].as_str() {
        "help" => Some(Ok(CliAction::Help)),
        "version" => Some(Ok(CliAction::Version)),
        "update" => Some(Ok(CliAction::Update { check: false })),
        "status" => Some(Ok(CliAction::Status {
            model: model.to_string(),
            permission_mode: permission_mode_override.unwrap_or_else(default_permission_mode),
        })),
        "sandbox" => Some(Ok(CliAction::Sandbox)),
        other => bare_slash_command_guidance(other).map(Err),
    }
}

pub(crate) fn bare_slash_command_guidance(command_name: &str) -> Option<String> {
    if matches!(
        command_name,
        "dump-manifests"
            | "bootstrap-plan"
            | "agents"
            | "mcp"
            | "skills"
            | "system-prompt"
            | "login"
            | "logout"
            | "init"
            | "doctor"
            | "update"
            | "__self-update-background"
            | "prompt"
    ) {
        return None;
    }
    let slash_command = slash_command_specs()
        .iter()
        .find(|spec| spec.name == command_name)?;
    let guidance = if slash_command.resume_supported {
        format!(
            "`zo {command_name}` is a slash command. Use `zo --resume SESSION.jsonl /{command_name}` or start `zo` and run `/{command_name}`."
        )
    } else {
        format!(
            "`zo {command_name}` is a slash command. Start `zo` and run `/{command_name}` inside the REPL."
        )
    };
    Some(guidance)
}

pub(crate) fn join_optional_args(args: &[String]) -> Option<String> {
    let joined = args.join(" ");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(crate) fn parse_direct_slash_cli_action(
    rest: &[String],
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<CliAction, String> {
    let raw = rest.join(" ");
    match SlashCommand::parse(&raw) {
        Ok(Some(SlashCommand::Help)) => Ok(CliAction::Help),
        Ok(Some(SlashCommand::Agents { args })) => Ok(CliAction::Agents { args }),
        Ok(Some(SlashCommand::Mcp { action, target })) => Ok(CliAction::Mcp {
            args: match (action, target) {
                (None, None) => None,
                (Some(action), None) => Some(action),
                (Some(action), Some(target)) => Some(format!("{action} {target}")),
                (None, Some(target)) => Some(target),
            },
        }),
        Ok(Some(SlashCommand::Skills { args })) => Ok(CliAction::Skills { args }),
        // Route `/doctor` to the standalone engine (default repair mode) rather
        // than the generic `SlashCommand` path, so it runs before `LiveCli`
        // startup — a missing auth or session must not fail the diagnosis.
        Ok(Some(SlashCommand::Doctor)) => Ok(CliAction::Doctor { check: false }),
        Ok(Some(SlashCommand::Unknown { name, .. })) => {
            Err(format_unknown_direct_slash_command(&name))
        }
        Ok(Some(command)) => Ok(CliAction::SlashCommand {
            command,
            model,
            allowed_tools,
            permission_mode,
        }),
        Ok(None) => Err(format!("unknown subcommand: {}", rest[0])),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn format_unknown_option(option: &str) -> String {
    let mut message = format!("unknown option: {option}");
    if let Some(suggestion) = suggest_closest_term(option, CLI_OPTION_SUGGESTIONS) {
        message.push_str("\nDid you mean ");
        message.push_str(suggestion);
        message.push('?');
    }
    message.push_str("\nRun `zo --help` for usage.");
    message
}

fn parse_positive_usize_flag(flag: &str, value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {flag}: {value} (expected a positive integer)"))?;
    if parsed == 0 {
        return Err(format!(
            "invalid value for {flag}: {value} (expected a positive integer)"
        ));
    }
    Ok(parsed)
}

fn parse_u32_flag(flag: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("invalid value for {flag}: {value} (expected a non-negative integer)"))
}

pub(crate) fn format_unknown_direct_slash_command(name: &str) -> String {
    let mut message = format!("unknown slash command outside the REPL: /{name}");
    if let Some(suggestions) = render_suggestion_line("Did you mean", &suggest_slash_commands(name))
    {
        message.push('\n');
        message.push_str(&suggestions);
    }
    message.push_str("\nRun `zo --help` for CLI usage, or start `zo` and use /help.");
    message
}

pub(crate) fn format_unknown_slash_command(name: &str) -> String {
    let mut message = format!("Unknown slash command: /{name}");
    if let Some(suggestions) = render_suggestion_line("Did you mean", &suggest_slash_commands(name))
    {
        message.push('\n');
        message.push_str(&suggestions);
    }
    message.push_str("\n  Help             /help lists available slash commands");
    message
}

pub(crate) fn render_suggestion_line(label: &str, suggestions: &[String]) -> Option<String> {
    (!suggestions.is_empty()).then(|| format!("  {label:<16} {}", suggestions.join(", "),))
}

pub(crate) fn suggest_slash_commands(input: &str) -> Vec<String> {
    let mut candidates = public_slash_command_specs()
        .into_iter()
        .flat_map(|spec| {
            std::iter::once(spec.name)
                .chain(spec.aliases.iter().copied())
                .map(|name| format!("/{name}"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    let candidate_refs = candidates.iter().map(String::as_str).collect::<Vec<_>>();
    ranked_suggestions(input.trim_start_matches('/'), &candidate_refs)
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(crate) fn suggest_closest_term<'a>(input: &str, candidates: &'a [&'a str]) -> Option<&'a str> {
    ranked_suggestions(input, candidates).into_iter().next()
}

pub(crate) fn ranked_suggestions<'a>(input: &str, candidates: &'a [&'a str]) -> Vec<&'a str> {
    let normalized_input = input.trim_start_matches('/').to_ascii_lowercase();
    let mut ranked = candidates
        .iter()
        .filter_map(|candidate| {
            let normalized_candidate = candidate.trim_start_matches('/').to_ascii_lowercase();
            let distance =
                core_types::text::levenshtein_distance(&normalized_input, &normalized_candidate);
            let prefix_bonus = usize::from(
                !(normalized_candidate.starts_with(&normalized_input)
                    || normalized_input.starts_with(&normalized_candidate)),
            );
            let score = distance + prefix_bonus;
            (score <= 4).then_some((score, *candidate))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.cmp(right).then_with(|| left.1.cmp(right.1)));
    ranked
        .into_iter()
        .map(|(_, candidate)| candidate)
        .take(3)
        .collect()
}

/// Resolve a model alias to its canonical id.
///
/// Delegates to [`api::resolve_model_alias`] so the CLI and the API client
/// share a single source of truth (the provider catalog). The catalog covers
/// the bare/`claude-` prefixed aliases and dot-versioned id normalisation that
/// this used to duplicate.
pub(crate) fn resolve_model_alias(model: &str) -> String {
    api::resolve_model_alias(model)
}

pub(crate) fn normalize_allowed_tools(values: &[String]) -> Result<Option<AllowedToolSet>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    crate::current_tool_registry()?
        .normalize_allowed_tools(values)
        .map_err(|e| e.to_string())
}

pub(crate) fn normalize_disallowed_tools(
    values: &[String],
) -> Result<Option<DisallowedToolSet>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    crate::current_tool_registry()?
        .normalize_allowed_tools(values)
        .map_err(|error| error.to_string().replace("--allowedTools", "--disallowedTools"))
}

pub(crate) fn parse_permission_mode_arg(value: &str) -> Result<PermissionMode, String> {
    let normalized = crate::normalize_permission_mode(value).ok_or_else(|| {
        format!(
            "unsupported permission mode '{value}'. Use read-only, workspace-write, danger-full-access, default, acceptEdits, plan, or bypassPermissions."
        )
    })?;
    if normalized != value.trim() {
        eprintln!(
            "[zo] permission mode '{}' maps to '{normalized}'",
            value.trim()
        );
    }
    Ok(crate::permission_mode_from_label(normalized))
}

/// `model` is the globally parsed `--model` (already alias-resolved), or `None`
/// when the user did not pin one — the flag is consumed before subcommand
/// dispatch, so it is threaded in rather than re-parsed here.
pub(crate) fn parse_system_prompt_args(
    args: &[String],
    model: Option<String>,
) -> Result<CliAction, String> {
    let mut cwd = env::current_dir().map_err(|error| error.to_string())?;
    let mut date = default_prompt_date();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--cwd" => {
                let value = next_value(args, index, "--cwd")?;
                cwd = PathBuf::from(value);
                index += 2;
            }
            "--date" => {
                let value = next_value(args, index, "--date")?;
                date.clone_from(value);
                index += 2;
            }
            other => return Err(format!("unknown system-prompt option: {other}")),
        }
    }

    Ok(CliAction::PrintSystemPrompt { cwd, date, model })
}

pub(crate) fn parse_resume_args(
    args: &[String],
    fork_session: bool,
) -> Result<CliAction, String> {
    let mut from_turn = None;
    let mut filtered = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--from-turn" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --from-turn".to_string())?;
                from_turn = Some(parse_u32_flag("--from-turn", value)?);
                index += 2;
            }
            flag if flag.starts_with("--from-turn=") => {
                from_turn = Some(parse_u32_flag("--from-turn", &flag[12..])?);
                index += 1;
            }
            other => {
                filtered.push(other.to_string());
                index += 1;
            }
        }
    }

    let (session_path, command_tokens): (PathBuf, &[String]) = match filtered.first() {
        None => (PathBuf::from(crate::LATEST_SESSION_REFERENCE), &[]),
        Some(first) if looks_like_slash_command_token(first) => (
            PathBuf::from(crate::LATEST_SESSION_REFERENCE),
            filtered.as_slice(),
        ),
        Some(first) => (PathBuf::from(first), &filtered[1..]),
    };
    let mut commands = Vec::new();
    let mut current_command = String::new();

    for token in command_tokens {
        if token.trim_start().starts_with('/') {
            if resume_command_can_absorb_token(&current_command, token) {
                current_command.push(' ');
                current_command.push_str(token);
                continue;
            }
            if !current_command.is_empty() {
                commands.push(current_command);
            }
            current_command = String::from(token.as_str());
            continue;
        }

        if current_command.is_empty() {
            return Err("--resume trailing arguments must be slash commands".to_string());
        }

        current_command.push(' ');
        current_command.push_str(token);
    }

    if !current_command.is_empty() {
        commands.push(current_command);
    }

    Ok(CliAction::ResumeSession {
        session_path,
        from_turn,
        fork_session,
        commands,
    })
}

pub(crate) fn resume_command_can_absorb_token(current_command: &str, token: &str) -> bool {
    matches!(
        SlashCommand::parse(current_command),
        Ok(Some(SlashCommand::Export { path: None }))
    ) && !looks_like_slash_command_token(token)
}

pub(crate) fn looks_like_slash_command_token(token: &str) -> bool {
    let trimmed = token.trim_start();
    let Some(name) = trimmed.strip_prefix('/').and_then(|value| {
        value
            .split_whitespace()
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }) else {
        return false;
    };

    slash_command_specs()
        .iter()
        .any(|spec| spec.name == name || spec.aliases.contains(&name))
}

#[cfg(test)]
mod update_parse_tests {
    use super::{parse_args, CliAction, UPDATE_HELP};

    #[test]
    fn update_and_check_parse_without_prompt_fallback() {
        assert_eq!(
            parse_args(&["update".to_string()]).expect("update should parse"),
            CliAction::Update { check: false }
        );
        assert_eq!(
            parse_args(&["update".to_string(), "--check".to_string()])
                .expect("update check should parse"),
            CliAction::Update { check: true }
        );
        assert_eq!(
            parse_args(&["--update".to_string()]).expect("update alias should parse"),
            CliAction::Update { check: false }
        );
    }

    #[test]
    fn update_help_is_subcommand_specific() {
        assert_eq!(
            parse_args(&["update".to_string(), "--help".to_string()])
                .expect("update help should parse"),
            CliAction::HelpText(UPDATE_HELP.to_string())
        );
    }
}

#[cfg(test)]
mod acp_parse_tests {
    use runtime::PermissionMode;

    use super::{CliAction, parse_acp_args};

    #[test]
    fn acp_uses_resolved_global_configuration() {
        let action = parse_acp_args(
            &[],
            "resolved-model".to_string(),
            None,
            PermissionMode::WorkspaceWrite,
        )
        .expect("ACP arguments should parse");
        match action {
            CliAction::Acp {
                model,
                allowed_tools,
                permission_mode,
            } => {
                assert_eq!(model, "resolved-model");
                assert!(allowed_tools.is_none());
                assert_eq!(permission_mode, PermissionMode::WorkspaceWrite);
            }
            other => panic!("expected ACP action, got {other:?}"),
        }
    }

    #[test]
    fn acp_rejects_positional_arguments() {
        let error = parse_acp_args(
            &["extra".to_string()],
            "model".to_string(),
            None,
            PermissionMode::Prompt,
        )
        .expect_err("ACP should reject positional arguments");
        assert_eq!(
            error,
            "unexpected argument to acp: extra (usage: zo acp)"
        );
    }
}

#[cfg(test)]
mod format_parse_tests {
    use super::{CliInputFormat, CliOutputFormat};

    #[test]
    fn output_format_accepts_text_json_and_streaming_aliases() {
        assert_eq!(CliOutputFormat::parse("text"), Ok(CliOutputFormat::Text));
        assert_eq!(CliOutputFormat::parse("json"), Ok(CliOutputFormat::Json));
        // `stream-json` is the Claude Code parity name; `ndjson` is the
        // Zo alias. Both resolve to the same newline-delimited stream.
        assert_eq!(
            CliOutputFormat::parse("stream-json"),
            Ok(CliOutputFormat::Ndjson)
        );
        assert_eq!(
            CliOutputFormat::parse("ndjson"),
            Ok(CliOutputFormat::Ndjson)
        );
    }

    /// The mode split the headless output contract keys off: both JSON shapes
    /// are parsed by a program, `text` is prose a person may be watching.
    #[test]
    fn only_the_json_output_formats_are_machine_consumed() {
        assert!(CliOutputFormat::Json.machine_consumed());
        assert!(CliOutputFormat::Ndjson.machine_consumed());
        assert!(
            !CliOutputFormat::Text.machine_consumed(),
            "headless text prints prose for a human reader"
        );
    }

    #[test]
    fn output_format_rejects_unknown_value_with_expected_list() {
        let err = CliOutputFormat::parse("yaml").expect_err("yaml is not a format");
        assert!(err.contains("--output-format"), "message: {err}");
        assert!(err.contains("text, json, or ndjson"), "message: {err}");
    }

    #[test]
    fn input_format_accepts_text_and_streaming_aliases() {
        assert_eq!(CliInputFormat::parse("text"), Ok(CliInputFormat::Text));
        for value in ["stream-json", "json", "ndjson"] {
            assert_eq!(CliInputFormat::parse(value), Ok(CliInputFormat::StreamJson));
        }
    }

    #[test]
    fn input_format_rejects_unknown_value_with_expected_text() {
        let err = CliInputFormat::parse("xml").expect_err("xml is not a format");
        assert!(
            err.contains("unsupported value for --input-format"),
            "message: {err}"
        );
        assert!(
            err.contains("expected text or stream-json"),
            "message: {err}"
        );
    }
}

#[cfg(test)]
mod suggestion_coverage_tests {
    use super::{format_unknown_option, parse_args, CliAction, CLI_OPTION_SUGGESTIONS};

    /// Every long flag the parser accepts, so a typo of any of them lands in the
    /// did-you-mean set. This list is the source of truth for the guard below:
    /// adding a new parsed flag here forces a matching entry in
    /// [`CLI_OPTION_SUGGESTIONS`], so a flag can never silently drift out of the
    /// suggestion set (the bug this test was written to prevent). Short aliases
    /// (`-h`, `-V`, `-p`) are intentionally omitted — Levenshtein suggestions key
    /// off the long spellings.
    const PARSED_LONG_FLAGS: &[&str] = &[
        "--help",
        "--version",
        "--update",
        "--model",
        "--output-format",
        "--permission-mode",
        "--dangerously-skip-permissions",
        "--allowedTools",
        "--allowed-tools",
        "--disallowedTools",
        "--disallowed-tools",
        "--resume",
        "--continue",
        "--print",
        "--max-turns",
        "--max-tool-calls",
        "--system-prompt",
        "--append-system-prompt",
        "--verbose",
        "--input-format",
        "--mcp-config",
        "--prefill",
        "--no-follow",
        "--cwd",
        "--bind",
        "--port",
        "--plain",
        // Process-wide overrides peeled off before the action parser.
        "--add-dir",
        "--settings",
        "--strict-mcp-config",
        // One-shot prompt parity flags.
        "--session-id",
        "--fallback-model",
        // Resume sub-parser.
        "--from-turn",
    ];

    #[test]
    fn every_parsed_flag_is_a_did_you_mean_suggestion() {
        for flag in PARSED_LONG_FLAGS {
            assert!(
                CLI_OPTION_SUGGESTIONS.contains(flag),
                "parsed flag {flag} is missing from CLI_OPTION_SUGGESTIONS; a typo of it \
                 would get no `Did you mean` hint. Add it to the suggestions array."
            );
        }
    }

    #[test]
    fn newly_added_flags_surface_in_unknown_option_hint() {
        // A near-miss typo of one of the previously-missing flags now resolves to
        // a concrete suggestion rather than falling through with no hint.
        for (typo, expected) in [
            ("--add-di", "--add-dir"),
            ("--sessionid", "--session-id"),
            ("--fallbackmodel", "--fallback-model"),
            ("--from-turns", "--from-turn"),
            ("--strict-mcp-configs", "--strict-mcp-config"),
        ] {
            let message = format_unknown_option(typo);
            assert!(
                message.contains("Did you mean"),
                "no suggestion offered for {typo}: {message}"
            );
            assert!(
                message.contains(expected),
                "expected {expected} suggested for {typo}, got: {message}"
            );
        }
    }

    /// `--check` is the first-class door for the headless objective check:
    /// both spellings parse onto `CliAction::Prompt::check_command`.
    #[test]
    fn check_flag_parses_in_both_spellings() {
        for argv in [
            vec!["--check".to_string(), "npm test".to_string(), "prompt".to_string(), "fix".to_string()],
            vec!["--check=npm test".to_string(), "prompt".to_string(), "fix".to_string()],
        ] {
            let action = parse_args(&argv).expect("must parse");
            let CliAction::Prompt { check_command, .. } = action else {
                panic!("expected a prompt action: {argv:?}");
            };
            assert_eq!(check_command.as_deref(), Some("npm test"), "{argv:?}");
        }
    }

    #[test]
    fn unknown_flag_before_positionals_is_rejected_with_suggestion() {
        // End-to-end through the parser: an unknown top-level flag should error
        // with the did-you-mean message rather than be swallowed.
        let err = parse_args(&["--add-di".to_string()])
            .expect_err("an unknown flag must not parse successfully");
        assert!(err.contains("--add-dir"), "message: {err}");
    }
}
