use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use runtime::lsp_client::{LspRegistry, LspServerStatus, LspStdioTransport};
use runtime::McpStdioTransport;

/// LSP runtime state.
///
/// LSP initialization requires driving async transport setup from a sync call
/// site (`build_runtime_plugin_state_with_loader`). The call site may or may
/// not already be inside a tokio runtime:
///
/// * When called from a plain sync `main()` (no ambient runtime), we own a
///   fallback multi-threaded runtime stored in `owned_runtime` and `block_on`
///   it directly.
/// * When called from inside an existing tokio runtime (e.g. `#[tokio::main]`
///   or `#[tokio::test]`), calling `Runtime::new().block_on(...)` panics with
///   "Cannot start a runtime from within a runtime." Instead we delegate to
///   the shared [`api::sync_bridge::run_blocking`], which re-enters a
///   multi-thread ambient runtime via `block_in_place` and routes a
///   `current_thread` one to its private fallback runtime.
///
/// Both creation and `shutdown` go through [`Self::run_blocking`] so the same
/// bridge is used consistently for spawn and teardown.
pub(crate) struct RuntimeLspState {
    /// Fallback runtime used when no ambient tokio runtime exists. `None`
    /// means we rely on `Handle::current()` at call time.
    owned_runtime: Option<tokio::runtime::Runtime>,
    pub(crate) registry: LspRegistry,
    transports: Vec<LspStdioTransport>,
}

/// Maximum time to wait for a single LSP server to finish the
/// `initialize` handshake before giving up and booting without it.
/// Mirrors the MCP stdio init budget so one hung server (e.g. a
/// misconfigured rust-analyzer) can't freeze the session before the
/// banner.
#[cfg(not(test))]
const LSP_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// The test profile gets a loaded-machine allowance instead: under a full
/// parallel suite the init-only fixture server intermittently missed the
/// interactive 5s budget (2/80 loaded stress rounds), and the tests that
/// exercise the timeout path inject their own short deadline explicitly.
#[cfg(test)]
const LSP_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const LSP_AUTODETECT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);
static RUST_ANALYZER_USABILITY_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, bool>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

struct LspStartupConfig {
    language: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    root_path: Option<String>,
    capabilities: Vec<String>,
    explicit: bool,
}


enum LspStartupFailure {
    Startup(String),
    TimedOut(std::time::Duration),
}

impl LspStartupFailure {
    fn message(&self) -> String {
        match self {
            Self::Startup(error) => error.clone(),
            Self::TimedOut(timeout) => format!(
                "initialize timed out after {}s",
                timeout.as_secs()
            ),
        }
    }
}

impl RuntimeLspState {
    pub(crate) fn new(
        cwd: &std::path::Path,
        runtime_config: &runtime::RuntimeConfig,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        Self::new_with_startup_configs(cwd, lsp_startup_configs(cwd, runtime_config))
    }

    fn new_with_startup_configs(
        cwd: &std::path::Path,
        startup_configs: Vec<LspStartupConfig>,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        Self::new_with_startup_configs_and_timeout(cwd, startup_configs, LSP_INIT_TIMEOUT)
    }

