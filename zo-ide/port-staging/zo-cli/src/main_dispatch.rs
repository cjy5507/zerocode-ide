use crate::auth::{run_login_provider, run_logout};
use std::io::{self, BufRead, IsTerminal};

use crate::cli_args::{CliAction, CliInputFormat, CliOutputFormat};
use runtime::Session;
use crate::resume::resume_session;
use crate::session::{run_repl, LiveCli};
use crate::session_registry::SessionScope;
use crate::status_actions::{
    dump_manifests, print_bootstrap_plan, print_sandbox_status_snapshot, print_status_snapshot,
    print_system_prompt, print_version,
};
use crate::{print_help, run_init, DEFAULT_MODEL};
use zo_cli::tui::modals::Effort;

// No difficulty *label* rides along: the budget is one continuous score with
// two visible inputs (effort + prompt shape), and any single band word over
// that sum misattributes one input to the other — `-p "안녕"` under smart
// effort printed "(complex prompt)" purely because of the effort term. The
// operator note shows the effort token and the derived numbers instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeadlessExecutionBudget {
    max_turns: usize,
    max_tool_calls: usize,
}

/// Point the `Workflow` tool's resume cache out-of-tree for a one-shot run, so
/// a headless `zo -p …` leaves the target repo clean (the same rationale as
/// the ephemeral session scope). An explicit `ZO_WORKFLOW_STORE` wins.
fn redirect_workflow_cache_off_tree() {
    if std::env::var_os("ZO_WORKFLOW_STORE").is_some() {
        return;
    }
    if let Ok(cwd) = std::env::current_dir() {
        std::env::set_var(
            "ZO_WORKFLOW_STORE",
            crate::session_registry::ephemeral_workflow_store_dir(&cwd),
        );
    }
}

fn fork_resumed_session(
    session_path: &std::path::Path,
    from_turn: Option<u32>,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let resolved_path = if session_path.exists() {
        session_path.to_path_buf()
    } else {
        crate::resolve_session_reference(&session_path.display().to_string())?.path
    };
    let source = Session::load_from_path_from_turn(&resolved_path, from_turn)?;
    let forked = source.fork(None);
    let handle = crate::create_managed_session_handle(&forked.session_id, SessionScope::Project)?;
    forked
        .with_persistence_path(handle.path.clone())
        .save_to_path(&handle.path)?;
    Ok(handle.path)
}

