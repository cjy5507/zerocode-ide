use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use super::misc_tools::{
    AgentInput, AgentJob, MAX_COUNCIL_CANDIDATE_CHARS, MAX_COUNCIL_CANDIDATES,
    MAX_COUNCIL_LLM_JUDGE_CALLS, MAX_SPAWN_MULTI_AGENT_AGENTS, SubagentToolExecutor,
    agent_permission_policy, allowed_tools_for_subagent, classify_lane_failure,
    effective_spawn_window, execute_agent_with_spawn,
    execute_agent_with_spawn_and_parent_model_and_hooks, final_assistant_text,
    persist_agent_terminal_state, push_output_block, workflow_concurrency_limit,
};
use super::task_tools::run_task_packet;
use super::{
    GlobalToolRegistry, ToolContext, ToolError, ToolFamily, ToolInvocationResult, ToolPolicyCheck,
    ToolPolicyDecision, execute_tool, mvp_tool_specs, normalize_shell_command,
    permission_mode_from_plugin,
};
use api::OutputContentBlock;
use runtime::{
    ApiRequest, AssistantEvent, ConversationRuntime, LaneEventName, LaneFailureClass,
    PermissionMode, PermissionPolicy, RuntimeError, RuntimePermissionRuleConfig, Session,
    TaskPacket, ToolExecutor, permission_enforcer::PermissionEnforcer,
};
use serde_json::json;

/// Shared test context — cheap to create (registries are Arc-backed). Its task
/// registry is pinned to a folder of this process's own under the temp folder:
/// the first caller may run inside another test's cwd window, and the process
/// cwd must not pick the registry folder (t-21146). Not this crate's folder,
/// which it was: `.gitignore` hides `.zo/` there, and the file a task test
/// wrote stayed in `crates/tools/.zo/registries` (t-78045).
fn test_ctx() -> &'static ToolContext {
    static CTX: OnceLock<ToolContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let registries = std::env::temp_dir().join(format!("zo-tools-test-registries-{}", std::process::id()));
        ToolContext::new().with_tasks(runtime::task_registry::TaskRegistry::with_persistence_path(
            Some(registries.join("tasks.json")),
        ))
    })
}

/// Convenience wrapper matching the old two-arg signature used across tests.
fn run_tool(name: &str, input: &serde_json::Value) -> Result<String, ToolError> {
    execute_tool(test_ctx(), name, input)
}

/// Every caller holds `env_lock` (t-21146), so the context is built raw here.
fn run_tool_isolated(name: &str, input: &serde_json::Value) -> Result<String, ToolError> {
    let ctx = ToolContext::new();
    execute_tool(&ctx, name, input)
}

pub(crate) fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| {
        // Tests must never consult the developer's real Claude Code keychain:
        // an agent-spawning test would otherwise shell out to `security` and,
        // on an expired blob, fire a real token-endpoint refresh — hanging in
        // sandboxed runs and mutating live credentials in unsandboxed ones.
        std::env::set_var("ZO_DISABLE_KEYCHAIN", "1");
        // Nor the developer's own providers. A pane of the ZeroCode window
        // exports them (`ZO_CUSTOM_PROVIDERS`), and a model one of them
        // declares is read as that provider's and has no fast model of its
        // own: a test of what a session is asked on would turn on whose
        // machine ran it. A test that wants such providers declares them
        // under this lock, and puts back what it found.
        std::env::remove_var(api::CUSTOM_PROVIDERS_ENV);
        api::refresh_custom_providers_from_env();
        // Nor the developer's config home. The model catalog a person edits
        // lives there, and it is asked first whose account a model's
        // requests go on (`crate::model_home`): a row the developer wrote
        // would move a test's model to another provider. An empty home of
        // this process's own, the homes of runs that ended going first; a
        // test that wants a catalog makes its own ([`ConfigHome`]).
        let _ = core_types::ended_process_folders::remove_folders_of_ended_processes(
            &std::env::temp_dir(),
            "zo-tools-test-home-",
        );
        let home = std::env::temp_dir().join(format!("zo-tools-test-home-{}", std::process::id()));
        let _ = fs::create_dir_all(&home);
        std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, &home);
        // A sub-agent turn a test runs roots its traces (`.zo/turns`,
        // `.zo/dream`) at `ZO_TRACE_ROOT`, else at the process cwd — the
        // crate's folder, where `.gitignore` hides them (244 session logs by
        // 2026-10-28, t-78045). No test here reads a trace back.
        std::env::set_var(
            "ZO_TRACE_ROOT",
            std::env::temp_dir().join(format!("zo-tools-test-traces-{}", std::process::id())),
        );
        Mutex::new(())
    })
}

