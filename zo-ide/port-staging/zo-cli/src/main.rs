mod acp_host;
mod attach;
mod attach_tui;
mod auth;
mod cli_args;
mod cli_support;
mod cli_tool_executor;
mod command_reports;
mod conversation_support;
mod custom_provider_env;
mod model_wire_env;
mod doctor;
mod formatting;
mod git_helpers;
mod init;
mod input;
mod main_dispatch;
mod permission_mode;
mod render;
mod response_events;
mod remote_control;
mod resume;
mod runtime_support;
mod serve;
mod serve_auth;
mod serve_protocol;
mod self_update;
mod session;
mod session_format;
mod session_registry;
mod status_actions;
mod tool_formatting;
mod workspace_reports;
mod workspace_trust;

#[cfg(test)]
use std::collections::BTreeSet;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

/// Global flag set while the ratatui TUI owns the terminal
/// (alt-screen + raw mode). When `true`, every legacy code path that
/// would otherwise write assistant text or tool output to `io::stdout`
/// MUST redirect to `io::sink()` instead. Direct stdout writes during
/// raw mode produce a "staircase" because ONLCR is disabled, and they
/// also race with ratatui's frame draws — corrupting the screen.
///
/// Set by `tui_loop::init_terminal`, cleared by
/// `tui_loop::restore_terminal`. Read by `CliToolExecutor::execute`
/// and `AnthropicRuntimeClient::stream`.
pub(crate) static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

/// `ThreadId` of the thread that owns the TUI terminal. The panic hook restores
/// the terminal ONLY for a panic on this thread; a panic on a background worker
/// (e.g. the `mcp-discovery` thread, whose own `catch_unwind` keeps the process
/// alive) must not tear down the live terminal or clear [`TUI_ACTIVE`] — doing
/// so would corrupt the screen and disarm a later real panic's restore.
pub(crate) static TUI_THREAD_ID: std::sync::OnceLock<std::thread::ThreadId> =
    std::sync::OnceLock::new();

/// Record the calling thread as the TUI terminal owner (first call wins).
pub(crate) fn mark_tui_thread() {
    let _ = TUI_THREAD_ID.set(std::thread::current().id());
}

/// `true` while the TUI holds the terminal — see [`TUI_ACTIVE`].
#[must_use]
pub(crate) fn tui_active() -> bool {
    TUI_ACTIVE.load(Ordering::Relaxed)
}
use std::thread::{self, JoinHandle};

use api::ToolDefinition;
#[cfg(test)]
use commands::{public_slash_command_specs_iter, slash_command_names};
use init::initialize_repo;
use plugins::{PluginManager, PluginManagerConfig};
use runtime::ConfigLoader;
use status_actions::current_tool_registry;
use tools::GlobalToolRegistry;

#[cfg(test)]
use crate::cli_args::resolve_model_alias;
use crate::cli_args::{AllowedToolSet, DisallowedToolSet, parse_args};
#[cfg(test)]
pub(crate) use crate::cli_support::print_help_to;
pub(crate) use crate::cli_support::{print_help, render_version_line, render_version_report};
pub(crate) use crate::cli_tool_executor::CliToolExecutor;
pub(crate) use crate::command_reports::{
    auto_gate_directive, build_bughunter_prompt, build_council_prompt, build_distill_prompt,
    build_init_prompt, build_issue_prompt, build_pr_prompt, build_ultraplan_prompt,
    deep_gate_directive, format_commit_preflight_report, format_commit_skipped_report, git_output,
    render_last_tool_debug_report, render_teleport_report, validate_no_args,
};
#[cfg(test)]
pub(crate) use crate::conversation_support::permission_policy;
pub(crate) use crate::conversation_support::{
    collect_prompt_cache_events, collect_tool_results, collect_tool_uses, convert_messages,
    final_assistant_text, mark_conversation_cache_breakpoints, redact_for_share,
    render_export_text,
};
pub(crate) use crate::permission_mode::{
    default_permission_mode, interactive_default_permission_mode, normalize_permission_mode,
    permission_mode_from_label, resolved_tui_mode_is_inline,
};
#[cfg(test)]
pub(crate) use crate::response_events::{push_output_block, response_to_events};
use crate::resume::{StatusContext, StatusUsage};
#[cfg(test)]
pub(crate) use crate::runtime_support::build_runtime_with_plugin_state;
pub(crate) use crate::runtime_support::AnthropicRuntimeClient;
use crate::session::{BuiltRuntime, RuntimePluginState};
pub(crate) use crate::session_format::{
    format_missing_session_reference, format_no_managed_sessions, format_session_modified_age,
};
pub(crate) use crate::session_registry::{
    create_managed_session_handle, render_session_list, resolve_export_path,
    resolve_session_reference, write_atomic, write_session_clear_backup,
};
#[cfg(test)]
pub(crate) use crate::tool_formatting::format_tool_call_start;
pub(crate) use crate::tool_formatting::format_tool_result;
pub(crate) use crate::workspace_reports::{
    build_review_prompt, format_sandbox_report, format_status_report, render_config_report,
    render_diff_report, render_diff_report_for, render_hooks_report,
    render_memory_report, render_repl_help, render_review_report, status_context,
};

