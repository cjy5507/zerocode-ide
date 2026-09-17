//! zo-cli `main.rs` 가 크레이트 루트에 두던 헬퍼들의 새 집.
//!
//! 포팅된 엔진 파일들은 `crate::current_cli_cwd()` 처럼 루트 경로로 이들을
//! 부른다. `lib.rs` 가 같은 이름으로 재수출하므로 원본 파일의 경로는 그대로
//! 컴파일된다. TUI 전용이던 항목은 여기서 **평면 CLI 의미로 고정**된다 —
//! `tui_active()` 는 항상 `false` (이 바이너리는 터미널을 장악하지 않는다).

use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use api::ToolDefinition;
use plugins::{PluginManager, PluginManagerConfig};
use runtime::ConfigLoader;
use tools::GlobalToolRegistry;

use crate::cli_args::AllowedToolSet;

pub const DEFAULT_MODEL: &str = api::ANTHROPIC_LATEST_MODEL_ALIAS;
pub const PRIMARY_SESSION_EXTENSION: &str = "jsonl";
pub const LEGACY_SESSION_EXTENSION: &str = "json";
pub const LATEST_SESSION_REFERENCE: &str = "latest";
pub const SESSION_REFERENCE_ALIASES: &[&str] = &[LATEST_SESSION_REFERENCE, "last", "recent"];

/// zo-cli 의 `TUI_ACTIVE` 게이트. 이 프런트엔드는 raw mode·alt screen 을 절대
/// 켜지 않으므로 항상 `false` — 레거시 stdout 직배출 분기(`emit_output &&
/// !tui_active()`)는 `emit_output=false` 로 런타임을 빌드해 막는다.
#[must_use]
pub const fn tui_active() -> bool {
    false
}

/// Per-response output-token cap for `model` — 카탈로그 단일 진실
/// [`api::max_tokens_for_model`] 위임.
#[must_use]
pub fn max_tokens_for_model(model: &str) -> u32 {
    api::max_tokens_for_model(model)
}

/// 시스템 프롬프트에 주입되는 오늘 날짜(로컬).
#[must_use]
pub fn default_prompt_date() -> String {
    core_types::date::current_local_date()
}