    fn new_with_startup_configs_and_timeout(
        cwd: &std::path::Path,
        startup_configs: Vec<LspStartupConfig>,
        init_timeout: std::time::Duration,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        if startup_configs.is_empty() {
            return Ok(None);
        }

        // If we are already inside a tokio runtime, piggy-back on it. Otherwise
        // bring up a dedicated multi-thread runtime we own for blocking calls.
        let owned_runtime = if tokio::runtime::Handle::try_current().is_ok() {
            None
        } else {
            Some(bootstrap_runtime()?)
        };

        let mut state = Self {
            owned_runtime,
            registry: LspRegistry::new(),
            transports: Vec::new(),
        };

        for config in startup_configs {
            let root_path = config
                .root_path
                .as_deref()
                .unwrap_or_else(|| cwd.to_str().unwrap_or("."));
            // Bound LSP init so a server that hangs on `initialize`
            // cannot freeze session boot. A timed-out or failing server
            // is non-fatal: it is registered as `Error` and the session
            // boots without it.
            let spawn = LspStdioTransport::spawn_initialized(
                McpStdioTransport {
                    command: config.command.clone(),
                    args: config.args.clone(),
                    env: config.env.clone(),
                    tool_call_timeout_ms: None,
                },
                Some(root_path),
            );
            let outcome = state.run_blocking(async {
                match tokio::time::timeout(init_timeout, spawn).await {
                    Ok(Ok(transport)) => Ok(transport),
                    Ok(Err(error)) => Err(LspStartupFailure::Startup(error)),
                    Err(_) => Err(LspStartupFailure::TimedOut(init_timeout)),
                }
            });
            match outcome {
                Ok(transport) => {
                    state.registry.register_with_transport(
                        &config.language,
                        LspServerStatus::Connected,
                        Some(root_path),
                        config.capabilities.clone(),
                        Some(Arc::new(transport.clone())),
                    );
                    state.transports.push(transport);
                }
                Err(error) => {
                    let message = error.message();
                    if config.explicit {
                        eprintln!(
                            "zo: LSP server '{}' unavailable ({message}); continuing without it",
                            config.language
                        );
                    }
                    state.registry.register_with_transport(
                        &config.language,
                        LspServerStatus::Error,
                        Some(root_path),
                        config.capabilities.clone(),
                        None,
                    );
                    // Auto-detect filters out known-unusable candidates (notably
                    // rustup's `rust-analyzer` proxy with no installed component)
                    // before startup. Once a server has been selected, failures
                    // and timeouts are surfaced so the sidebar does not silently
                    // hide a real LSP startup problem.
                }
            }
        }

        Ok(Some(state))
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // When we own a dedicated runtime (LSP built at sync startup), its child
        // processes and spawned reader loops live on THAT runtime. A live
        // `/permission` or `/model` rebuild calls this from inside the TUI's
        // ambient async runtime — and both `block_on`'ing the owned runtime
        // (terminate) AND dropping it from there freeze the TUI main thread (the
        // Shift+Tab hang: a prior `block_in_place(owned_runtime.block_on(..))`
        // guard only traded the panic for this hang). Hand the entire teardown to
        // a detached OS thread that has NO ambient runtime, where `block_on` and
        // the runtime drop are both safe and never touch the UI thread. The
        // rebuild returns immediately, so typing/scrolling stay responsive.
        if let Some(runtime) = self.owned_runtime.take() {
            let transports = std::mem::take(&mut self.transports);
            std::thread::spawn(move || {
                runtime.block_on(async {
                    for transport in &transports {
                        let _ = transport.terminate().await;
                    }
                });
                drop(transports);
                drop(runtime);
            });
            return Ok(());
        }
        // No owned runtime: the LSP was built inside an ambient runtime, so its
        // children belong to that runtime and terminate through the shared sync
        // bridge without nesting a second runtime.
        let transports = std::mem::take(&mut self.transports);
        for transport in &transports {
            self.run_blocking(transport.terminate())
                .map_err(std::io::Error::other)?;
        }
        self.transports = transports;
        Ok(())
    }

    /// Bridge a `Future` to a synchronous result without nesting tokio
    /// runtimes. See the type-level doc for the rationale.
    fn run_blocking<F: Future>(&self, fut: F) -> F::Output {
        if let Some(runtime) = self.owned_runtime.as_ref() {
            // Spawned LSP reader loops live on this runtime
            // (`lsp_client::transport` does `tokio::spawn(reader_loop(..))`
            // at connect time), and its worker thread keeps driving them
            // between bridge calls — so it must stay the driver whenever we
            // own one. The shared fallback runtime only makes progress
            // while a caller is parked inside it.
            //
            // But `owned_runtime` is built at startup (sync `main`, no ambient
            // runtime). A later `/permission` or `/model` rebuild calls
            // `replace_runtime` → `shutdown_lsp` → here from *inside* the TUI's
            // ambient multi-thread runtime. Calling `runtime.block_on(..)`
            // there panics ("Cannot start a runtime from within a runtime").
            // `block_in_place` parks the ambient worker so the owned runtime
            // can drive `fut`; without an ambient runtime (startup) we call
            // `block_on` directly as before.
            if matches!(
                tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()),
                Ok(tokio::runtime::RuntimeFlavor::MultiThread)
            ) {
                return tokio::task::block_in_place(|| runtime.block_on(fut));
            }
            return runtime.block_on(fut);
        }
        api::sync_bridge::run_blocking(fut)
    }
}

fn bootstrap_runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
}