pub(crate) const DEFAULT_MODEL: &str = api::ANTHROPIC_LATEST_MODEL_ALIAS;

/// Per-response output-token cap for `model`. Delegates to the canonical
/// [`api::max_tokens_for_model`] so the CLI and the API crate stay one source
/// of truth — the previous CLI-local copy capped Opus at 8,192, silently
/// truncating large Opus 4.8 / GPT-5.5 responses.
pub(crate) fn max_tokens_for_model(model: &str) -> u32 {
    api::max_tokens_for_model(model)
}

/// Reference date injected into the model's system prompt: today's **local**
/// date, resolved at session start from the system clock (single owner:
/// [`core_types::date`]). This used to be a hardcoded constant, which froze
/// the model's sense of "today" at the last edit of that literal — months
/// stale in practice. Distinct from [`BUILD_DATE`], which is when the binary
/// was compiled. An explicit `--date` (e.g. `zo system-prompt --date …`)
/// still overrides per invocation for reproducible prompt dumps.
pub(crate) fn default_prompt_date() -> String {
    core_types::date::current_local_date()
}
pub(crate) const DEFAULT_OAUTH_CALLBACK_PORT: u16 = 4545;
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const BUILD_TARGET: Option<&str> = option_env!("TARGET");
pub(crate) const GIT_SHA: Option<&str> = option_env!("GIT_SHA");
/// Date the binary was compiled (`YYYY-MM-DD`), stamped by `build.rs`. Used only
/// for `--version`; see [`default_prompt_date`] for the prompt-context date.
pub(crate) const BUILD_DATE: Option<&str> = option_env!("BUILD_DATE");
pub(crate) const PRIMARY_SESSION_EXTENSION: &str = "jsonl";
pub(crate) const LEGACY_SESSION_EXTENSION: &str = "json";
pub(crate) const LATEST_SESSION_REFERENCE: &str = "latest";
pub(crate) const SESSION_REFERENCE_ALIASES: &[&str] = &[LATEST_SESSION_REFERENCE, "last", "recent"];

pub(crate) fn current_cli_cwd() -> io::Result<PathBuf> {
    let actual = env::current_dir()?;
    let Some(shell_pwd) = env::var_os("PWD") else {
        return Ok(actual);
    };
    let shell_pwd = PathBuf::from(shell_pwd);
    if !shell_pwd.is_absolute() {
        return Ok(actual);
    }

    let same_location = std::fs::canonicalize(&shell_pwd)
        .ok()
        .zip(std::fs::canonicalize(&actual).ok())
        .is_some_and(|(left, right)| left == right);

    if same_location {
        Ok(shell_pwd)
    } else {
        Ok(actual)
    }
}