// A flat dispatch over every `CliAction` variant; one arm per subcommand reads
// more clearly than splitting it across helpers.
#[allow(clippy::too_many_lines)] // flat CliAction dispatch, one arm per action
pub(crate) fn run_action(action: CliAction) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        CliAction::DumpManifests => dump_manifests(),
        CliAction::BootstrapPlan => print_bootstrap_plan(),
        CliAction::Agents { args } => LiveCli::print_agents(args.as_deref())?,
        CliAction::Mcp { args } => LiveCli::print_mcp(args.as_deref())?,
        CliAction::Skills { args } => LiveCli::print_skills(args.as_deref())?,
        CliAction::PrintSystemPrompt { cwd, date, model } => {
            print_system_prompt(cwd, date, model.as_deref());
        }
        CliAction::Version => print_version(),
        CliAction::Update { check } => crate::self_update::run_cli(check)?,
        CliAction::BackgroundUpdate => crate::self_update::run_background()?,
        CliAction::Doctor { check } => {
            let mode = if check {
                crate::doctor::DoctorMode::Check
            } else {
                crate::doctor::DoctorMode::Repair
            };
            let cwd = crate::current_cli_cwd()?;
            let report = crate::doctor::run(mode, &cwd);
            println!("{}", report.render());
        }
        CliAction::ResumeSession {
            session_path,
            from_turn,
            fork_session,
            commands,
        } => {
            if fork_session {
                let fork_path = fork_resumed_session(&session_path, from_turn)?;
                resume_session(&fork_path, None, &commands);
            } else {
                resume_session(&session_path, from_turn, &commands);
            }
        }
        CliAction::Status {
            model,
            permission_mode,
        } => print_status_snapshot(&model, permission_mode)?,
        CliAction::Sandbox => print_sandbox_status_snapshot()?,
        CliAction::Prompt {
            prompt,
            model,
            model_pinned,
            output_format,
            allowed_tools,
            disallowed_tools,
            permission_mode,
            max_turns,
            max_tool_calls,
            system_prompt,
            append_system_prompt,
            verbose,
            input_format,
            mcp_config,
            prefill,
            no_follow,
            session_id,
            fallback_model,
            check_command,
        } => {
            // `--check` is the first-class door for the headless objective
            // check; the env var stays as the internal seam (same flag->env
            // pattern as `redirect_workflow_cache_off_tree`). The flag wins
            // over a pre-set env so the invocation's contract is explicit.
            if let Some(check) = check_command
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                std::env::set_var("ZO_AUTO_VERIFY_CMD", check);
            }
            warn_unapplied_prompt_flags(verbose, no_follow);
            crate::runtime_support::install_cli_runtime_overrides(
                crate::runtime_support::CliRuntimeOverrides {
                    disallowed_tools,
                    ..crate::runtime_support::CliRuntimeOverrides::default()
                },
            );
            let full_prompt = match prefill {
                Some(pre) => format!("{pre}\n\n{prompt}"),
                None => prompt,
            };
            // A headless one-shot must leave the repo clean — send the
            // workflow resume cache out-of-tree too (see the helper).
            redirect_workflow_cache_off_tree();
            // Non-interactive one-shot: persist the session out-of-tree so a
            // benchmark/CI run leaves the target repo clean (no
            // `.zo/sessions/` in the working tree).
            let mut cli = LiveCli::new_scoped_with_mcp_config_and_session_id(
                model,
                true,
                allowed_tools,
                permission_mode,
                SessionScope::Ephemeral,
                mcp_config,
                session_id,
                crate::runtime_support::StartupAuthPolicy::Require,
            )?;
            cli.set_model_user_pinned(model_pinned);
            // Session start is the only place this may be set: the contract it
            // arms rides in the system prefix, and `--output-format` is fixed
            // for the life of the process, so both turn paths below (text
            // stdin-stream and one-shot) inherit one stable prefix.
            cli.set_machine_consumed_output(output_format.machine_consumed());
            // Extended thinking bills as (expensive) output tokens. A headless
            // one-shot defaults it OFF — analysis/summarization is quality-neutral
            // without it and it was the dominant cost driver vs. peer CLIs. Opt
            // back in for reasoning-heavy automation (e.g. the deep lane) with
            // `ZO_EFFORT=high|medium|max|...`, or pin `reasoningEffort` in the
            // project's `.zo/settings*.json` so an interactive choice carries
            // into `-p` runs. The interactive TUI keeps its High default and the
            // `/effort` picker.
            let persisted_effort = crate::session::project_effort_preference(&cli.cwd);
            let effort = headless_prompt_effort(&full_prompt, persisted_effort);
            if let Some(warning) = cli.set_effort(effort) {
                eprintln!("{warning}");
            }
            // Headless one-shots should be bounded by default, but a single
            // fixed cap is either wasteful for simple prompts or too tight for
            // dynamic workflows. Explicit flags win; otherwise derive the
            // smallest budget that fits the prompt shape.
            let budget = headless_execution_budget(&full_prompt, effort, max_turns, max_tool_calls);
            // Surface an auto-derived budget on stderr so a run that stops at the
            // cap is never a silent mystery — the operator sees the ceiling and
            // how to lift it. Skipped when both bounds were passed explicitly.
            if max_turns.is_none() || max_tool_calls.is_none() {
                eprintln!(
                    "[zo] effort {} | auto budget: --max-turns {} / \
                     --max-tool-calls {} — pass either flag to override.",
                    effort_token(effort),
                    budget.max_turns,
                    budget.max_tool_calls,
                );
            }
            cli.set_max_turns(Some(budget.max_turns));
            cli.set_max_tool_calls(Some(budget.max_tool_calls));
            cli.apply_system_prompt_overrides(system_prompt, append_system_prompt);
            // CC parity: a headless one-shot is still a session — SessionStart
            // before the turn, SessionEnd after, same as the interactive loop.
            cli.runtime.fire_lifecycle_hook(
                runtime::HookEvent::SessionStart,
                &serde_json::json!({ "source": "headless" }),
            );
            let turn_result = match input_format {
                CliInputFormat::Text => run_headless_turn_with_optional_fallback(
                    &mut cli,
                    &full_prompt,
                    output_format,
                    fallback_model.as_deref(),
                ),
                CliInputFormat::StreamJson => run_stream_json_input(
                    &mut cli,
                    &full_prompt,
                    output_format,
                    fallback_model.as_deref(),
                    budget.max_turns,
                ),
            };
            cli.runtime.fire_lifecycle_hook(
                runtime::HookEvent::SessionEnd,
                &serde_json::json!({
                    "reason": if turn_result.is_ok() { "exit" } else { "error" },
                }),
            );
            turn_result?;
        }
        CliAction::Login { provider } => {
            run_login_provider(provider.as_deref().unwrap_or("claude"))?;
            eprintln!("\nStarting zo...\n");
            // Post-login handoff pins neither mode; the settings/default
            // resolution applies exactly as it would for a bare `zo`.
            run_repl(
                DEFAULT_MODEL.to_string(),
                false,
                None,
                runtime::PermissionMode::DangerFullAccess,
                None,
                false,
                false,
            )?;
        }
        CliAction::Logout => run_logout()?,
        CliAction::Init => run_init()?,
        CliAction::Repl {
            model,
            model_pinned,
            allowed_tools,
            disallowed_tools,
            permission_mode,
            max_turns,
            max_tool_calls,
            system_prompt,
            append_system_prompt,
            verbose,
            mcp_config,
            inline,
            fullscreen,
        } => {
            warn_unapplied_repl_flags(verbose);
            crate::runtime_support::install_cli_runtime_overrides(
                crate::runtime_support::CliRuntimeOverrides {
                    disallowed_tools,
                    max_turns,
                    max_tool_calls,
                    system_prompt,
                    append_system_prompt,
                },
            );
            run_repl(
                model,
                model_pinned,
                allowed_tools,
                permission_mode,
                mcp_config,
                inline,
                fullscreen,
            )?;
        }
        CliAction::Help => print_help(),
        CliAction::HelpText(text) => println!("{text}"),
        CliAction::SlashCommand {
            command,
            model,
            allowed_tools,
            permission_mode,
        } => {
            // Headless slash commands (e.g. `/session rename`) deliberately
            // operate on the project's managed sessions in `.zo/sessions/`,
            // so they stay project-scoped. Only the one-shot `-p` prompt path
            // (above) is ephemeral.
            let mut cli = LiveCli::new_requiring_startup_auth(model, true, allowed_tools, permission_mode)?;
            cli.handle_repl_command(command)?;
            // This path never runs a turn, so `persist_session` never fires —
            // without this, a headless `/goal clear|pause|start` mutation is
            // silently dropped and the stale on-disk goal resurrects next boot.
            cli.save_automation_state();
        }
        CliAction::Serve {
            bind_addr,
            model,
            allowed_tools,
            permission_mode,
        } => crate::serve::run_serve(bind_addr, model, allowed_tools, permission_mode)?,
        CliAction::Acp {
            model,
            allowed_tools,
            permission_mode,
        } => crate::acp_host::run_acp(model, allowed_tools, permission_mode)?,
        CliAction::Attach {
            bind_addr,
            session_id,
            plain,
        } => {
            if plain {
                crate::attach::run_attach(bind_addr, session_id)?;
            } else {
                crate::attach_tui::run_attach_tui(bind_addr, session_id)?;
            }
        }
    }
    Ok(())
}