/// Readies what [`env_lock`] readies — no keychain, none of the developer's
/// providers, an empty config home — for a test that never takes the lock,
/// before the code under test reads any of them.
pub(crate) fn env_ready() {
    let _ = env_lock();
}

/// One environment variable given a value, or taken out, for as long as this
/// lives, and put back as the test found it. The [`env_lock`] guard it is
/// given is held for as long as it lives.
pub(crate) struct Var<'held> {
    key: &'static str,
    before: Option<std::ffi::OsString>,
    _held: std::marker::PhantomData<&'held std::sync::MutexGuard<'held, ()>>,
}

impl<'held> Var<'held> {
    pub(crate) fn set(
        _held: &'held std::sync::MutexGuard<'held, ()>,
        key: &'static str,
        value: impl AsRef<std::ffi::OsStr>,
    ) -> Self {
        let before = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, before, _held: std::marker::PhantomData }
    }

    pub(crate) fn unset(_held: &'held std::sync::MutexGuard<'held, ()>, key: &'static str) -> Self {
        let before = std::env::var_os(key);
        std::env::remove_var(key);
        Self { key, before, _held: std::marker::PhantomData }
    }
}

impl Drop for Var<'_> {
    fn drop(&mut self) {
        match self.before.take() {
            Some(before) => std::env::set_var(self.key, before),
            None => std::env::remove_var(self.key),
        }
    }
}

/// The person's own providers as the test found them, put back on drop. They
/// are the process's, so the [`env_lock`] guard it is given is held for as
/// long as it lives.
pub(crate) struct OwnProviders<'held> {
    before: Option<std::ffi::OsString>,
    _held: std::marker::PhantomData<&'held std::sync::MutexGuard<'held, ()>>,
}

impl<'held> OwnProviders<'held> {
    /// Declare `providers` (the `ZO_CUSTOM_PROVIDERS` JSON) for this process.
    pub(crate) fn declared(_held: &'held std::sync::MutexGuard<'held, ()>, providers: &str) -> Self {
        let before = std::env::var_os(api::CUSTOM_PROVIDERS_ENV);
        std::env::set_var(api::CUSTOM_PROVIDERS_ENV, providers);
        api::refresh_custom_providers_from_env();
        Self { before, _held: std::marker::PhantomData }
    }
}

impl Drop for OwnProviders<'_> {
    fn drop(&mut self) {
        match self.before.take() {
            Some(before) => std::env::set_var(api::CUSTOM_PROVIDERS_ENV, before),
            None => std::env::remove_var(api::CUSTOM_PROVIDERS_ENV),
        }
        api::refresh_custom_providers_from_env();
    }
}

/// A config home of the test's own while it lives, holding the model catalog
/// rows the test writes into it; the home the test found is put back on
/// drop. It changes `ZO_CONFIG_HOME`, so the [`env_lock`] guard it is given
/// is held for as long as it lives.
pub(crate) struct ConfigHome<'held> {
    home: tempfile::TempDir,
    before: Option<std::ffi::OsString>,
    _held: std::marker::PhantomData<&'held std::sync::MutexGuard<'held, ()>>,
}