/// Process-wide current-directory lock for tests.
///
/// LOCK-ORDER CONTRACT: when a test needs BOTH this and [`test_env_lock`],
/// it must take the env lock FIRST (env → cwd) — `src/tests.rs` holds env
/// around `with_current_dir`, and mixing directions across modules produced
/// a real AB-BA deadlock that wedged the whole bin suite (2026-07-16).
#[cfg(test)]
pub(crate) fn test_cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Point the user-global Zo home (`ZO_CONFIG_HOME` — the
/// highest-priority root that sessions, session-prefs, and per-project state
/// all resolve through) at one per-process temp directory.
///
/// Process-constant and set once, so parallel tests agree on the value with no
/// env race, and the per-project slug layout underneath keeps tests isolated
/// from one another exactly as before. Without this, every tempdir-cwd test
/// session persisted into the developer's REAL `~/.zo/projects/` — 27k
/// orphan slug directories (~2 GB) accumulated from `cargo test` runs alone.
/// Tests that set-and-restore `ZO_CONFIG_HOME` themselves are unaffected:
/// they capture this value as "prior" and put it back.
#[cfg(test)]
static ISOLATED_ZO_HOME: OnceLock<std::path::PathBuf> = OnceLock::new();

#[cfg(test)]
pub(crate) fn isolate_global_zo_home_for_tests() {
    // The seeding `set_var` lives inside the env-mutex init (see
    // `test_env_mutex`): while the mutex is still being created no test can
    // hold it, which is the only moment a lock-free env write cannot stomp a
    // lock-holding test's own `ZO_CONFIG_HOME`. Seeding lazily HERE did
    // exactly that — the first caller overwrote another test's isolated home
    // mid-guard (observed: MissingAuthRouteCredentials, 1/20 loaded rounds).
    let _ = test_env_mutex();
    let home = ISOLATED_ZO_HOME
        .get()
        .expect("seeded by the test_env_mutex init");
    // Re-assert in case an env-restoring test removed it — but only when no
    // test holds the env lock: a holder is managing the environment itself
    // (possibly asserting on an EMPTY home). `try_lock` cannot deadlock, so
    // the env→cwd lock-order contract is not violated by cwd-lock holders
    // passing through here.
    let guard = match test_env_mutex().try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if std::env::var_os("ZO_CONFIG_HOME").is_none_or(|value| value.is_empty()) {
        std::env::set_var("ZO_CONFIG_HOME", home);
    }
    // `zo_global_config_roots` ALSO discovers `$HOME/.zo` as a lower-priority
    // user root that merges under `ZO_CONFIG_HOME`, so leaving `HOME` on the
    // developer's real home lets their personal `~/.zo/settings.json` supply
    // any key a test's temp home leaves unset.
    if std::env::var_os("HOME").is_none_or(|value| value.is_empty()) {
        std::env::set_var("HOME", home);
    }
    drop(guard);
}

#[cfg(test)]
pub(crate) fn test_env_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| {
        // Tests must never consult the developer's real Claude Code keychain:
        // credential resolution would otherwise shell out to `security` and, on
        // an expired blob, fire a real token-endpoint refresh (mutating live
        // credentials / hanging in sandboxed runs).
        std::env::set_var("ZO_DISABLE_KEYCHAIN", "1");
        // Seed the isolated global zo home in the same pre-mutex moment: no
        // holder can exist before the mutex does, so these are the only
        // lock-free `set_var`s that cannot race a lock-holding test. Every
        // later reader goes through `isolate_global_zo_home_for_tests`.
        ISOLATED_ZO_HOME.get_or_init(|| {
            let dir =
                std::env::temp_dir().join(format!("zo-test-home-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            std::env::set_var("ZO_CONFIG_HOME", &dir);
            std::env::set_var("HOME", &dir);
            dir
        });
        Mutex::new(())
    })
}

/// Single process-wide lock every test that mutates a shared environment
/// variable (`ZO_TODO_STORE`, `ZO_AGENT_STORE`, API keys, …) must hold.
///
/// It is one shared mutex on purpose: env vars are process-global, so a test
/// reading one back must serialize against *every* sibling test that writes it,
/// even across modules. Per-module `static ENV_LOCK`s only serialize within
/// their own file and let cross-module tests stomp each other's `set_var`
/// (the `clear_session_scopes_todo_store_to_fresh_session` flake). All test env
/// guards route here.
///
/// LOCK-ORDER CONTRACT: acquire BEFORE [`test_cwd_lock`] (env → cwd), and
/// never a second time on the same thread — it is not re-entrant. See
/// [`test_cwd_lock`] for the deadlock this ordering prevents.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    test_env_mutex()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Install a panic hook that restores the terminal (disable raw mode +
/// leave the alternate screen + drop mouse capture) before delegating
/// to the default panic printer. Without this, a panic during a TUI
/// turn leaves the terminal in raw/alt-screen mode, which causes
/// ratatui frame bytes and ANSI escape sequences to spill into the
/// user's cooked shell as garbage.
fn install_terminal_restoring_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Only the TUI-owning thread may restore the terminal. A panic on a
        // background worker (e.g. mcp-discovery) leaves the live TUI intact —
        // tearing it down here would corrupt the screen and clear TUI_ACTIVE,
        // disarming a later real panic's restore. Before the TUI takes the
        // terminal (id unset) the original behavior is preserved.
        let on_tui_thread = match TUI_THREAD_ID.get() {
            Some(owner) => *owner == std::thread::current().id(),
            None => true,
        };
        // Only the TUI-owning thread tears down the terminal (see above);
        // `emergency_restore` itself no-ops unless the TUI is active, and pops
        // the Kitty keyboard flags that this hook could not reach directly.
        if on_tui_thread {
            session::tui_loop::emergency_restore();
        }
        default_hook(info);
    }));
}