fn run_headless_turn_with_optional_fallback(
    cli: &mut LiveCli,
    prompt: &str,
    output_format: CliOutputFormat,
    fallback_model: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Same evidence teardown the TUI loop has: headless turns attest too, and
    // `/refine` is only as good as the sessions that report in.
    let _attest_evidence =
        crate::session::refine::AttestPersistGuard::new(cli.session.id.clone());
    let mut turn_result = cli.run_turn_with_output(prompt, output_format);
    // `--fallback-model` (CC parity): one retry on a capacity-shaped failure of
    // the primary model — overload, rate-limit, or a 5xx that survived the
    // in-stream retries. Other errors (auth, bad input) surface as-is.
    if let (Err(error), Some(fallback)) = (&turn_result, fallback_model) {
        if turn_error_warrants_model_fallback(error.as_ref()) {
            eprintln!(
                "[zo] primary model failed ({error}); retrying once with --fallback-model {fallback}"
            );
            cli.apply_model_change(fallback);
            turn_result = cli.run_turn_with_output(prompt, output_format);
        }
    }
    turn_result
}

fn run_stream_json_input(
    cli: &mut LiveCli,
    first_prompt_prefix: &str,
    output_format: CliOutputFormat,
    fallback_model: Option<&str>,
    max_turns: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Err("--input-format stream-json requires NDJSON on stdin".into());
    }

    let mut processed = 0usize;
    for (line_index, line) in stdin.lock().lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if processed >= max_turns {
            eprintln!(
                "[zo] --max-turns {max_turns} reached; remaining stream-json input ignored"
            );
            break;
        }
        let content = stream_json_user_content(&line)
            .map_err(|error| format!("[zo] line {}: {error}", line_index + 1))?;
        let prompt = if processed == 0 && !first_prompt_prefix.trim().is_empty() {
            format!("{first_prompt_prefix}\n\n{content}")
        } else {
            content
        };
        run_headless_turn_with_optional_fallback(cli, &prompt, output_format, fallback_model)?;
        processed += 1;
    }
    Ok(())
}