impl<'held> ConfigHome<'held> {
    pub(crate) fn empty(_held: &'held std::sync::MutexGuard<'held, ()>) -> Self {
        let home = tempfile::tempdir().expect("a temp config home");
        let before = std::env::var_os(core_types::paths::ZO_CONFIG_HOME_ENV);
        std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, home.path());
        Self { home, before, _held: std::marker::PhantomData }
    }

    /// The folder: a test that keeps other stores out of the developer's
    /// home points them here too.
    pub(crate) fn path(&self) -> &Path {
        self.home.path()
    }

    /// The catalog holding exactly `rows` from here on, as an edit made in
    /// another pane leaves it while a session runs.
    pub(crate) fn catalog_now(&self, rows: &[(runtime::model_catalog::CatalogProvider, &str)]) {
        let _ = fs::remove_file(self.home.path().join(runtime::model_catalog::CATALOG_FILE));
        let mut catalog = runtime::model_catalog::ModelCatalog::load_from_home(self.home.path())
            .expect("the config home loads");
        for (provider, id) in rows {
            catalog
                .add_with_auth_route(*provider, id, id, api::AuthRoute::ApiKey)
                .expect("the row is added");
        }
    }

    /// Write a row into the catalog by hand, as a person does: `id` under
    /// `provider`, with no wire mapping, dialled with that provider's API
    /// key (a test machine holds no login).
    pub(crate) fn catalog_row(self, provider: runtime::model_catalog::CatalogProvider, id: &str) -> Self {
        runtime::model_catalog::ModelCatalog::load_from_home(self.home.path())
            .expect("the config home loads")
            .add_with_auth_route(provider, id, id, api::AuthRoute::ApiKey)
            .expect("the row is added");
        self
    }
}

impl Drop for ConfigHome<'_> {
    fn drop(&mut self) {
        match self.before.take() {
            Some(before) => std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, before),
            None => std::env::remove_var(core_types::paths::ZO_CONFIG_HOME_ENV),
        }
    }
}

/// This process's session while it lives: a client put in for a model, as
/// `zo-ide` says when it builds one (`runtime::model_home::hold_session`),
/// and nothing held when the test ends, however it ends. The record is the
/// process's, so the [`env_lock`] guard it is given is held for as long as
/// it lives.
pub(crate) struct RunningSession<'held>(std::marker::PhantomData<&'held std::sync::MutexGuard<'held, ()>>);

impl<'held> RunningSession<'held> {
    /// A session whose own client for `model` was built as the catalog and
    /// the providers read now, on the login a row pins or on any.
    pub(crate) fn built_now(_held: &'held std::sync::MutexGuard<'held, ()>, model: &str) -> Self {
        runtime::model_home::hold_session(model, Self::read_now(model));
        Self(std::marker::PhantomData)
    }

    /// A client of the session's built for `model` as the catalog reads now,
    /// as `zo-ide` says of each: its own client built again, or the client a
    /// turn's leg is given for another model. Asked of a session that is
    /// held, so that what it adds is forgotten with it.
    #[allow(clippy::unused_self)]
    pub(crate) fn client_built_now(&self, model: &str) {
        runtime::model_home::hold(model, Self::read_now(model));
    }

    fn read_now(model: &str) -> runtime::model_home::Held {
        let catalog = runtime::model_catalog::ModelCatalog::load().ok();
        let named = |catalog: &runtime::model_catalog::ModelCatalog| catalog.provider_for_model(model.trim());
        runtime::model_home::Held {
            home: runtime::model_home::ModelHome::read(catalog.as_ref(), model),
            catalog_provider: catalog.as_ref().and_then(named),
            auth_route: catalog
                .as_ref()
                .and_then(|catalog| catalog.auth_route_for_model(model.trim()))
                .unwrap_or(api::AuthRoute::Auto),
        }
    }
}

impl Drop for RunningSession<'_> {
    fn drop(&mut self) {
        runtime::model_home::forget_session();
    }
}

/// A `ps` first on `PATH` while it lives, which notes each run and then
/// gives the listing the real `ps` gave when it started — how many times the
/// kill guard read the processes for one call (t-84911). It never runs the
/// real `ps` itself, so a test does not turn on how fast `ps` is under load;
/// start it after the processes a test judges. It changes `PATH`, so the
/// `env_lock` guard it is given is held for as long as it lives.
#[cfg(unix)]
pub(crate) struct NotingPs<'held> {
    dir: PathBuf,
    path: Option<std::ffi::OsString>,
    _held: std::marker::PhantomData<&'held std::sync::MutexGuard<'held, ()>>,
}