fn lsp_startup_configs(
    cwd: &std::path::Path,
    runtime_config: &runtime::RuntimeConfig,
) -> Vec<LspStartupConfig> {
    let mut configs = Vec::new();
    for scoped in runtime_config.lsp().servers().values() {
        let config = &scoped.config;
        configs.push(LspStartupConfig {
            language: config.language.clone(),
            command: config.command.clone(),
            args: config.args.clone(),
            env: config.env.clone(),
            root_path: config.root_path.clone(),
            capabilities: config.capabilities.clone(),
            explicit: true,
        });
    }

    // Auto-detection spawns a language server at session start, which adds the
    // server's `initialize` latency to boot. Explicit (user-configured) servers
    // always run; auto-detection is default-on but can be turned off with
    // `ZO_LSP_AUTODETECT=0` so latency-sensitive one-shots (benchmarks, CI
    // `-p` runs) opt out without losing configured servers.
    if !autodetect_enabled(std::env::var("ZO_LSP_AUTODETECT").ok().as_deref()) {
        return configs;
    }

    for language in detect_project_languages(cwd) {
        if configs.iter().any(|config| config.language == language) {
            continue;
        }
        if let Some(command) = detect_language_server(language) {
            configs.push(LspStartupConfig {
                language: language.to_string(),
                command: command.to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                root_path: cwd.to_str().map(str::to_string),
                capabilities: Vec::new(),
                explicit: false,
            });
        }
    }

    configs
}

fn detect_project_languages(cwd: &std::path::Path) -> Vec<&'static str> {
    let mut languages = Vec::new();
    if cwd.join("Cargo.toml").exists() {
        push_unique(&mut languages, "rust");
    }
    if cwd.join("go.mod").exists() {
        push_unique(&mut languages, "go");
    }
    if cwd.join("pyproject.toml").exists() || cwd.join("requirements.txt").exists() {
        push_unique(&mut languages, "python");
    }
    if cwd.join("tsconfig.json").exists() {
        push_unique(&mut languages, "typescript");
    }
    if cwd.join("package.json").exists() {
        push_unique(&mut languages, "javascript");
    }
    detect_languages_from_files(cwd, 0, &mut languages);
    languages
}

fn detect_languages_from_files(path: &Path, depth: usize, languages: &mut Vec<&'static str>) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if matches!(name, ".git" | ".zo" | "node_modules" | "target") {
                continue;
            }
            detect_languages_from_files(&path, depth + 1, languages);
            continue;
        }
        match path.extension().and_then(OsStr::to_str) {
            Some("rs") => push_unique(languages, "rust"),
            Some("ts" | "tsx") => push_unique(languages, "typescript"),
            Some("js" | "jsx") => push_unique(languages, "javascript"),
            Some("py") => push_unique(languages, "python"),
            Some("go") => push_unique(languages, "go"),
            _ => {}
        }
    }
}

fn push_unique(languages: &mut Vec<&'static str>, language: &'static str) {
    if !languages.contains(&language) {
        languages.push(language);
    }
}

/// Whether LSP server auto-detection is enabled. Default on; an explicit
/// `0`/`false`/`off`/`no` (from `ZO_LSP_AUTODETECT`) disables it.
fn autodetect_enabled(raw: Option<&str>) -> bool {
    !matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

fn detect_language_server(language: &str) -> Option<&'static str> {
    detect_language_server_in_path(language, std::env::var_os("PATH").as_deref())
}

fn detect_language_server_in_path(language: &str, path: Option<&OsStr>) -> Option<&'static str> {
    let candidates: &[&str] = match language {
        "rust" => &["rust-analyzer"],
        "typescript" | "javascript" => &["typescript-language-server"],
        "python" => &["pyright", "pyright-langserver"],
        "go" => &["gopls"],
        _ => return None,
    };
    candidates.iter().copied().find(|command| {
        let Some(candidate_path) = command_path_in_path(command, path) else {
            return false;
        };
        *command != "rust-analyzer" || rust_analyzer_candidate_usable(&candidate_path)
    })
}

fn command_path_in_path(command: &str, path: Option<&OsStr>) -> Option<std::path::PathBuf> {
    let path = path?;
    std::env::split_paths(path).find_map(|dir| {
        let candidate = dir.join(command);
        (candidate.is_file() && is_executable(&candidate)).then_some(candidate)
    })
}

fn rust_analyzer_candidate_usable(path: &Path) -> bool {
    let key = path.to_path_buf();
    if let Some(cached) = RUST_ANALYZER_USABILITY_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied()
    {
        return cached;
    }
    let usable = command_succeeds_with_timeout(path, "--version", LSP_AUTODETECT_PROBE_TIMEOUT);
    RUST_ANALYZER_USABILITY_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, usable);
    usable
}