fn stream_json_user_content(line: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    if let Some(role) = object.get("role").and_then(serde_json::Value::as_str) {
        if role != "user" {
            eprintln!("[zo] stream-json role {role:?} coerced to user");
        }
    }
    let content = object
        .get("content")
        .ok_or_else(|| "missing content".to_string())?;
    match content {
        serde_json::Value::String(text) => Ok(text.clone()),
        serde_json::Value::Array(parts) => Ok(parts
            .iter()
            .filter_map(|part| {
                part.as_str().map(ToOwned::to_owned).or_else(|| {
                    part.as_object()
                        .and_then(|object| object.get("text"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                })
            })
            .collect::<String>()),
        _ => Err("content must be a string or array".to_string()),
    }
}

/// Extended-thinking effort for the headless `-p` one-shot path.
///
/// Precedence, highest first:
/// 1. an explicit `ZO_EFFORT=low|medium|high|xhigh|max|ultra|smart` env pin
///    (aliases: `none`/`disable`, `med`, `smartcode`/`ultracode`/`uc` → Smart;
///    `ultra` is its own static top-tier level, not a Smart alias) — always wins;
/// 2. the project's pinned `reasoningEffort` (`persisted`), so an interactive
///    `/effort` choice saved to `.zo/settings*.json` carries into `-p` runs
///    instead of being silently dropped (Gap D);
/// 3. the prompt's shape (see [`auto_effort_for_prompt`]): a feature build gets
///    `Max` because strong thinking focuses the agent and *lowers* cost on real
///    feature tasks, while a narrow fix or analysis keeps `Off` — thinking bills
///    as the most expensive output-token class and buys those no accuracy.
fn headless_prompt_effort(prompt: &str, persisted: Option<Effort>) -> Effort {
    let env = std::env::var("ZO_EFFORT").ok();
    resolve_headless_effort(env.as_deref(), persisted, prompt)
}

/// Pure core of [`headless_prompt_effort`] — kept env-free so the env > project
/// preference > prompt-shape precedence is unit-testable without mutating
/// process state.
///
/// A *present* `ZO_EFFORT` (even empty/garbage) is treated as an explicit,
/// authoritative pin and resolved by [`parse_headless_effort`] (garbage → `Off`),
/// so a deliberate env override is never silently shadowed by a persisted
/// project effort. Only an *absent* env var falls through to the persisted
/// project preference, then to the prompt-shape default.
fn resolve_headless_effort(
    env_token: Option<&str>,
    persisted: Option<Effort>,
    prompt: &str,
) -> Effort {
    match env_token {
        Some(token) => parse_headless_effort(Some(token)),
        None => persisted.unwrap_or_else(|| auto_effort_for_prompt(prompt)),
    }
}

/// Pure core of [`headless_prompt_effort`] — kept env-free so it is unit-testable
/// without mutating process state. An empty/absent/unrecognized value is OFF.
fn parse_headless_effort(raw: Option<&str>) -> Effort {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(Effort::from_token)
        .unwrap_or(Effort::Off)
}

// Safe envelope for the dynamic budget. These are *bounds*, not a
// per-difficulty table: the actual turn/tool budget is computed continuously
// from the prompt's measured shape and only clamped into this range — so there
// are no hardcoded "simple = 4 turns" magic numbers to drift.
const MIN_TURNS: usize = 8;
const MAX_TURNS: usize = 150;
const MIN_TOOL_CALLS: usize = 16;
const MAX_TOOL_CALLS: usize = 400;
/// Tool calls scale off turns: one agentic iteration can fan out several tools.
const TOOL_CALLS_PER_TURN: f64 = 2.5;

// Weights converting each prompt signal into turn-equivalents. Tunable bounds,
// not a difficulty lookup — the budget varies continuously with the signals.
const BASE_WORK_UNITS: f64 = 8.0;
const CHARS_PER_TURN: f64 = 500.0;
const WORKFLOW_WEIGHT: f64 = 6.0;
const COMPLEX_WEIGHT: f64 = 2.0;
const FILE_WEIGHT: f64 = 1.5;
const MAX_FILE_SIGNAL: usize = 8;
/// Turn-equivalent floor for a prompt that asks for an actual code change.
///
/// Coding is inherently multi-turn — explore → edit → run tests → fix failures
/// → re-run — and a prompt's *length* badly under-predicts that turn demand: a
/// one-line "fix the failing test" can need 50+ iterations, and a real
/// multi-file feature routinely does. The length/vocabulary estimate alone made
/// `-p` cap such tasks at ~22 turns and die mid-edit having written nothing
/// (measured on a real `click` feature task). When a coding signal is present we
/// raise the floor to this value. It is only a *ceiling*: a task that finishes
/// early stops at `end_turn` and never spends the headroom, so a generous floor
/// costs no extra tokens — it only prevents a premature death.
const CODING_FLOOR_WORK: f64 = 90.0;

/// Derive a bounded, *dynamic* headless execution budget from the prompt's
/// measured shape. There is no hardcoded difficulty→budget table: the budget
/// scales continuously with prompt length, workflow/complexity vocabulary,
/// referenced files, and the effort level, then is clamped into a safe
/// envelope. This keeps default `-p` runs cost-safe without making operators
/// hand-tune `--max-turns` / `--max-tool-calls`, while never starving an
/// honest short-but-deep task. Explicit flags always win.
// `turns`/`tool_calls` are tiny counts (≤ MAX_TURNS), so the usize↔f64 casts
// here and in the helpers below are exact in practice; the precision-loss lint
// is moot at this scale.
#[allow(clippy::cast_precision_loss)]
fn headless_execution_budget(
    prompt: &str,
    effort: Effort,
    explicit_max_turns: Option<usize>,
    explicit_max_tool_calls: Option<usize>,
) -> HeadlessExecutionBudget {
    let work = prompt_work_units(prompt, effort);
    let turns = round_clamp(work, MIN_TURNS, MAX_TURNS);
    let tool_calls = round_clamp(
        turns as f64 * TOOL_CALLS_PER_TURN,
        MIN_TOOL_CALLS,
        MAX_TOOL_CALLS,
    );
    HeadlessExecutionBudget {
        max_turns: explicit_max_turns.unwrap_or(turns),
        max_tool_calls: explicit_max_tool_calls.unwrap_or(tool_calls),
    }
}

/// Continuous "work pressure" of a prompt, in turn-equivalents. Every term is
/// derived from the actual prompt (or the operator's effort), so a longer,
/// more workflow-heavy, multi-file, or higher-effort prompt yields a strictly
/// larger budget — the dynamic replacement for a fixed difficulty table.
// Char/marker counts are small; the usize→f64 widening is exact at this scale.
#[allow(clippy::cast_precision_loss)]
fn prompt_work_units(prompt: &str, effort: Effort) -> f64 {
    let lower = prompt.to_ascii_lowercase();
    let chars = prompt.chars().count() as f64;
    let workflow_hits = count_present(&lower, WORKFLOW_MARKERS) as f64;
    let complex_hits = count_present(&lower, COMPLEX_MARKERS) as f64;
    // Path-like tokens signal "touch N files"; cap so a path dump can't explode.
    let file_hits = count_path_mentions(prompt).min(MAX_FILE_SIGNAL) as f64;

    let estimated = BASE_WORK_UNITS
        + chars / CHARS_PER_TURN
        + workflow_hits * WORKFLOW_WEIGHT
        + complex_hits * COMPLEX_WEIGHT
        + file_hits * FILE_WEIGHT
        + effort_work_units(effort);

    // A code-change request is inherently multi-turn; the length/vocabulary
    // estimate above under-predicts that demand. Raise the floor when a coding
    // signal is present (see `CODING_FLOOR_WORK`). Analysis/summary prompts —
    // which the budget tests exercise — carry no coding signal and are unaffected.
    if is_coding_task(&lower, file_hits) {
        estimated.max(CODING_FLOOR_WORK)
    } else {
        estimated
    }
}

/// Whether a prompt is asking for an actual code change (vs. analysis/summary).
/// A referenced file path is the strongest signal; otherwise an explicit
/// code-change verb. Deliberately excludes `analyze`/`benchmark`/`report` so
/// read-only investigation prompts keep their smaller, length-derived budget.
pub(crate) fn is_coding_task(lower: &str, file_hits: f64) -> bool {
    file_hits > 0.0 || count_present(lower, CODING_TASK_MARKERS) > 0
}

/// [`is_coding_task`] for a raw, unpreprocessed prompt — the single entry point
/// callers outside the effort-derivation path use (e.g. gating headless
/// reactive auto-verify to code-changing turns). Reuses the exact same
/// lowercase + path-mention preprocessing as [`auto_effort_for_prompt`] so the
/// two never diverge on what counts as "coding".
#[allow(clippy::cast_precision_loss)]
pub(crate) fn prompt_is_coding_task(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let file_hits = count_path_mentions(prompt) as f64;
    is_coding_task(&lower, file_hits)
}

/// Auto-select reasoning effort from the prompt when no `ZO_EFFORT` is pinned.
/// Grounded in the harness bench (real `click`/`sqlparse` tasks, same opus model):
///   * Analysis / narrow fix → `Off`. Already token-optimal; thinking buys no
///     accuracy — bug & sql pass 100% at Off and cost 1.3–2× more tokens at Max
///     for the *same* result.
///   * New-feature implementation → `High`. Feature work needs real thinking,
///     but post effort-fix (f69e9e7a) the bench shows `High` is the coding sweet
///     spot: moderate features pass at `High` (196/197) fast and cheap, while
///     `Xhigh`/`Max` only burn 3–12× the tokens/time for the same-or-worse
///     result — `Max` even over-thinks past the budget without converging.
///     Genuinely *hard* features (where `High` proves insufficient) want `Xhigh`,
///     but static prompt shape can't tell hard from moderate; that is the job of
///     runtime escalation (start `High`, step up to `Xhigh` on a stalled retry),
///     not of a higher blanket tier here. `Max` is never the coding optimum.
///
/// Static complexity (length, file count, generic markers) can't separate these:
/// on the bench all three prompts score ~17–18 work-units alike. The gate is the
/// *intent* verb — building new behavior vs. fixing existing behavior.
#[allow(clippy::cast_precision_loss)]
fn auto_effort_for_prompt(prompt: &str) -> Effort {
    let lower = prompt.to_ascii_lowercase();
    let file_hits = count_path_mentions(prompt) as f64;
    if is_coding_task(&lower, file_hits) && count_present(&lower, FEATURE_IMPL_MARKERS) > 0 {
        Effort::High
    } else {
        Effort::Off
    }
}

/// Markers that a prompt asks to *build new behavior*, gating the auto effort
/// raise. Deliberately the "building" verbs only — `fix`/`bug`/`regression`/
/// `debug` are excluded because narrow fixes are already optimal at `Off`.
const FEATURE_IMPL_MARKERS: &[&str] = &[
    "implement",
    "new feature",
    "add a ",
    "add an ",
    "add support",
    "add the ability",
    "구현",
    "새 기능",
    "기능 추가",
];

/// Markers of a code-change request. Substring match, so multi-word forms
/// (`fix the`, `add support`) are used where a bare verb (`fix`) would false-match
/// inside unrelated words (`prefix`, `suffix`).
const CODING_TASK_MARKERS: &[&str] = &[
    "implement",
    "improve",
    "improvement",
    "optimize",
    "document ",
    "documentation",
    "refactor",
    "rename",
    "bugfix",
    "fix the",
    "fix a ",
    "fix this",
    "add support",
    "add a method",
    "add an option",
    "make the test",
    "failing test",
    "regression",
    "def ",
    "class ",
    "import ",
    "src/",
    "구현",
    "개선",
    "문서화",
    "리팩터",
    "버그",
    "함수",
];

/// Effort's contribution in turn-equivalents: reasoning-heavy runs warrant a
/// larger envelope. Exhaustive so a new `Effort` variant forces a choice here.
fn effort_work_units(effort: Effort) -> f64 {
    match effort {
        Effort::Off => 0.0,
        Effort::Low => 2.0,
        Effort::Medium => 5.0,
        Effort::High => 10.0,
        Effort::Xhigh => 14.0,
        Effort::Max => 18.0,
        // Static top pin, one rung above Max, below Smart's orchestration budget.
        Effort::Ultra => 22.0,
        Effort::Smart => 28.0,
    }
}

/// Lower-case token for the auto-budget operator note, so a `-p` run surfaces
/// which reasoning effort was applied — auto-selected from the prompt shape or
/// pinned via `ZO_EFFORT`. The inverse of `Effort::from_token`.
const fn effort_token(effort: Effort) -> &'static str {
    match effort {
        Effort::Off => "off",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
        Effort::Ultra => "ultra",
        Effort::Smart => "smart",
    }
}

/// Number of distinct markers from `needles` present in `haystack`.
fn count_present(haystack: &str, needles: &[&str]) -> usize {
    needles
        .iter()
        .filter(|needle| haystack.contains(**needle))
        .count()
}

/// Count whitespace tokens that look like a file path (`a/b/c.rs`), a cheap
/// proxy for how many files the task spans.
fn count_path_mentions(prompt: &str) -> usize {
    prompt
        .split_whitespace()
        .filter(|token| {
            let trimmed = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '/');
            trimmed.contains('/') && !trimmed.starts_with("http")
        })
        .count()
}