#[cfg(unix)]
impl<'held> NotingPs<'held> {
    pub(crate) fn start(_held: &'held std::sync::MutexGuard<'held, ()>) -> Self {
        use std::os::unix::fs::PermissionsExt as _;

        let path = std::env::var_os("PATH");
        let real = path
            .iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join("ps"))
            .find(|ps| ps.is_file())
            .expect("a ps on PATH");
        let dir = std::env::temp_dir().join(format!("zo-tools-noting-ps-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("the noting ps's folder");
        // The listing is taken once, now, with the real `ps` and the guard's
        // own arguments: the guard then reads it without waiting on `ps`,
        // whose 800 ms wall a loaded machine overran (t-84911 flake).
        let listing = Command::new(&real)
            .args(["-ww", "-eo", "pid=,ppid=,pgid=,command="])
            .env("LC_ALL", "C")
            .output()
            .expect("the real ps lists the processes");
        assert!(listing.status.success(), "the real ps lists the processes");
        fs::write(dir.join("listing"), &listing.stdout).expect("the listing is kept");
        let script = format!(
            "#!/bin/sh\necho read >> '{}'\nexec cat '{}'\n",
            dir.join("log").display(),
            dir.join("listing").display()
        );
        fs::write(dir.join("ps"), script).expect("the noting ps is written");
        fs::set_permissions(dir.join("ps"), fs::Permissions::from_mode(0o755)).expect("the noting ps runs");
        // macOS looks a new executable over the first time it runs — 0.13 to
        // 0.38 s on 11-10 for this script, more on a loaded machine — and the
        // guard gives `ps` 800 ms: the guard's run was that first run, and
        // overran it. The first run happens here, outside the wall, and what
        // it noted is cleared.
        let warmed = Command::new(dir.join("ps"))
            .stdout(std::process::Stdio::null())
            .status()
            .expect("the noting ps runs once");
        assert!(warmed.success(), "the noting ps runs once");
        fs::remove_file(dir.join("log")).expect("the warming run is forgotten");
        let mut dirs = vec![dir.clone()];
        dirs.extend(path.iter().flat_map(std::env::split_paths));
        std::env::set_var("PATH", std::env::join_paths(dirs).expect("a PATH"));
        Self {
            dir,
            path,
            _held: std::marker::PhantomData,
        }
    }

    /// How many times it ran.
    pub(crate) fn reads(&self) -> usize {
        fs::read_to_string(self.dir.join("log")).unwrap_or_default().lines().count()
    }
}

#[cfg(unix)]
impl Drop for NotingPs<'_> {
    fn drop(&mut self) {
        match self.path.take() {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The threads waiting for `env_lock` that said so first (t-57711). A test
/// that holds a cwd window open closes it once the colliding call waits for
/// the lock, so the collision needs no timing.
fn env_lock_waiters() -> &'static Mutex<Vec<thread::ThreadId>> {
    static WAITERS: OnceLock<Mutex<Vec<thread::ThreadId>>> = OnceLock::new();
    WAITERS.get_or_init(Mutex::default)
}

/// Whether `waiter` waits for `env_lock` and said so.
pub(crate) fn waits_for_env_lock(waiter: thread::ThreadId) -> bool {
    env_lock_waiters()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&waiter)
}

/// `env_lock`, taken by a thread that first says it waits for it.
pub(crate) fn lock_env_announced() -> std::sync::MutexGuard<'static, ()> {
    let me = thread::current().id();
    env_lock_waiters()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(me);
    let guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    env_lock_waiters()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|waiter| *waiter != me);
    guard
}

/// A `ToolContext` for tests that may run beside another test's `set_current_dir`
/// window (t-21146). `ToolContext::new()` binds the process cwd's `.zo/registries`
/// when it is built, so the construction waits for `env_lock`, which each window's
/// owner holds for the whole window. Do not call it while holding `env_lock`:
/// std's mutex does not re-enter.
pub(crate) fn tool_context_outside_cwd_windows() -> ToolContext {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ToolContext::new()
}

