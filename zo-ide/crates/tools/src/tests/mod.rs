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

/// Shared test context — cheap to create (registries are Arc-backed).
fn test_ctx() -> &'static ToolContext {
    static CTX: OnceLock<ToolContext> = OnceLock::new();
    CTX.get_or_init(ToolContext::new)
}

/// Convenience wrapper matching the old two-arg signature used across tests.
fn run_tool(name: &str, input: &serde_json::Value) -> Result<String, ToolError> {
    execute_tool(test_ctx(), name, input)
}

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
        Mutex::new(())
    })
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

fn run_tool_in_cwd(name: &str, input: &serde_json::Value, cwd: &Path) -> Result<String, ToolError> {
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
mod files_web;
mod foreground_boundary;
mod permissions_misc;
mod registry;
mod shell;

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
                        stream
                            .write_all(response.to_bytes().as_slice())
                            .expect("write response");
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