/// Round a work score to the nearest whole turn and clamp into `[min, max]`.
// `max(0.0)` rules out a negative cast and the value is small, so truncation /
// sign-loss can't actually occur here.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_clamp(value: f64, min: usize, max: usize) -> usize {
    (value.round().max(0.0) as usize).clamp(min, max)
}

const WORKFLOW_MARKERS: &[&str] = &[
    "multi-agent",
    "parallel",
    "workflow",
    "ultrawork",
    "ultraqa",
    "dynamic workflow",
    "entire repository",
    "large repo",
    "병렬",
    "워크플로우",
    "에이전트",
    "전체 저장소",
];

const COMPLEX_MARKERS: &[&str] = &[
    "analysis",
    "analyze",
    "benchmark",
    "bench",
    "debug",
    "diagnose",
    "evidence",
    "implement",
    "opencode",
    "open code",
    "provider",
    "refactor",
    "repository",
    "runtime",
    "test",
    "벤치",
    "분석",
    "검증",
    "구현",
    "디버그",
    "수정",
    "코딩",
];

/// Which optional `-p` flags were present. Grouped into a struct so the
/// predicate fn stays under clippy's bool-parameter limit and every field is
/// named at the call site. These are independent presence bits — any subset can
/// be set at once — so they're genuinely bools, not a state machine.
#[derive(Clone, Copy, Default)]
struct PromptFlagPresence {
    verbose: bool,
    no_follow: bool,
}

/// Flags that `-p` parses but does not yet apply, returned in CLI-surface order
/// so the operator note lists exactly what was ignored. Pure for unit testing.
fn unapplied_prompt_flags(present: PromptFlagPresence) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if present.verbose {
        flags.push("--verbose");
    }
    if present.no_follow {
        flags.push("--no-follow");
    }
    flags
}