/// `GlobalToolRegistry::builtin()` for callers that do not hold `env_lock`: the
/// registry's own `ToolContext` binds the process cwd (t-21146).
pub(crate) fn builtin_registry_outside_cwd_windows() -> GlobalToolRegistry {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    GlobalToolRegistry::builtin()
}

/// `GlobalToolRegistry::with_plugin_tools` for callers that do not hold `env_lock`
/// (t-21146): it builds a registry with its own `ToolContext`.
pub(crate) fn with_plugin_tools_outside_cwd_windows(
    plugin_tools: Vec<plugins::PluginTool>,
) -> Result<GlobalToolRegistry, ToolError> {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    GlobalToolRegistry::with_plugin_tools(plugin_tools)
}

/// `SubagentToolExecutor::new` for callers that do not hold `env_lock` (t-21146).
pub(crate) fn subagent_executor_outside_cwd_windows(
    allowed_tools: BTreeSet<String>,
) -> crate::misc_tools::agent_tools::SubagentToolExecutor {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::misc_tools::agent_tools::SubagentToolExecutor::new(allowed_tools)
}

/// A scoped environment override shared by tests that use process-wide state.
pub(crate) struct EnvGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
    extra: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    pub(crate) fn set(key: &'static str, value: &str) -> Self {
        // Crate-wide lock: `custom.rs` tests mutate the same
        // `ZO_AGENT_DEFS_DIR`, so a module-local mutex cannot exclude them.
        let lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self {
            key,
            previous,
            extra: Vec::new(),
            _lock: lock,
        }
    }

    pub(crate) fn clear(key: &'static str) -> Self {
        let lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::env::var_os(key);
        std::env::remove_var(key);
        Self {
            key,
            previous,
            extra: Vec::new(),
            _lock: lock,
        }
    }

    /// Apply one fixture's overrides while acquiring the process environment lock once.
    #[cfg(unix)]
    pub(crate) fn set_many(values: impl IntoIterator<Item = (&'static str, Option<String>)>) -> Self {
        let mut values = values.into_iter();
        let (key, value) = values.next().expect("a fixture environment must contain at least one override");
        let mut guard = match value {
            Some(value) => Self::set(key, &value),
            None => Self::clear(key),
        };
        for (key, value) in values {
            let previous = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            guard.extra.push((key, previous));
        }
        guard
    }

    pub(crate) fn set_also(mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        self.extra.push((key, previous));
        self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, previous) in self.extra.drain(..).rev() {
            if let Some(value) = previous {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        if let Some(value) = self.previous.take() {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

/// RAII guard for web-fetch tests that hit a loopback `TestServer`: holds
/// `env_lock` and enables `ZO_WEB_ALLOW_LOCAL` so the SSRF guard permits the
/// deliberate local target, restoring the env on drop. Serializes against the
/// other env-touching tests via the shared lock.
pub(crate) struct AllowLocalWeb(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

pub(crate) fn allow_local_web() -> AllowLocalWeb {
    let guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::set_var("ZO_WEB_ALLOW_LOCAL", "1");
    AllowLocalWeb(guard)
}

impl Drop for AllowLocalWeb {
    fn drop(&mut self) {
        std::env::remove_var("ZO_WEB_ALLOW_LOCAL");
    }
}

fn temp_path(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("zo-tools-{unique}-{name}"))
}

fn sandbox_disabled_cwd(name: &str) -> PathBuf {
    let root = temp_path(name);
    fs::create_dir_all(root.join(".zo")).expect("create sandbox config dir");
    fs::write(
        root.join(".zo").join("settings.json"),
        r#"{"sandbox":{"enabled":false}}"#,
    )
    .expect("write sandbox settings");
    root
}

/// For callers that already hold `env_lock` (t-21146): the context is built raw.
fn run_tool_in_cwd(name: &str, input: &serde_json::Value, cwd: &Path) -> Result<String, ToolError> {
    execute_tool(&ToolContext::new().with_cwd(cwd.to_path_buf()), name, input)
}

/// The `run_tool_in_cwd` for callers that do not hold `env_lock` (t-21146). The lock
/// covers the run as well as the construction: a bash call spawns through `PATH`,
/// and the tests that point `PATH` elsewhere hold `env_lock` while they do.
fn run_tool_in_cwd_outside_windows(
    name: &str,
    input: &serde_json::Value,
    cwd: &Path,
) -> Result<String, ToolError> {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    execute_tool(&ToolContext::new().with_cwd(cwd.to_path_buf()), name, input)
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|error| panic!("git {} failed: {error}", args.join(" ")));
    assert!(
        status.success(),
        "git {} exited with {status}",
        args.join(" ")
    );
}

fn init_git_repo(path: &Path) {
    std::fs::create_dir_all(path).expect("create repo");
    run_git(path, &["init", "--quiet", "-b", "main"]);
    run_git(path, &["config", "user.email", "tests@example.com"]);
    run_git(path, &["config", "user.name", "Tools Tests"]);
    std::fs::write(path.join("README.md"), "initial\n").expect("write readme");
    run_git(path, &["add", "README.md"]);
    run_git(path, &["commit", "-m", "initial commit", "--quiet"]);
}

fn commit_file(path: &Path, file: &str, contents: &str, message: &str) {
    std::fs::write(path.join(file), contents).expect("write file");
    run_git(path, &["add", file]);
    run_git(path, &["commit", "-m", message, "--quiet"]);
}

fn permission_policy_for_mode(mode: PermissionMode) -> PermissionPolicy {
    mvp_tool_specs()
        .iter()
        .fold(PermissionPolicy::new(mode), |policy, spec| {
            policy.with_tool_requirement(spec.name, spec.required_permission)
        })
}

mod agent;
mod apply_patch;
mod files_web;
mod foreground_boundary;
mod permissions_misc;
mod registry;
mod shell;
mod wire_names;

// --- Shared HTTP test infrastructure (used by the web fetch/search tests) ---

struct TestServer {
    addr: SocketAddr,
    shutdown: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn spawn(handler: Arc<dyn Fn(&str) -> HttpResponse + Send + Sync + 'static>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        listener
            .set_nonblocking(true)
            .expect("set nonblocking listener");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = std::sync::mpsc::channel::<()>();

        let handle = thread::spawn(move || {
            loop {
                if rx.try_recv().is_ok() {
                    break;
                }

                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted streams inherit the listener's non-blocking flag on
                        // macOS/BSD; force blocking so the request read waits for bytes
                        // instead of panicking with WouldBlock.
                        stream.set_nonblocking(false).expect("set blocking stream");
                        let mut buffer = [0_u8; 4096];
                        let size = stream.read(&mut buffer).expect("read request");
                        let request = String::from_utf8_lossy(&buffer[..size]).into_owned();
                        let request_line = request.lines().next().unwrap_or_default().to_string();
                        let response = handler(&request_line);
                        // A client may hang up before the whole response is
                        // out (WebFetch does at its body cap); that ends the
                        // exchange, and the client asserts what it received.
                        let _ = stream.write_all(response.to_bytes().as_slice());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("server accept failed: {error}"),
                }
            }
        });

        Self {
            addr,
            shutdown: Some(tx),
            handle: Some(handle),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop the server; `Err` when its thread ended in a panic.
    fn stop(mut self) -> thread::Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.handle.take().map_or(Ok(()), thread::JoinHandle::join)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.join().expect("join test server");
        }
    }
}

struct HttpResponse {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: String,
}

impl HttpResponse {
    fn html(status: u16, reason: &'static str, body: &str) -> Self {
        Self {
            status,
            reason,
            content_type: "text/html; charset=utf-8",
            body: body.to_string(),
        }
    }

    fn text(status: u16, reason: &'static str, body: &str) -> Self {
        Self {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            body: body.to_string(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        format!(
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                self.status,
                self.reason,
                self.content_type,
                self.body.len(),
                self.body
            )
            .into_bytes()
    }
}