fn command_succeeds_with_timeout(path: &Path, arg: &str, timeout: std::time::Duration) -> bool {
    let Ok(mut child) = std::process::Command::new(path)
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub(crate) type LspStateResult = (Option<Arc<Mutex<RuntimeLspState>>>, LspRegistry);

pub(crate) fn build_runtime_lsp_state(
    cwd: &std::path::Path,
    runtime_config: &runtime::RuntimeConfig,
) -> Result<LspStateResult, Box<dyn std::error::Error>> {
    let Some(lsp_state) = RuntimeLspState::new(cwd, runtime_config)? else {
        return Ok((None, LspRegistry::new()));
    };
    let registry = lsp_state.registry.clone();
    Ok((Some(Arc::new(Mutex::new(lsp_state))), registry))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use runtime::lsp_client::LspServerStatus;
    use runtime::ConfigLoader;

    use super::{
        autodetect_enabled, build_runtime_lsp_state, detect_language_server_in_path,
        detect_project_languages, LspStartupConfig, RuntimeLspState,
    };

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-lsp-runtime-{label}-{unique}"))
    }

    fn write_init_only_lsp_server() -> PathBuf {
        let root = temp_dir("server");
        fs::create_dir_all(&root).expect("temp dir");
        let script_path = root.join("fake-lsp-init.py");
        let script = [
            "#!/usr/bin/env python3",
            "import json, sys",
            "",
            "def read_message():",
            "    header = b''",
            r"    while not header.endswith(b'\r\n\r\n'):",
            "        chunk = sys.stdin.buffer.read(1)",
            "        if not chunk:",
            "            return None",
            "        header += chunk",
            "    length = 0",
            r"    for line in header.decode().split('\r\n'):",
            r"        if line.lower().startswith('content-length:'):",
            r"            length = int(line.split(':', 1)[1].strip())",
            "    payload = sys.stdin.buffer.read(length)",
            "    return json.loads(payload.decode())",
            "",
            "def send_message(message):",
            "    payload = json.dumps(message).encode()",
            r"    sys.stdout.buffer.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)",
            "    sys.stdout.buffer.flush()",
            "",
            "while True:",
            "    request = read_message()",
            "    if request is None:",
            "        break",
            "    if request['method'] == 'initialize':",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'result': {'capabilities': {'hoverProvider': True}, 'serverInfo': {'name': 'fake-lsp', 'version': '0.1.0'}}",
            "        })",
            "    else:",
            "        send_message({",
            "            'jsonrpc': '2.0',",
            "            'id': request['id'],",
            "            'error': {'code': -32601, 'message': 'unsupported'}",
            "        })",
            "",
        ]
        .join("\n");
        fs::write(&script_path, script).expect("write script");
        let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("chmod");
        script_path
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_runtime_lsp_state_does_not_panic_inside_ambient_runtime() {
        // Regression: previously `RuntimeLspState::new` called
        // `Runtime::new().block_on(...)` which panics with
        // "Cannot start a runtime from within a runtime" when invoked from an
        // existing tokio context. Exercise exactly that path.
        let root = temp_dir("ambient");
        let cwd = root.join("project");
        let home = root.join("home").join(".zo");
        fs::create_dir_all(cwd.join(".zo")).expect("project config dir");
        fs::create_dir_all(&home).expect("home config dir");
        // Project LSP servers spawn a process at boot, so they are supply-chain
        // gated; opt in from the trusted User scope for this LSP-behavior test.
        fs::write(home.join("settings.json"), r#"{"enableAllProjectLsp":true}"#)
            .expect("write lsp opt-in");

        let script_path = write_init_only_lsp_server();
        let settings = format!(
            r#"{{
              "lspServers": {{
                "rust-analyzer": {{
                  "language": "rust",
                  "command": "{}",
                  "args": [],
                  "capabilities": ["hover"]
                }}
              }}
            }}"#,
            script_path.display()
        );
        fs::write(cwd.join(".zo").join("settings.json"), settings).expect("write settings");

        let cwd_clone = cwd.clone();
        let home_clone = home.clone();
        let (state, registry) = tokio::task::spawn_blocking(move || {
            let runtime_config = ConfigLoader::new(&cwd_clone, &home_clone)
                .load()
                .expect("config should load");
            build_runtime_lsp_state(&cwd_clone, &runtime_config).expect("lsp state should build")
        })
        .await
        .expect("spawn_blocking join");

        let state = state.expect("configured lsp state should exist");
        assert!(registry.get("rust").is_some());

        tokio::task::spawn_blocking(move || {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .shutdown()
                .expect("shutdown lsp state from ambient runtime");
        })
        .await
        .expect("shutdown join");

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lsp_state_shutdown_inside_ambient_runtime_does_not_panic() {
        // Regression for the Shift+Tab / `/permission` freeze: an LSP state built
        // at startup (sync, no ambient runtime) owns its own runtime
        // (`owned_runtime = Some`). A later permission/model rebuild calls
        // `replace_runtime` → `shutdown_lsp` from *inside* the TUI's ambient
        // multi-thread runtime. Doing `owned_runtime.block_on(..)` there panicked
        // ("Cannot start a runtime from within a runtime"); guarding it with
        // `block_in_place` only traded the panic for a HANG (the owned runtime's
        // single-worker driver can't make progress driven from the parked TUI
        // worker, and dropping it there blocks too). The fix offloads the whole
        // teardown to a detached OS thread, so `shutdown` returns promptly on the
        // calling thread and the owned runtime is taken off it. Build the state
        // off the runtime (so it owns one), then shut it down *on* the runtime.
        let root = temp_dir("ambient-shutdown");
        let cwd = root.join("project");
        let home = root.join("home").join(".zo");
        fs::create_dir_all(cwd.join(".zo")).expect("project config dir");
        fs::create_dir_all(&home).expect("home config dir");
        // Project LSP servers spawn a process at boot, so they are supply-chain
        // gated; opt in from the trusted User scope for this LSP-behavior test.
        fs::write(home.join("settings.json"), r#"{"enableAllProjectLsp":true}"#)
            .expect("write lsp opt-in");

        let script_path = write_init_only_lsp_server();
        let settings = format!(
            r#"{{
              "lspServers": {{
                "rust-analyzer": {{
                  "language": "rust",
                  "command": "{}",
                  "args": [],
                  "capabilities": ["hover"]
                }}
              }}
            }}"#,
            script_path.display()
        );
        fs::write(cwd.join(".zo").join("settings.json"), settings).expect("write settings");

        let cwd_clone = cwd.clone();
        let home_clone = home.clone();
        // Build on a plain OS thread with NO ambient tokio runtime — exactly
        // how startup builds it in sync `main`. `spawn_blocking` would NOT
        // work here: its threads still expose `Handle::current()`, so the
        // state would take the `owned_runtime = None` path and never exercise
        // the crash. A bare thread forces `owned_runtime = Some`.
        let (state, _registry) = std::thread::spawn(move || {
            let runtime_config = ConfigLoader::new(&cwd_clone, &home_clone)
                .load()
                .expect("config should load");
            build_runtime_lsp_state(&cwd_clone, &runtime_config).expect("lsp state should build")
        })
        .join()
        .expect("thread join");
        let state = state.expect("configured lsp state should exist");

        // Shut down directly on the ambient multi-thread runtime — the exact
        // context `replace_runtime` runs in. Must return promptly (no panic, no
        // hang): the owned runtime is moved to a detached thread for teardown.
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown()
            .expect("shutdown lsp state from inside the ambient runtime");

        // The owned runtime was taken off the calling thread (handed to the
        // detached teardown thread), so nothing tokio is left to drop here — the
        // drop is now trivially safe even on the ambient runtime, no
        // `block_in_place` needed.
        assert!(
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .owned_runtime
                .is_none(),
            "shutdown must take the owned runtime off the calling thread"
        );
        drop(state);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn build_runtime_lsp_state_registers_configured_stdio_server() {
        let root = temp_dir("config");
        let cwd = root.join("project");
        let home = root.join("home").join(".zo");
        fs::create_dir_all(cwd.join(".zo")).expect("project config dir");
        fs::create_dir_all(&home).expect("home config dir");
        // Project LSP servers spawn a process at boot, so they are supply-chain
        // gated; opt in from the trusted User scope for this LSP-behavior test.
        fs::write(home.join("settings.json"), r#"{"enableAllProjectLsp":true}"#)
            .expect("write lsp opt-in");

        let script_path = write_init_only_lsp_server();
        let settings = format!(
            r#"{{
              "lspServers": {{
                "rust-analyzer": {{
                  "language": "rust",
                  "command": "{}",
                  "args": [],
                  "capabilities": ["hover", "definition"]
                }}
              }}
            }}"#,
            script_path.display()
        );
        fs::write(cwd.join(".zo").join("settings.json"), settings).expect("write settings");

        let runtime_config = ConfigLoader::new(&cwd, &home)
            .load()
            .expect("config should load");
        let (state, registry) =
            build_runtime_lsp_state(&cwd, &runtime_config).expect("lsp state should build");

        let state = state.expect("configured lsp state should exist");
        let server = registry
            .get("rust")
            .expect("rust server should be registered");
        assert_eq!(
            server.status,
            runtime::lsp_client::LspServerStatus::Connected
        );
        assert!(server.transport.is_some());
        assert!(server
            .capabilities
            .iter()
            .any(|capability| capability == "hover"));

        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown()
            .expect("shutdown lsp state");

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }


    #[test]
    fn project_language_detection_skips_root_and_nested_zo_operational_state() {
        let root = temp_dir("private-zo-state");
        let root_state = root.join(".zo").join("turns");
        let nested_state = root.join("nested").join(".zo").join("dream");
        fs::create_dir_all(&root_state).expect("root Zo state dir");
        fs::create_dir_all(&nested_state).expect("nested Zo state dir");
        fs::write(root_state.join("private.rs"), "fn private_state() {}\n")
            .expect("root private source");
        fs::write(nested_state.join("private.py"), "print('private state')\n")
            .expect("nested private source");

        let languages = detect_project_languages(&root);
        assert!(
            !languages.contains(&"rust") && !languages.contains(&"python"),
            "Zo operational state must not drive language-server auto-discovery: {languages:?}"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn autodetect_skips_unusable_rust_analyzer_proxy() {
        let root = temp_dir("broken-proxy");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        let server = bin.join("rust-analyzer");
        fs::write(&server, "#!/bin/sh
echo 'rustup component missing' >&2
exit 42
")
            .expect("write fake broken proxy");
        let mut permissions = fs::metadata(&server).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&server, permissions).expect("chmod");

        assert_eq!(
            detect_language_server_in_path("rust", Some(bin.as_os_str())),
            None,
            "an executable but unusable rust-analyzer proxy must not create a permanent HUD error"
        );
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn autodetected_lsp_timeout_registers_error_server() {
        let root = temp_dir("auto-timeout");
        let cwd = root.join("project");
        let bin = root.join("bin");
        fs::create_dir_all(&cwd).expect("project dir");
        fs::create_dir_all(&bin).expect("bin dir");
        let server = bin.join("rust-analyzer");
        fs::write(&server, "#!/bin/sh
sleep 5
").expect("write fake hanging server");
        let mut permissions = fs::metadata(&server).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&server, permissions).expect("chmod");

        let mut state = RuntimeLspState::new_with_startup_configs_and_timeout(
            &cwd,
            vec![LspStartupConfig {
                language: "rust".to_string(),
                command: server.to_string_lossy().to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                capabilities: Vec::new(),
                root_path: Some(cwd.to_string_lossy().to_string()),
                explicit: false,
            }],
            std::time::Duration::from_millis(20),
        )
        .expect("lsp state should build")
        .expect("startup config should create an lsp state");

        let servers = state.registry.list_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].language, "rust");
        assert_eq!(servers[0].status, LspServerStatus::Error);
        state.shutdown().expect("shutdown empty lsp state");
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn detects_language_server_from_supplied_path() {
        let root = temp_dir("path");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        let server = bin.join("gopls");
        fs::write(&server, "#!/bin/sh\nexit 0\n").expect("write fake server");
        let mut permissions = fs::metadata(&server).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&server, permissions).expect("chmod");

        assert_eq!(
            detect_language_server_in_path("go", Some(bin.as_os_str())),
            Some("gopls")
        );
        assert_eq!(
            detect_language_server_in_path("unknown-language", Some(bin.as_os_str())),
            None
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn autodetect_defaults_on_and_opts_out_on_falsey() {
        assert!(autodetect_enabled(None), "default on when unset");
        assert!(autodetect_enabled(Some("1")));
        assert!(autodetect_enabled(Some("anything")));
        for off in ["0", "false", "off", "no", " OFF "] {
            assert!(!autodetect_enabled(Some(off)), "{off} should disable");
        }
    }
}