/// Surface `-p` flags that are parsed but not yet wired into the headless
/// runtime, so they fail visibly instead of being silently dropped — the same
/// honesty contract `--input-format` already follows. `--system-prompt`,
/// `--append-system-prompt`, and `--allowedTools` *are* applied and are never
/// listed here. `--disallowed-tools` is enforced at execution time and is not
/// an unapplied flag.
fn warn_unapplied_prompt_flags(verbose: bool, no_follow: bool) {
    let flags = unapplied_prompt_flags(PromptFlagPresence { verbose, no_follow });
    if !flags.is_empty() {
        eprintln!(
            "[zo] note: {} parsed but not yet applied in -p mode (honored: \
             --system-prompt, --append-system-prompt, --allowedTools, --disallowed-tools, \
             --max-turns, --max-tool-calls).",
            flags.join(", ")
        );
    }
}

fn warn_unapplied_repl_flags(verbose: bool) {
    if verbose {
        eprintln!(
            "[zo] note: --verbose parsed but not yet applied in interactive mode (honored: \
             --system-prompt, --append-system-prompt, --allowedTools, --disallowed-tools, \
             --max-turns, --max-tool-calls)."
        );
    }
}

/// Whether a turn failure is capacity-shaped — the only class where retrying
/// once on the `--fallback-model` makes sense (CC parity). Auth/input errors
/// would fail identically on any model.
fn runtime_error_warrants_model_fallback(error: &runtime::RuntimeError) -> bool {
    match error.provider_error_class() {
        Some(api::ProviderErrorClass::RateLimit { .. } | api::ProviderErrorClass::Transient) => {
            true
        }
        Some(
            api::ProviderErrorClass::AuthExpired
            | api::ProviderErrorClass::ContextOverflow
            | api::ProviderErrorClass::InvalidToolProtocol
            | api::ProviderErrorClass::InvalidToolSchema
            | api::ProviderErrorClass::SafetyBlocked
            | api::ProviderErrorClass::NonRetryable,
        ) => false,
        None => error_warrants_model_fallback(&error.to_string()),
    }
}

fn turn_error_warrants_model_fallback(error: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(runtime_error) = error.downcast_ref::<runtime::RuntimeError>() {
        return runtime_error_warrants_model_fallback(runtime_error);
    }
    if let Some(streaming_error) = error.downcast_ref::<runtime::StreamingTurnError>() {
        return match streaming_error.provider_error_class() {
            Some(
                api::ProviderErrorClass::RateLimit { .. } | api::ProviderErrorClass::Transient,
            ) => true,
            Some(
                api::ProviderErrorClass::AuthExpired
                | api::ProviderErrorClass::ContextOverflow
                | api::ProviderErrorClass::InvalidToolProtocol
                | api::ProviderErrorClass::InvalidToolSchema
                | api::ProviderErrorClass::SafetyBlocked
                | api::ProviderErrorClass::NonRetryable,
            ) => false,
            None => error_warrants_model_fallback(&streaming_error.to_string()),
        };
    }
    error_warrants_model_fallback(&error.to_string())
}

/// Whether `--fallback-model` should re-run a failed headless turn on the
/// fallback: capacity or transient pressure yes, an auth/validation verdict no
/// (the fallback would fail identically).
///
/// Classified through `core_types::retry_signal`, the same vocabulary the retry
/// schedule and the quota escape read, rather than a private copy of the
/// substring list. The copy was a second classifier that could drift from the
/// first — and it read the WHOLE `Display` string, hint included, which is
/// exactly how a hint's prose ("this is NOT your account's rate limit") taught
/// the retry layer the opposite of what it says.
///
/// The auth veto stays local: `RetrySignal` folds every non-retryable cause into
/// `Fatal`, and this site needs the narrower "was it credentials?" answer to keep
/// a 401 from being read as capacity by a relay that words it loosely.
fn error_warrants_model_fallback(error: &str) -> bool {
    let head = core_types::retry_signal::machine_readable_head(error).to_ascii_lowercase();
    if head.contains("401")
        || head.contains("unauthorized")
        || head.contains("invalid api key")
        || head.contains("auth failed")
        || head.contains("authentication")
    {
        return false;
    }
    core_types::retry_signal::classify_error_text(error).is_retryable()
}

#[cfg(test)]
mod unapplied_flag_tests {
    use super::{
        auto_effort_for_prompt, count_path_mentions, error_warrants_model_fallback,
        headless_execution_budget, is_coding_task, parse_headless_effort, prompt_is_coding_task,
        resolve_headless_effort, runtime_error_warrants_model_fallback,
        turn_error_warrants_model_fallback, unapplied_prompt_flags, Effort,
        PromptFlagPresence, MAX_TOOL_CALLS, MAX_TURNS, MIN_TOOL_CALLS, MIN_TURNS,
    };
    use api::ProviderErrorClass;

    /// `--fallback-model` 은 용량성 실패에만 발동 — 인증/입력 오류는 폴백해도
    /// 같은 실패라 재시도하지 않는다.
    #[test]
    fn fallback_triggers_only_on_capacity_failures() {
        assert!(error_warrants_model_fallback(
            "api returned 429 Too Many Requests"
        ));
        assert!(error_warrants_model_fallback("upstream overloaded (529)"));
        assert!(error_warrants_model_fallback("request timed out"));
        assert!(!error_warrants_model_fallback(
            "401 Unauthorized: invalid api key"
        ));
        assert!(!error_warrants_model_fallback(
            "invalid request: prompt too long"
        ));
        // Reading the human hint instead of the machine head is how a 529 taught
        // the retry layer "account throttle"; this predicate must be immune too.
        let overload_with_hint = api::ApiError::StreamApi {
            error_type: Some("overloaded_error".to_string()),
            message: Some("Overloaded".to_string()),
            body: String::new(),
            retryable: true,
        }
        .to_string();
        assert!(error_warrants_model_fallback(&overload_with_hint));
        // And an auth failure keeps its veto even though its own hint says
        // "retry" — the fallback would fail identically on the same credential.
        let auth_with_hint = api::ApiError::Auth("invalid api key".to_string()).to_string();
        assert!(!error_warrants_model_fallback(&auth_with_hint));
    }