fn main() {
    install_terminal_restoring_panic_hook();
    if let Err(error) = run() {
        // Ensure terminal is restored even on non-panic errors (e.g. IO
        // failures that bubble up from the TUI loop without hitting the
        // panic hook). Without this, a TuiLoopError leaves the terminal
        // in raw/alt-screen mode, making it appear "frozen". No-ops unless
        // the TUI is active.
        session::tui_loop::emergency_restore();
        let message = error.to_string();
        if message.contains("`zo --help`") {
            eprintln!("error: {message}");
        } else {
            eprintln!(
                "error: {message}

Run `zo --help` for usage."
            );
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // A malformed ablation spec refuses the process instead of degrading to
    // "suppress nothing". Degrading would let a typo'd control arm run the
    // full production path and still be filed as a completed experiment —
    // a measured claim about a feature's value that nothing actually tested.
    if let Some(error) = telemetry::ablation_spec_error() {
        return Err(error.into());
    }
    let args: Vec<String> = env::args().skip(1).collect();

    // Extract --cwd before parsing so the working directory is set
    // before any action that depends on the current directory.
    if let Some(cwd) = cli_args::extract_cwd_override(&args) {
        std::env::set_current_dir(&cwd).map_err(|e| {
            format!(
                "failed to change working directory to {}: {e}",
                cwd.display()
            )
        })?;
    }

    let action = parse_args(&args)?;
    main_dispatch::run_action(action)
}

pub(crate) fn filter_tool_specs(
    tool_registry: &GlobalToolRegistry,
    allowed_tools: Option<&AllowedToolSet>,
) -> Vec<ToolDefinition> {
    tool_registry.definitions(allowed_tools)
}

pub(crate) fn build_plugin_manager(
    cwd: &Path,
    loader: &ConfigLoader,
    runtime_config: &runtime::RuntimeConfig,
) -> PluginManager {
    let plugin_settings = runtime_config.plugins();
    let mut plugin_config = PluginManagerConfig::new(loader.config_home().to_path_buf());
    plugin_config.enabled_plugins = plugin_settings.enabled_plugins().clone();
    plugin_config.external_dirs = plugin_settings
        .external_directories()
        .iter()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path))
        .collect();
    plugin_config.install_root = plugin_settings
        .install_root()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    plugin_config.registry_path = plugin_settings
        .registry_path()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    plugin_config.bundled_root = plugin_settings
        .bundled_root()
        .map(|path| resolve_plugin_path(cwd, loader.config_home(), path));
    // Discover installed plugins under every lower-priority canonical home
    // (`ZO_HOME`, `$HOME/.zo`) using each root's default install layout, so a
    // plugin installed under any canonical home is still found. Writes stay on
    // the primary `config_home`-derived install root above.
    let primary = loader.config_home();
    plugin_config.discovery_install_roots = loader
        .config_roots()
        .iter()
        .filter(|root| root.as_path() != primary)
        .map(|root| root.join("plugins").join("installed"))
        .collect();
    PluginManager::new(plugin_config)
}

fn resolve_plugin_path(cwd: &Path, config_home: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else if value.starts_with('.') {
        cwd.join(path)
    } else {
        config_home.join(path)
    }
}

pub(crate) struct HookAbortMonitor {
    stop_tx: Option<Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

impl HookAbortMonitor {
    pub(crate) fn spawn(abort_signal: runtime::HookAbortSignal) -> Self {
        Self::spawn_with_waiter(abort_signal, move |stop_rx, abort_signal| {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };

            runtime.block_on(async move {
                let wait_for_stop = tokio::task::spawn_blocking(move || {
                    let _ = stop_rx.recv();
                });

                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if result.is_ok() {
                            abort_signal.abort();
                        }
                    }
                    _ = wait_for_stop => {}
                }
            });
        })
    }

    fn spawn_with_waiter<F>(abort_signal: runtime::HookAbortSignal, wait_for_interrupt: F) -> Self
    where
        F: FnOnce(Receiver<()>, runtime::HookAbortSignal) + Send + 'static,
    {
        let (stop_tx, stop_rx) = mpsc::channel();
        let join_handle = thread::spawn(move || wait_for_interrupt(stop_rx, abort_signal));

        Self {
            stop_tx: Some(stop_tx),
            join_handle: Some(join_handle),
        }
    }

    pub(crate) fn stop(mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

pub(crate) fn init_context_md() -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    Ok(initialize_repo(&cwd)?.render())
}