/// 셸의 `$PWD` 가 실제 cwd 와 같은 위치를 가리키면 그 철자를 보존한다
/// (심링크 경로 유지) — 아니면 실제 cwd.
pub fn current_cli_cwd() -> io::Result<PathBuf> {
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

/// The tools this session ADVERTISES to the model.
///
/// The deny set has to bite here, not only at execution. `check_disallowed_tools`
/// refuses a turned-off tool when it is called — but the model was still shown
/// its schema and still spent a turn calling it. Measured with `--no-spawn` on
/// an empty workspace: the flag documented as "turn off zo's own
/// Agent/SpawnMultiAgent/Workflow tools" moved the prefix by **2 tokens**, while
/// `tool Agent` (1,168) and `# Delegation and workflow routing` (1,315) both
/// still shipped. A flag that says it turns something off must stop paying for
/// it.
///
/// Narrowing the ALLOW set instead would be wrong: `definitions` treats an
/// explicit allow-list as "advertise exactly this", which un-defers the deferred
/// tool families and would grow the prefix rather than shrink it.
#[must_use]
pub fn filter_tool_specs(
    tool_registry: &GlobalToolRegistry,
    allowed_tools: Option<&AllowedToolSet>,
) -> Vec<ToolDefinition> {
    let mut definitions = tool_registry.definitions(allowed_tools);
    if let Some(disallowed) = crate::runtime_support::disallowed_tool_names() {
        definitions.retain(|definition| !disallowed.contains(&definition.name));
    }
    definitions
}

#[must_use]
pub fn build_plugin_manager(
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

/// Ctrl-C 를 `HookAbortSignal` 로 번역하는 감시 스레드. 턴 드라이버가 턴마다
/// 띄우고 `stop()` 으로 내린다.
pub struct HookAbortMonitor {
    stop_tx: Option<Sender<()>>,
    join_handle: Option<JoinHandle<()>>,
}

impl HookAbortMonitor {
    #[must_use]
    pub fn spawn(abort_signal: runtime::HookAbortSignal) -> Self {
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

    pub fn stop(mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

/// 프롬프트가 실제 코드 변경을 요구하는지(분석/요약 아님) — 파일 경로 언급이
/// 가장 강한 신호, 아니면 명시적 변경 동사. 포팅: zo-cli `main_dispatch.rs`.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn prompt_is_coding_task(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let file_hits = count_path_mentions(prompt) as f64;
    file_hits > 0.0 || count_present(&lower, CODING_TASK_MARKERS) > 0
}

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

fn count_present(haystack: &str, needles: &[&str]) -> usize {
    needles
        .iter()
        .filter(|needle| haystack.contains(**needle))
        .count()
}

fn count_path_mentions(prompt: &str) -> usize {
    prompt
        .split_whitespace()
        .filter(|token| {
            let trimmed = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '/');
            trimmed.contains('/') && !trimmed.starts_with("http")
        })
        .count()
}

/// 테스트 전용 잠금 — zo-cli 와 같은 계약: env 락을 cwd 락보다 먼저 잡는다.
/// 테스트가 환경 변수를 잠깐 바꿀 때 쓰는 가드 — **복원은 Drop 이 한다**.
///
/// 같은 20줄이 네 모듈에 복제돼 있었고, 그 위에 가드 없이 손으로 되돌리는
/// 자리가 더 있었다. 손 복원은 assert 가 터지면 도달하지 못해 같은 프로세스의
/// 형제 테스트를 오염시킨다(단위 테스트는 한 바이너리에서 병렬로 돈다).
/// `var_os` 로 다루므로 non-UTF8 값도 원래대로 돌아온다.
#[cfg(test)]
pub(crate) struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl EnvVarGuard {
    /// `value` 가 `None` 이면 지운다. 반환한 가드가 살아 있는 동안만 유효하다.
    pub(crate) fn set(key: &'static str, value: Option<&str>) -> Self {
        let previous = std::env::var_os(key);
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
        Self { key, previous }
    }
}

#[cfg(test)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

/// 테스트용 임시 디렉터리 — 만들어 준 채로 돌려준다.
///
/// 일곱 벌이 있었고 규약이 갈렸다: 만들어 주는 판과 안 만들어 주는 판, 유일성을
/// 나노초로 잡는 판(병렬 테스트에서 같은 라벨끼리 충돌한다)과 pid+카운터로 잡는
/// 판. 여기 남긴 것은 그중 제일 튼튼한 pid+카운터 판이다.
#[cfg(test)]
pub(crate) fn temp_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "zo-{label}-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[cfg(test)]
pub(crate) fn test_cwd_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// A synthetic session store pinned as this process's `ZO_SESSION_ROOT` for
/// one test (t-2947): takes the cwd lock, isolates the config home, writes the
/// corpus, and puts the environment back when dropped.
#[cfg(test)]
pub(crate) struct SessionRootPin {
    pub(crate) root: PathBuf,
    pub(crate) sessions_dir: PathBuf,
    /// The workspace the newest sessions record in their `.cwd` sidecar.
    pub(crate) cwd: PathBuf,
    pub(crate) manifest: Vec<session_corpus::GeneratedSession>,
    prior_root: Option<std::ffi::OsString>,
    prior_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl SessionRootPin {
    pub(crate) fn new(tag: &str, spec: &session_corpus::CorpusSpec) -> Self {
        let guard = test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "zo-session-root-{tag}-{}-{unique}",
            std::process::id()
        ));
        let sessions_dir = root.join("sessions");
        let home = root.join("config-home");
        let cwd = root.join("workspace");
        std::fs::create_dir_all(&home).expect("config home");
        std::fs::create_dir_all(&cwd).expect("workspace");
        let manifest = session_corpus::generate(&sessions_dir, spec).expect("synthetic corpus");
        // Sidecars for the head window only: the sidecar writer fsyncs, and
        // the picker reads a sidecar for its listed rows alone.
        for session in manifest.iter().take(spec.head_window) {
            crate::resume::write_session_cwd_if_missing(&session.path, &cwd).expect("cwd sidecar");
        }
        let prior_root = env::var_os("ZO_SESSION_ROOT");
        let prior_home = env::var_os("ZO_CONFIG_HOME");
        env::set_var("ZO_CONFIG_HOME", &home);
        env::set_var("ZO_SESSION_ROOT", &root);
        Self {
            root,
            sessions_dir,
            cwd,
            manifest,
            prior_root,
            prior_home,
            _guard: guard,
        }
    }
}

#[cfg(test)]
impl Drop for SessionRootPin {
    fn drop(&mut self) {
        match self.prior_root.take() {
            Some(value) => env::set_var("ZO_SESSION_ROOT", value),
            None => env::remove_var("ZO_SESSION_ROOT"),
        }
        match self.prior_home.take() {
            Some(value) => env::set_var("ZO_CONFIG_HOME", value),
            None => env::remove_var("ZO_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
pub(crate) fn test_env_mutex() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| {
        std::env::set_var("ZO_DISABLE_KEYCHAIN", "1");
        let home = std::env::temp_dir().join(format!("zo-ide-test-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        std::env::set_var("ZO_CONFIG_HOME", &home);
        std::env::set_var("HOME", &home);
        // `CODEX_HOME` 은 **비어 있지 않아도** 덮는다. 테스트는 codex 패인에서
        // 돌 수 있고(창이 그 변수를 심는다), 거기서 `save_openai_oauth` 의
        // `$CODEX_HOME/auth.json` 폴백은 사람의 **진짜 codex 로그인**에 쓴다.
        // 2026-08-27 에 실제로 그 일이 났다: 픽스처 토큰(`access_token:
        // "oauth-token"`, `account_id: "acct"`)이 미러 auth.json 을 덮어 codex
        // 패인이 전부 "access token could not be refreshed" 로 죽었다.
        std::env::set_var("CODEX_HOME", &home);
        std::sync::Mutex::new(())
    })
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    let guard = test_env_mutex()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // 창은 자기가 연 모든 판에 세컨드 브레인 볼트를 심는다(`CODEX_HOME` 과 같은
    // 부류의 함정이다). 그대로 두면 판 안에서 도는 시험이 사람의 **진짜** 볼트를
    // 색인해 픽스처에 없는 회수 결과를 보게 되므로, 락을 잡을 때마다 지운다 —
    // 세컨드 브레인 시험은 이 락 아래에서 스스로 다시 심는다.
    std::env::remove_var(runtime::second_brain::VAULT_ENV);
    guard
}

#[cfg(test)]
mod tests {
    use super::{prompt_is_coding_task, HookAbortMonitor};
    use runtime::HookAbortSignal;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn coding_task_predicate_prefers_paths_then_verbs() {
        assert!(prompt_is_coding_task("fix src/lib.rs please"));
        assert!(prompt_is_coding_task("Refactor the parser"));
        assert!(!prompt_is_coding_task("Summarize what this repo does"));
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

#[cfg(test)]
mod advertisement_tests {
    use super::filter_tool_specs;
    use crate::runtime_support::{install_cli_runtime_overrides, CliRuntimeOverrides};

    /// A tool the session turned off must not be ADVERTISED, not merely refused.
    ///
    /// `check_disallowed_tools` is an execution-time gate: it denies the call
    /// after the model has already read the schema and spent a turn making it.
    /// Measured before this: `--no-spawn` — documented as "turn off zo's own
    /// Agent/SpawnMultiAgent/Workflow tools" — moved the fixed prefix by **2
    /// tokens**, because `tool Agent` (1,168) and `# Delegation and workflow
    /// routing` (1,315) both still shipped. After: 18,184 → 15,011 tokens on an
    /// empty workspace, below both Claude Code (16,204) and codex (16,364) on
    /// the same protocol.
    ///
    /// The prompt half is pinned by `runtime::prompt::tests`'
    /// `the_delegation_rubric_is_on_by_default_and_off_without_the_spawn_family`.
    #[test]
    fn a_disallowed_tool_leaves_the_advertised_set() {
        let registry = tools::GlobalToolRegistry::builtin();

        // The override is process-global (it is a launch flag), so restore it.
        let restore = CliRuntimeOverrides::default();
        install_cli_runtime_overrides(restore.clone());
        let before: Vec<String> = filter_tool_specs(&registry, None)
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        // Sample a tool that is actually on the wire. `Agent` stood here
        // until it was deferred; the property is about the deny set reaching
        // the advertisement, not about which tool is sampled.
        assert!(
            before.iter().any(|name| name == "TodoWrite"),
            "with nothing denied an ordinary tool is advertised: {before:?}"
        );

        install_cli_runtime_overrides(CliRuntimeOverrides {
            disallowed_tools: Some(["TodoWrite".to_string()].into_iter().collect()),
            ..CliRuntimeOverrides::default()
        });
        let after: Vec<String> = filter_tool_specs(&registry, None)
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        install_cli_runtime_overrides(restore);

        assert!(
            !after.iter().any(|name| name == "TodoWrite"),
            "a denied tool must not be advertised: {after:?}"
        );
        assert_eq!(
            after.len() + 1,
            before.len(),
            "exactly the denied one leaves, nothing else"
        );
    }
}