    #[test]
    fn fallback_string_heuristic_keeps_auth_precedence_over_rate_limit_text() {
        assert!(!error_warrants_model_fallback(
            "401 Unauthorized: invalid api key; quota page mentions rate limit diagnostics"
        ));
    }

    #[test]
    fn model_fallback_uses_structured_rate_limit_class() {
        let rate_limit = runtime::RuntimeError::with_provider_error_class(
            "401 text appears in stale diagnostics",
            ProviderErrorClass::account_rate_limit(None),
        );
        assert!(runtime_error_warrants_model_fallback(&rate_limit));

        let auth = runtime::RuntimeError::with_provider_error_class(
            "provider body mentions rate limit diagnostics",
            ProviderErrorClass::AuthExpired,
        );
        assert!(!runtime_error_warrants_model_fallback(&auth));
    }

    #[test]
    fn model_fallback_downcasts_structured_runtime_error_before_text_fallback() {
        let auth = runtime::RuntimeError::with_provider_error_class(
            "rate limit text in auth diagnostic",
            ProviderErrorClass::AuthExpired,
        );
        let boxed: Box<dyn std::error::Error> = Box::new(auth);
        assert!(!turn_error_warrants_model_fallback(boxed.as_ref()));

        let transient =
            runtime::StreamingTurnError::from(runtime::RuntimeError::with_provider_error_class(
                "plain provider failure",
                ProviderErrorClass::Transient,
            ));
        let boxed: Box<dyn std::error::Error> = Box::new(transient);
        assert!(turn_error_warrants_model_fallback(boxed.as_ref()));
    }

    #[test]
    fn headless_effort_defaults_off_and_parses_opt_in() {
        // Absent / empty / unrecognized → OFF (cost-safe default for one-shots).
        assert_eq!(parse_headless_effort(None), Effort::Off);
        assert_eq!(parse_headless_effort(Some("   ")), Effort::Off);
        assert_eq!(parse_headless_effort(Some("garbage")), Effort::Off);
        // Opt-in: canonical names + case-insensitivity + aliases. `ultra` is now
        // its own static top-tier level (P9), no longer an alias of Smart; use
        // `ultracode`/`smartcode`/`uc` to reach the dynamic-band preset.
        assert_eq!(parse_headless_effort(Some("high")), Effort::High);
        assert_eq!(parse_headless_effort(Some("  HIGH ")), Effort::High);
        assert_eq!(parse_headless_effort(Some("ultra")), Effort::Ultra);
        assert_eq!(parse_headless_effort(Some("ultracode")), Effort::Smart);
        assert_eq!(parse_headless_effort(Some("off")), Effort::Off);
    }

    #[test]
    fn resolve_headless_effort_precedence_env_over_persisted_over_prompt() {
        // A feature prompt auto-derives `High`; an analysis prompt auto-derives
        // `Off`. These anchor tier 3 (prompt-shape) below.
        let feat = "Implement a new feature: add a `deprecated` keyword to \
                    options and commands under src/click/.";
        let analysis = "Analyze and summarize the overall architecture.";

        // (a) A present env pin wins over both a persisted preference and the
        //     prompt shape — including the authoritative "garbage → Off" pin,
        //     which must NOT silently fall through to a persisted High.
        assert_eq!(
            resolve_headless_effort(Some("medium"), Some(Effort::Smart), feat),
            Effort::Medium,
        );
        assert_eq!(
            resolve_headless_effort(Some("garbage"), Some(Effort::Smart), feat),
            Effort::Off,
            "a present (even invalid) env pin is authoritative and is not shadowed by a persist"
        );

        // (b) No env → the persisted project preference carries (Gap D): a
        //     pinned `smart` survives into `-p` even on an analysis prompt
        //     that would otherwise auto-derive `Off`.
        assert_eq!(
            resolve_headless_effort(None, Some(Effort::Smart), analysis),
            Effort::Smart,
        );

        // (c) No env, no persisted preference → fall through to prompt-shape,
        //     byte-identical to the prior default behavior.
        assert_eq!(
            resolve_headless_effort(None, None, feat),
            auto_effort_for_prompt(feat),
        );
        assert_eq!(
            resolve_headless_effort(None, None, analysis),
            auto_effort_for_prompt(analysis),
        );
    }

    #[test]
    fn auto_effort_raises_for_feature_implementation() {
        // A feature build warrants real thinking, but `High` is the coding sweet
        // spot post effort-fix: moderate features pass at High fast/cheap, while
        // Xhigh/Max only burn 3–12× for the same-or-worse result (Max even
        // over-thinks past budget). Hard features that need Xhigh are the job of
        // runtime escalation, not this static gate.
        let feat = "Implement a new feature: add a `deprecated` keyword to \
                    options and commands under src/click/.";
        assert_eq!(auto_effort_for_prompt(feat), Effort::High);
    }

    #[test]
    fn auto_effort_stays_off_for_narrow_fix() {
        // A narrow regression fix is already token-optimal at Off; thinking only
        // adds output cost for the same 100% pass rate. No FEATURE_IMPL marker.
        let bug = "Fix a real regression in the library source under src/click/ \
                   so the parameter source is recorded early enough.";
        assert_eq!(auto_effort_for_prompt(bug), Effort::Off);
        let sql = "Fix the statement splitter under sqlparse/ so BEGIN \
                   TRANSACTION blocks split into individual statements.";
        assert_eq!(auto_effort_for_prompt(sql), Effort::Off);
    }

    #[test]
    fn auto_effort_off_for_non_coding_prompt() {
        // No coding signal (no path, no code-change verb) → never charged
        // thinking, even though "analyze" is a generic complexity marker.
        let analysis = "Analyze and summarize the overall architecture.";
        assert_eq!(auto_effort_for_prompt(analysis), Effort::Off);
    }