pub(crate) fn run_init() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", init_context_md()?);
    Ok(())
}

// Bin tests assert the historical completion list; the TUI palette builds
// its own candidates.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn slash_command_completion_candidates_with_sessions(
    model: &str,
    active_session_id: Option<&str>,
    recent_session_ids: &[String],
    prompt_commands: &[commands::PromptCommandDef],
) -> Vec<String> {
    const STATIC_COMPLETION_CANDIDATES: &[&str] = &[
        "/bughunter ",
        "/clear --confirm",
        "/config ",
        "/config env",
        "/config hooks",
        "/config model",
        "/config plugins",
        "/mcp ",
        "/mcp list",
        "/mcp show ",
        "/export ",
        "/issue ",
        "/model ",
        "/model fable",
        "/model opus",
        "/model sonnet",
        "/model haiku",
        "/permissions ",
        "/permissions read-only",
        "/permissions workspace-write",
        "/permissions danger-full-access",
        "/plugin list",
        "/plugin install ",
        "/plugin enable ",
        "/plugin disable ",
        "/plugin uninstall ",
        "/plugin update ",
        "/plugins list",
        "/pr ",
        "/resume ",
        "/session list",
        "/session switch ",
        "/session fork ",
        "/teleport ",
        "/ultraplan ",
        "/agents help",
        "/mcp help",
        "/skills help",
    ];

    fn add_session_completions(completions: &mut BTreeSet<String>, session_id: &str) {
        completions.insert(format!("/resume {session_id}"));
        completions.insert(format!("/session switch {session_id}"));
    }

    let mut completions = BTreeSet::new();

    completions.extend(
        public_slash_command_specs_iter()
            .flat_map(slash_command_names)
            .map(|name| format!("/{name}")),
    );
    completions.extend(
        STATIC_COMPLETION_CANDIDATES
            .iter()
            .map(|candidate| (*candidate).to_string()),
    );
    completions.extend(prompt_commands.iter().flat_map(|command| {
        let name = format!("/{}", command.name);
        match &command.argument_hint {
            Some(_) => vec![format!("{name} "), name],
            None => vec![name],
        }
    }));

    if !model.trim().is_empty() {
        completions.insert(format!("/model {}", resolve_model_alias(model)));
        completions.insert(format!("/model {model}"));
    }

    if let Some(active_session_id) = active_session_id.filter(|value| !value.trim().is_empty()) {
        add_session_completions(&mut completions, active_session_id);
    }

    for session_id in recent_session_ids
        .iter()
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .take(10)
    {
        add_session_completions(&mut completions, session_id);
    }

    completions.into_iter().collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod sandbox_report_tests {
    use super::{HookAbortMonitor, format_sandbox_report};
    use runtime::HookAbortSignal;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn sandbox_report_renders_expected_fields() {
        let report = format_sandbox_report(&runtime::SandboxStatus::default());
        assert!(report.contains("Sandbox"));
        assert!(report.contains("Enabled"));
        assert!(report.contains("Effective posture"));
        assert!(report.contains("Platform backend"));
        assert!(report.contains("Isolation note"));
        assert!(report.contains("Filesystem mode"));
        assert!(report.contains("Fallback reason"));
    }

    #[test]
    fn hook_abort_monitor_stops_without_aborting() {
        let abort_signal = HookAbortSignal::new();
        let (ready_tx, ready_rx) = mpsc::channel();
        let monitor = HookAbortMonitor::spawn_with_waiter(
            abort_signal.clone(),
            move |stop_rx, abort_signal| {
                ready_tx.send(()).expect("ready signal");
                let _ = stop_rx.recv();
                assert!(!abort_signal.is_aborted());
            },
        );

        ready_rx.recv().expect("waiter should be ready");
        monitor.stop();

        assert!(!abort_signal.is_aborted());
    }

    #[test]
    fn hook_abort_monitor_propagates_interrupt() {
        let abort_signal = HookAbortSignal::new();
        let (done_tx, done_rx) = mpsc::channel();
        let monitor = HookAbortMonitor::spawn_with_waiter(
            abort_signal.clone(),
            move |_stop_rx, abort_signal| {
                abort_signal.abort();
                done_tx.send(()).expect("done signal");
            },
        );

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("interrupt should complete");
        monitor.stop();

        assert!(abort_signal.is_aborted());
    }
}