    #[test]
    fn prompt_is_coding_task_matches_code_changes_not_analysis() {
        // Raw-prompt wrapper must agree with the preprocessed `is_coding_task`
        // (same lowercase + path-mention signal `auto_effort_for_prompt` uses),
        // so headless reactive auto-verify gates on exactly the code-change set.
        // A referenced path → coding.
        assert!(prompt_is_coding_task(
            "fix the parser bug in src/click/core.py"
        ));
        // An explicit code-change verb → coding.
        assert!(prompt_is_coding_task(
            "implement a new --deprecated flag for the CLI"
        ));
        // Read-only investigation → NOT coding (stays single-pass / no verify).
        assert!(!prompt_is_coding_task(
            "analyze and summarize the overall architecture"
        ));
        assert!(!prompt_is_coding_task(
            "benchmark the suite and report timings"
        ));
        // Consistency with the preprocessed predicate it wraps.
        let p = "refactor the auth module in crates/api/src/lib.rs";
        let lower = p.to_ascii_lowercase();
        #[allow(clippy::cast_precision_loss)]
        let hits = count_path_mentions(p) as f64;
        assert_eq!(prompt_is_coding_task(p), is_coding_task(&lower, hits));
    }

    #[test]
    fn explicit_headless_budgets_override_dynamic_policy() {
        let budget = headless_execution_budget(
            "debug this large benchmark",
            Effort::Smart,
            Some(3),
            Some(5),
        );

        // Explicit flags win outright.
        assert_eq!(budget.max_turns, 3);
        assert_eq!(budget.max_tool_calls, 5);
    }

    #[test]
    fn dynamic_budget_scales_with_prompt_difficulty() {
        let simple = headless_execution_budget("summarize this sentence", Effort::Off, None, None);
        let complex = headless_execution_budget(
            "Benchmark Zo against OpenCode using the provided evidence pack and report gaps.",
            Effort::Max,
            None,
            None,
        );
        let workflow = headless_execution_budget(
            "Run a dynamic multi-agent workflow over this repository.",
            Effort::Smart,
            None,
            None,
        );

        // Budgets rise strictly with difficulty — derived continuously from the
        // prompt, not looked up from a hardcoded tier table.
        assert!(
            simple.max_turns < complex.max_turns,
            "{simple:?} should be cheaper than {complex:?}"
        );
        assert!(
            complex.max_turns <= workflow.max_turns,
            "{complex:?} should not exceed {workflow:?}"
        );
        assert!(simple.max_tool_calls < complex.max_tool_calls);
    }

    #[test]
    fn dynamic_budget_stays_within_safe_envelope() {
        // Even a trivial prompt gets a usable floor; a pathological one is capped.
        let tiny = headless_execution_budget("hi", Effort::Off, None, None);
        assert!(tiny.max_turns >= MIN_TURNS && tiny.max_turns <= MAX_TURNS);
        assert!(tiny.max_tool_calls >= MIN_TOOL_CALLS);

        let huge =
            headless_execution_budget(&"analyze ".repeat(10_000), Effort::Smart, None, None);
        assert_eq!(huge.max_turns, MAX_TURNS);
        assert!(huge.max_tool_calls <= MAX_TOOL_CALLS);
    }

    #[test]
    fn coding_task_gets_a_multi_turn_floor() {
        // A short code-change prompt whose length/vocabulary scores low still
        // gets a budget that can survive an explore → edit → test → fix loop.
        // The real `click` feature task died at ~22 turns before this floor.
        let coding = headless_execution_budget(
            "Fix the failing test in src/click/core.py.",
            Effort::Off,
            None,
            None,
        );
        assert!(
            coding.max_turns >= 50,
            "coding task must clear an explore→edit→test→fix loop, got {coding:?}"
        );

        // A same-length analysis prompt carries no coding signal and stays lean.
        let analysis = headless_execution_budget(
            "Summarize the high-level architecture of this project.",
            Effort::Off,
            None,
            None,
        );
        assert!(
            analysis.max_turns < coding.max_turns,
            "analysis should stay cheaper than a coding task, got {analysis:?}"
        );
    }

    #[test]
    fn improve_and_document_prompts_get_coding_floor() {
        for prompt in [
            "improve ultracode multiagent Claude-agent routing",
            "document ultracode logic in docs/ultracode-routing.md",
            "ultracode 일반 처리 개선",
            "ultracode logic 문서화",
        ] {
            let budget = headless_execution_budget(prompt, Effort::Smart, None, None);
            assert!(
                budget.max_turns >= 50,
                "improve/document coding prompt should not get a tiny one-shot budget: {prompt} -> {budget:?}"
            );
        }
    }

    #[test]
    fn benchmark_analysis_gets_enough_tool_headroom() {
        // The opencode analysis the goal measured used ~14 tools; the dynamic
        // budget must clear that comfortably so the benchmark never self-clips.
        let budget = headless_execution_budget(
            "Analyze the OpenCode evidence pack and report gaps with file:line.",
            Effort::Max,
            None,
            None,
        );
        assert!(
            budget.max_tool_calls >= 16,
            "benchmark needs tool headroom, got {budget:?}"
        );
    }

    #[test]
    fn lists_only_set_flags_in_surface_order() {
        let present = PromptFlagPresence {
            verbose: true,
            no_follow: true,
        };
        assert_eq!(
            unapplied_prompt_flags(present),
            vec!["--verbose", "--no-follow"]
        );
    }

    #[test]
    fn enforced_disallowed_tools_are_not_reported_unapplied() {
        assert!(
            !unapplied_prompt_flags(PromptFlagPresence {
                verbose: true,
                no_follow: true,
            })
            .contains(&"--disallowed-tools")
        );
    }

    #[test]
    fn empty_when_nothing_unapplied() {
        assert!(unapplied_prompt_flags(PromptFlagPresence::default()).is_empty());
    }

    #[test]
    fn all_flags_listed_in_surface_order() {
        let present = PromptFlagPresence {
            verbose: true,
            no_follow: true,
        };
        assert_eq!(
            unapplied_prompt_flags(present),
            vec!["--verbose", "--no-follow"]
        );
    }
}
