//! Real SQLite authority + generated product CLI + production Python functions.
//! The HTTP listener is test transport. All capabilities and the HOME belong to this fixture.
//! This does not launch the installed window, use its accounts, or validate a release gate.
use super::*;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use zerocode_core::agent_teams::{TEAM_ID_VAR, TEAM_PANE_VAR, TEAM_TOKEN_VAR, shim_script};

const HOOK: &str = "test-only-hook-capability";
const CHILD: &str = "runtime_actor::tests::gate_reuse::receipt_gate_child";
const HTTP_IO_BUDGET: Duration = Duration::from_secs(10);

#[test]
fn receipt_gate_child() {
    // This real libtest process also compiles on the pre-API source for a functional RED.
    let kind: zerocode_core::orchestration::MessageKind = "land_check".parse().unwrap();
    assert!(kind.is_the_ledgers_own());
    if !Path::new("fixture-node.cjs").is_file() {
        return;
    }
    assert_no_inherited_gate_authority();
    for reporter in ["tap", "spec"] {
        let output = Command::new("node")
            .args([
                "--test",
                &format!("--test-reporter={reporter}"),
                "fixture-node.cjs",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "actual node fixture failed");
        println!("\n{}", String::from_utf8_lossy(&output.stdout));
        assert!(output.stderr.is_empty(), "unexpected node stderr");
    }
    let ui = Command::new("node").arg("fixture_ui.mjs").output().unwrap();
    assert!(ui.status.success(), "actual UI report fixture failed");
    println!("{}", String::from_utf8_lossy(&ui.stdout));
    assert!(ui.stderr.is_empty());
    let output = Command::new("python3")
        .args([
            "-I",
            "-S",
            "-B",
            "-m",
            "unittest",
            "discover",
            "-s",
            ".",
            "-p",
            "fixture_unittest.py",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "actual unittest fixture failed");
    println!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn receipt_gate_child_cannot_forge_fixture_completion() {
    let Ok(bytes) = std::fs::read("probe.json") else {
        return;
    };
    let probe: Value = serde_json::from_slice(&bytes).unwrap();
    if std::env::var_os(TEAM_TOKEN_VAR).is_none() {
        assert_no_inherited_gate_authority();
    }
    println!("fixture probe began");
    let task = probe["task"].as_str().unwrap();
    let call = |verb: &str, value: &Value, extra: &[&str]| -> Option<Value> {
        let output = Command::new(probe["cli"].as_str().unwrap())
            .args([
                verb,
                "--task",
                task,
                "--attempt",
                "none",
                "--body",
                &value.to_string(),
                "--retry-request",
                &format!(
                    "fixture-child-{verb}-{}",
                    extra.first().copied().unwrap_or("finish")
                ),
            ])
            .args(extra)
            .output()
            .unwrap();
        if output.status.success() {
            Some(serde_json::from_slice(&output.stdout).unwrap())
        } else {
            let reason = String::from_utf8_lossy(&output.stderr);
            if reason.contains("out_of_scope:") {
                assert!(std::env::var_os(TEAM_TOKEN_VAR).is_some());
                println!("fixture probe denied missing scoped job context");
                return None;
            }
            assert!(
                reason.contains("this shell is not part of an agent team"),
                "probe failed without an authority refusal: {reason}"
            );
            println!("fixture probe denied missing authority");
            None
        }
    };
    let inputs = &probe["inputs"];
    let pin = json!({"version":1,"policy":inputs["policy"],"producer":inputs["producer"]});
    let Some(_) = call("gate-policy-pin", &pin, &["--gate", "gate-root"]) else {
        println!("fixture child has no ledger authority");
        return;
    };
    let Some(registered) = call("gate-register", inputs, &[]) else {
        println!("fixture child has no ledger authority");
        return;
    };
    let id = registered["registration"].as_str().unwrap();
    let lease = registered["lease"].as_str().unwrap();
    let Some(started) = call("gate-record", inputs, &["--start", id, "--lease", lease]) else {
        return;
    };
    let metadata = json!({"version":1,"registration":id,"policyPin":registered["policyPin"],"lease":lease,"execution":started["execution"],
        "attempt":"none","inputs":inputs,"startedMs":started["startedMs"],"endedMs":started["startedMs"],
        "durationMs":0,"rc":0,"outcome":"passed","tests":1,"logSha256":"d".repeat(64),
        "logBytes":32,"inputsUnchanged":true,"executed":true});
    let Some(completed) = call("gate-record", &metadata, &[]) else {
        return;
    };
    let request = json!({"version":1,"registration":id,"lease":lease,"inputs":inputs,
        "metadataSha256":completed["metadataSha256"],"logSha256":metadata["logSha256"],"logBytes":32,"tests":1});
    let hit = call("gate-reuse", &request, &[]);
    println!(
        "fixture forged completion: {}",
        json!({"original":completed["original"],"reuse":hit})
    );
}

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn native_bootstrap_binary() -> PathBuf {
    let path = std::env::var_os("HERDR_FIXTURE_BOOTSTRAP")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .expect("the current workspace test executable")
                .parent()
                .and_then(Path::parent)
                .expect("the Cargo target profile directory")
                .join(format!("zerocode{}", std::env::consts::EXE_SUFFIX))
        });
    assert!(
        path.is_file(),
        "the real native bootstrap must be built by the root workspace test gate: {}",
        path.display()
    );
    path
}

fn assert_no_inherited_gate_authority() {
    for key in [
        TEAM_TOKEN_VAR,
        "ZEROCODE_AGENT_TEAM_TOKEN_FILE",
        "ZEROCODE_HOOK_TOKEN",
        "ZEROCODE_GATE_JOB_TOKEN",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "gate child inherited authority variable {key}"
        );
    }
    for fd in 3..256 {
        let mut kind = 0_i32;
        let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
        // SAFETY: getsockopt writes to the valid fixed-size kind buffer.
        let socket = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut i32).cast(),
                &mut length,
            )
        };
        assert_ne!(socket, 0, "gate child inherited socket descriptor {fd}");
    }
    println!("fixture child has no authority environment or socket descriptor");
}

struct CliFixture {
    _server: FixtureHttpServer,
    actor: Arc<RuntimeActor>,
    old_api: Arc<AtomicBool>,
    gate_open_attempts: Arc<AtomicUsize>,
    panes: TableFixture,
    port: u16,
    root: PathBuf,
    script: PathBuf,
    input: PathBuf,
    child: PathBuf,
    task: String,
    run: String,
    policy: std::sync::Mutex<Option<Value>>,
    output: std::sync::Mutex<PathBuf>,
}

struct FixtureHttpServer {
    stop: Arc<AtomicBool>,
    accepted: Arc<AtomicUsize>,
    serving: Option<thread::JoinHandle<()>>,
}

type FixtureHeaders = std::collections::HashMap<String, String>;

#[derive(Debug)]
enum FixtureHttpError {
    Stopped,
    Io(io::Error),
}

impl From<io::Error> for FixtureHttpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for FixtureHttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => formatter.write_str("fixture HTTP shutdown requested"),
            Self::Io(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl FixtureHttpError {
    fn context(self, context: String) -> Self {
        match self {
            Self::Stopped => Self::Stopped,
            Self::Io(error) => {
                Self::Io(io::Error::new(error.kind(), format!("{context}: {error}")))
            }
        }
    }

    fn kind(&self) -> io::ErrorKind {
        match self {
            Self::Stopped => io::ErrorKind::ConnectionAborted,
            Self::Io(error) => error.kind(),
        }
    }
}

fn bounded_socket_io(
    stream: &mut TcpStream,
    stop: &AtomicBool,
    deadline: Instant,
    mut operation: impl FnMut(&mut TcpStream) -> io::Result<usize>,
) -> Result<usize, FixtureHttpError> {
    let mut last_wait = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(FixtureHttpError::Stopped);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("fixture HTTP deadline expired; last I/O error: {last_wait:?}"),
            )
            .into());
        }
        let interval = remaining.min(Duration::from_millis(50));
        stream.set_read_timeout(Some(interval))?;
        stream.set_write_timeout(Some(interval))?;
        match operation(stream) {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) =>
            {
                last_wait = Some(error);
            }
            Ok(count) => return Ok(count),
            Err(error) => return Err(error.into()),
        }
    }
}

fn http_request(
    stream: &mut TcpStream,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(FixtureHeaders, Vec<u8>, String), FixtureHttpError> {
    stream.set_nonblocking(false)?;
    let mut bytes = Vec::new();
    let split = loop {
        let mut block = [0_u8; 4096];
        let count = bounded_socket_io(stream, stop, deadline, |stream| stream.read(&mut block))
            .map_err(|error| {
                error.context(format!(
                    "fixture request headers after {} bytes",
                    bytes.len()
                ))
            })?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("incomplete fixture request headers: {} bytes", bytes.len()),
            )
            .into());
        }
        if bytes.len() + count > 128 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fixture request headers exceed 128 KiB",
            )
            .into());
        }
        bytes.extend_from_slice(&block[..count]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers: FixtureHeaders = std::str::from_utf8(&bytes[..split])
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().into()))
        .collect();
    let length = headers["content-length"].parse::<usize>().unwrap();
    assert!(length <= 64 * 1024);
    while bytes.len() < split + length {
        let mut block = [0_u8; 4096];
        let remaining = (split + length - bytes.len()).min(block.len());
        let count = bounded_socket_io(stream, stop, deadline, |stream| {
            stream.read(&mut block[..remaining])
        })
        .map_err(|error| {
            error.context(format!(
                "fixture request body after {} of {length} bytes",
                bytes.len() - split
            ))
        })?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "incomplete fixture request body: {} of {length} bytes",
                    bytes.len() - split
                ),
            )
            .into());
        }
        bytes.extend_from_slice(&block[..count]);
    }
    let path = std::str::from_utf8(&bytes[..split])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    Ok((headers, bytes[split..split + length].to_vec(), path))
}

fn http_response(
    stream: &mut TcpStream,
    bytes: &[u8],
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(), FixtureHttpError> {
    let mut written = 0;
    while written < bytes.len() {
        let count = bounded_socket_io(stream, stop, deadline, |stream| {
            stream.write(&bytes[written..])
        })
        .map_err(|error| {
            error.context(format!(
                "fixture response after {written} of {} bytes",
                bytes.len()
            ))
        })?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!(
                    "incomplete fixture response: {written} of {} bytes",
                    bytes.len()
                ),
            )
            .into());
        }
        written += count;
    }
    Ok(())
}

impl FixtureHttpServer {
    fn start(
        listener: TcpListener,
        mut handle: impl FnMut(FixtureHeaders, Vec<u8>, String) -> (u16, String) + Send + 'static,
    ) -> Self {
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let accepted = Arc::new(AtomicUsize::new(0));
        let stopping = Arc::clone(&stop);
        let accepting = Arc::clone(&accepted);
        let serving = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(held) => held,
                    Err(why) if why.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(why) => panic!("fixture transport: {why}"),
                };
                accepting.fetch_add(1, Ordering::SeqCst);
                let deadline = Instant::now() + HTTP_IO_BUDGET;
                let (headers, bytes, path) = match http_request(&mut stream, &stopping, deadline) {
                    Ok(request) => request,
                    Err(FixtureHttpError::Stopped) => return,
                    Err(error) => panic!("fixture transport request: {error}"),
                };
                let (status, body) = handle(headers, bytes, path);
                let response = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                match http_response(&mut stream, response.as_bytes(), &stopping, deadline) {
                    Ok(()) => {}
                    Err(FixtureHttpError::Stopped) => return,
                    Err(error) => panic!("fixture transport response: {error}"),
                }
            }
        });
        Self {
            stop,
            accepted,
            serving: Some(serving),
        }
    }
}

impl CliFixture {
    fn new(authority: &Fixture) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let root = authority._root.path().join("gate-cli");
        for part in ["home", "bin", "input", "deps", "toolchain"] {
            std::fs::create_dir_all(root.join(part)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let door = root.join("bin/zerocode-orc");
        std::fs::write(
            &door,
            shim_script("ZEROCODE_HOOK_PORT", "ZEROCODE_HOOK_TOKEN", "test-only-orc"),
        )
        .unwrap();
        std::fs::set_permissions(&door, std::fs::Permissions::from_mode(0o700)).unwrap();
        let child = root.join("toolchain/gate-child");
        std::fs::copy(std::env::current_exe().unwrap(), &child).unwrap();
        std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            root.join("deps/closed-input"),
            b"fixture has no fetched dependency inputs\n",
        )
        .unwrap();
        std::fs::write(root.join("input/input.txt"), b"public fixture source\n").unwrap();
        std::fs::write(root.join("input/Cargo.lock"), b"version = 4\n").unwrap();
        std::fs::write(root.join("input/fixture-node.cjs"),
            b"const test = require('node:test'); const assert = require('node:assert/strict'); test('fixture', () => assert.equal(1 + 1, 2));\n").unwrap();
        std::fs::write(root.join("input/fixture_unittest.py"),
            b"import unittest\nclass Fixture(unittest.TestCase):\n    def test_fixture(self):\n        self.assertEqual(1 + 1, 2)\n").unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/tests/window-runner.mjs"),
            root.join("input/ui-runner.mjs"),
        )
        .unwrap();
        std::fs::write(root.join("input/fixture_ui.mjs"),
            b"import { createRunner } from './ui-runner.mjs'; const runner=createRunner({env:{},argv:[]}); runner.suite('fixture',({ok})=>ok('actual UI runner check',1+1===2)); await runner.run(); process.exitCode=runner.report()?1:0;\n").unwrap();
        std::fs::write(root.join("input/fixture_ui_zero.mjs"),
            b"import { createRunner } from './ui-runner.mjs'; const runner=createRunner({env:{},argv:[]}); await runner.run(); process.exitCode=runner.report()?1:0;\n").unwrap();
        let linked = root.join("input/tools/computer-bench/long-form/bin");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join("zerocode-door"), b"fixture source input\n").unwrap();
        for name in ["zerocode-browser", "zerocode-computer"] {
            std::os::unix::fs::symlink("zerocode-door", linked.join(name)).unwrap();
        }
        let input = root.join("input");
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "dev"],
            vec!["config", "user.email", "dev@example.com"],
            vec!["add", "."],
            vec!["commit", "-qm", "fixture"],
        ] {
            assert!(
                zerocode_core::host::lock_free_git(Command::new("git"))
                    .env_clear()
                    .env(zerocode_core::host::GIT_OPTIONAL_LOCKS, "0")
                    .env("HOME", root.join("home"))
                    .env("PATH", std::env::var("PATH").unwrap())
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .arg("-C")
                    .arg(&input)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let table = TableFixture::holding(a_team());
        let panes = (*table).clone();
        let actor = Arc::new(
            RuntimeActor::start(
                &authority.store,
                "main-ledger",
                RuntimeBoot::Fresh { now_ms: now() },
                4,
                table,
                Box::new(NoLauncher),
            )
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let old_api = Arc::new(AtomicBool::new(false));
        let gate_open_attempts = Arc::new(AtomicUsize::new(0));
        let server = {
            let actor = Arc::clone(&actor);
            let old_api = Arc::clone(&old_api);
            let gate_open_attempts = Arc::clone(&gate_open_attempts);
            FixtureHttpServer::start(listener, move |headers, bytes, path| {
                if path == "/gate-sessions/open" {
                    gate_open_attempts.fetch_add(1, Ordering::SeqCst);
                }
                let argv: Vec<String> = if path == "/gate-sessions/open" {
                    Vec::new()
                } else {
                    std::str::from_utf8(&bytes)
                        .unwrap()
                        .split('\u{1f}')
                        .filter(|word| !word.is_empty())
                        .map(str::to_string)
                        .collect()
                };
                if headers.get("x-zerocode-hook-token").map(String::as_str) != Some(HOOK) {
                    (403, "test capability refused".to_string())
                } else if old_api.load(Ordering::SeqCst)
                    && (path == "/gate-sessions/open"
                        || argv.first().is_some_and(|verb| verb.starts_with("gate-")))
                {
                    (400, "unknown verb in old fixture API".to_string())
                } else if path == "/gate-sessions/open" {
                    match super::super::gate_session::open_native(
                        &actor,
                        &headers["x-zerocode-team-id"],
                        &headers["x-zerocode-team-pane"],
                        &headers["x-zerocode-team-token"],
                        Some(format!("actor-v1:{}", "a".repeat(64))),
                        std::str::from_utf8(&bytes).unwrap(),
                    ) {
                        Ok(value) => (200, value.to_string()),
                        Err(code) => (400, format!("{code}: fixture gate scope refused")),
                    }
                } else {
                    let verb = argv.first().cloned().unwrap_or_default();
                    let command = PlanCommand::checked(
                        argv,
                        headers["x-zerocode-team-id"].clone(),
                        headers["x-zerocode-team-pane"].clone(),
                        headers["x-zerocode-team-token"].clone(),
                        Some(format!("actor-v1:{}", "a".repeat(64))),
                        now(),
                    )
                    .unwrap();
                    match actor.plan(command) {
                        Ok((planned, _)) if planned.reply.exit_code == 0 => {
                            (200, planned.reply.stdout)
                        }
                        Ok((planned, _)) => {
                            println!(
                                "{}",
                                json!({"fixtureNativeRefusal":verb,"reason":planned.reply.stderr})
                            );
                            (400, planned.reply.stderr)
                        }
                        Err(_) => (
                            403,
                            "native authority refused the fixture request".to_string(),
                        ),
                    }
                }
            })
        };
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/release/gate_receipt.py");
        let mut held = Self {
            _server: server,
            actor,
            old_api,
            gate_open_attempts,
            panes,
            port,
            root,
            script,
            input,
            child,
            task: String::new(),
            run: String::new(),
            policy: std::sync::Mutex::new(None),
            output: std::sync::Mutex::new(authority._root.path().join("gate-cli/out")),
        };
        held.run = held.cli(&[
            "run-create",
            "--name",
            "isolated-gates",
            "--retry-request",
            "fixture-run",
        ])["runId"]
            .as_str()
            .unwrap()
            .into();
        held.task = held.cli(&[
            "task-create",
            "--spec",
            "coordinator-direct",
            "--retry-request",
            "fixture-task",
        ])["taskId"]
            .as_str()
            .unwrap()
            .into();
        held
    }

    fn environment(&self, command: &mut Command) {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        command
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("PATH", path)
            .env("ZEROCODE_HOOK_PORT", self.port.to_string())
            .env("ZEROCODE_HOOK_TOKEN", HOOK)
            .env(TEAM_ID_VAR, "team-1")
            .env(TEAM_PANE_VAR, "%1")
            .env(TEAM_TOKEN_VAR, capability_of("%1"));
        if std::env::var("ZEROCODE_GATE_TIMING").as_deref() == Ok("1") {
            command.env("ZEROCODE_GATE_TIMING", "1");
        }
    }

    fn raw_cli(&self, args: &[&str]) -> Output {
        let mut command = Command::new(self.root.join("bin/zerocode-orc"));
        command.args(args);
        self.environment(&mut command);
        command.output().unwrap()
    }

    fn cli(&self, args: &[&str]) -> Value {
        let output = self.raw_cli(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn install_probe(&self) {
        let digest = "b".repeat(64);
        let inputs = json!({"version":1,"gate":"gate-root","head":"a".repeat(40),"tree":"c".repeat(40),
            "source":digest,"deps":digest,"lock":digest,"script":digest,"recipe":digest,
            "toolchain":digest,"platform":digest,"profile":digest,"env":digest,"scope":"deterministic",
            "policy":digest,"producer":digest,"handedInHead":null,"sourceAncestor":false});
        std::fs::write(
            self.input.join("probe.json"),
            json!({"task":self.task,
            "cli":self.root.join("bin/zerocode-orc"),"inputs":inputs})
            .to_string(),
        )
        .unwrap();
        for args in [
            vec!["add", "."],
            vec!["commit", "-qm", "fixture authority probe"],
        ] {
            assert!(
                zerocode_core::host::lock_free_git(Command::new("git"))
                    .env_clear()
                    .env(zerocode_core::host::GIT_OPTIONAL_LOCKS, "0")
                    .env("HOME", self.root.join("home"))
                    .env("PATH", std::env::var("PATH").unwrap())
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .arg("-C")
                    .arg(&self.input)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }

    fn production(&self, action: &str, clean: bool) -> Output {
        self.production_test(action, clean, CHILD)
    }

    fn production_test(&self, action: &str, clean: bool, child_test: &str) -> Output {
        self.production_fixture(action, clean, child_test, false, false)
    }

    fn production_fixture(
        &self,
        action: &str,
        clean: bool,
        child_test: &str,
        cargo: bool,
        ui_zero: bool,
    ) -> Output {
        if action == "run" && self.policy.lock().unwrap().is_none() {
            let mut prepare = Command::new("python3");
            prepare
                .args(["-I", "-S", "-B"])
                .arg(self.script.parent().unwrap().join("tests/gate_fixture.py"))
                .arg("--root")
                .arg(&self.root)
                .arg("--repo")
                .arg(&self.input)
                .arg("--producer")
                .arg(&self.script)
                .arg("--child")
                .arg(&self.child)
                .arg("--test")
                .arg(child_test)
                .arg("--bootstrap");
            if cargo {
                prepare.arg("--cargo");
            }
            if ui_zero {
                prepare.arg("--ui-zero");
            }
            self.environment(&mut prepare);
            if cargo {
                let rustup_home = std::env::var_os("RUSTUP_HOME").or_else(|| {
                    std::env::var_os("HOME")
                        .map(|home| PathBuf::from(home).join(".rustup").into_os_string())
                });
                if let Some(home) = rustup_home {
                    prepare.env("RUSTUP_HOME", home);
                }
                prepare.env("RUSTUP_AUTO_INSTALL", "0");
                prepare.env("CARGO_NET_OFFLINE", "true");
                for key in [
                    "RUSTUP_TOOLCHAIN",
                    "DEVELOPER_DIR",
                    "SDKROOT",
                    "HERDR_FIXTURE_TOOLCHAIN",
                    "HERDR_FIXTURE_DEVELOPER_DIR",
                    "HERDR_FIXTURE_SDKROOT",
                    "HERDR_FIXTURE_LINKER",
                ] {
                    if let Some(value) = std::env::var_os(key) {
                        prepare.env(key, value);
                    }
                }
            }
            let output = prepare.output().unwrap();
            assert!(
                output.status.success(),
                "fixture trust setup: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            *self.policy.lock().unwrap() = Some(serde_json::from_slice(&output.stdout).unwrap());
            let policy = self.policy.lock().unwrap().clone().unwrap();
            let pin = self.cli(&["gate-policy-pin", "--gate", "gate-root", "--body",
                &json!({"version":1,"policy":policy["policySha256"],"producer":policy["producerSha256"]}).to_string(),
                "--retry-request", "fixture-reviewed-policy-pin"]);
            self.policy.lock().unwrap().as_mut().unwrap()["pin"] = pin["pin"].clone();
        }
        let policy = self.policy.lock().unwrap().clone().unwrap();
        let output_root = self.output.lock().unwrap().clone();
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(self.root.join("trusted-policy.json")).unwrap())
                .unwrap();
        let registration = if action == "verify" {
            let metadata: Value =
                serde_json::from_slice(&std::fs::read(output_root.join("metadata.json")).unwrap())
                    .unwrap();
            metadata["registration"].clone()
        } else {
            Value::Null
        };
        let intent = json!({"version":1,"run":self.run,"task":self.task,"attempt":"none","gate":"gate-root",
            "pin":policy["pin"],"head":manifest["gates"]["gate-root"]["target"]["head"],"mode":action,"registration":registration});
        let mut command = Command::new(native_bootstrap_binary());
        command
            .arg("gate-bootstrap")
            .args(
                policy["bootstrap"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|word| word.as_str().unwrap()),
            )
            .arg("--gate-job")
            .arg(intent.to_string())
            .arg("--")
            .arg("--policy")
            .arg(self.root.join("trusted-policy.json"))
            .arg("--policy-sha256")
            .arg(policy["policySha256"].as_str().unwrap())
            .arg("--repo")
            .arg(&self.input)
            .arg("--")
            .arg(action)
            .arg("--repo")
            .arg(&self.input)
            .args([
                "--gate",
                "gate-root",
                "--task",
                &self.task,
                "--attempt",
                "none",
                "--scope",
                "deterministic",
                "--closure",
                "declared",
                "--profile",
                "test-debug",
            ]);
        command
            .arg("--timeout")
            .arg(manifest["gates"]["gate-root"]["timeout"].to_string());
        if clean {
            command.arg("--clean-env");
        }
        if action == "run" {
            command.arg("--output").arg(&output_root);
        } else {
            command
                .arg("--metadata")
                .arg(output_root.join("metadata.json"))
                .arg("--log")
                .arg(output_root.join("gate.log"));
        }
        command.arg("--").args(
            policy["command"]
                .as_array()
                .unwrap()
                .iter()
                .map(|word| word.as_str().unwrap()),
        );
        self.environment(&mut command);
        let output = command.output().unwrap();
        if std::env::var("ZEROCODE_GATE_TIMING").as_deref() == Ok("1") {
            println!(
                "{}",
                json!({"fixtureNative":serde_json::from_slice::<Value>(&output.stdout).unwrap_or_default(),"nativeStderr":String::from_utf8_lossy(&output.stderr)})
            );
        }
        output
    }

    fn job_intent(
        &self,
        mode: &str,
        registration: Option<&str>,
    ) -> zerocode_core::orchestration::gate_reuse::JobIntent {
        let policy = self.policy.lock().unwrap().clone().unwrap();
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(self.root.join("trusted-policy.json")).unwrap())
                .unwrap();
        serde_json::from_value(json!({"version":1,"run":self.run,"task":self.task,"attempt":"none","gate":"gate-root",
            "pin":policy["pin"],"head":manifest["gates"]["gate-root"]["target"]["head"],"mode":mode,"registration":registration})).unwrap()
    }

    fn scoped_connection(&self, mode: &str, registration: Option<&str>) -> (String, TcpStream) {
        let intent = self.job_intent(mode, registration);
        let owner = PlanCommand::checked(
            vec!["native-gate-open".into()],
            "team-1",
            "%1",
            capability_of("%1"),
            Some(format!("actor-v1:{}", "a".repeat(64))),
            now(),
        )
        .unwrap();
        let reply = self
            .actor
            .open_gate_session(
                owner,
                intent,
                std::fs::read_to_string(self.root.join("trusted-policy.json")).unwrap(),
                self.bootstrap_identity(),
            )
            .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        let job = reply["value"]["job"].as_str().unwrap().to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        super::super::gate_session::serve(
            Arc::downgrade(&self.actor),
            listener,
            job.clone(),
            reply["value"]["expiresMs"].as_i64().unwrap(),
        );
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        super::super::gate_session::write_frame(
            &mut stream,
            &json!({"capability":capability_of("%1")}),
        )
        .unwrap();
        let bound = super::super::gate_session::read_frame(&mut stream)
            .unwrap()
            .unwrap();
        assert_eq!(bound["ok"], true, "{bound}");
        (job, stream)
    }

    fn pin_current_policy(&self, retry: &str) {
        let policy = self.policy.lock().unwrap().clone().unwrap();
        let reply = self.cli(&["gate-policy-pin", "--gate", "gate-root", "--body",
            &json!({"version":1,"policy":policy["policySha256"],"producer":policy["producerSha256"]}).to_string(),
            "--retry-request", retry]);
        self.policy.lock().unwrap().as_mut().unwrap()["pin"] = reply["pin"].clone();
    }

    fn bootstrap_identity(&self) -> zerocode_core::orchestration::gate_reuse::BootstrapIdentity {
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(self.root.join("trusted-policy.json")).unwrap())
                .unwrap();
        zerocode_core::orchestration::gate_reuse::BootstrapIdentity::from_manifest(&manifest)
            .unwrap()
    }

    fn retained_registration_command(&self, job: &str, inputs: &Value, retry: &str) -> PlanCommand {
        use zerocode_core::orchestration::gate_reuse as gate;
        let ledger = Ledger::rebuild(self.actor.view().unwrap().projection().clone()).unwrap();
        let scope = gate::authorize_job(
            &ledger,
            &a_team(),
            "%1",
            &format!("actor-v1:{}", "a".repeat(64)),
            self.job_intent("run", None),
            &std::fs::read_to_string(self.root.join("trusted-policy.json")).unwrap(),
            self.bootstrap_identity(),
            now(),
        )
        .unwrap();
        let retry = format!(
            "gate-job-{}",
            zerocode_core::user_preferences::content_key(format!("{job}:{retry}").as_bytes())
        );
        let mut command = PlanCommand::checked(
            vec![
                "gate-register".into(),
                "--run".into(),
                self.run.clone(),
                "--task".into(),
                self.task.clone(),
                "--attempt".into(),
                "none".into(),
                "--body".into(),
                inputs.to_string(),
                "--retry-request".into(),
                retry,
            ],
            "team-1",
            "%1",
            capability_of("%1"),
            Some(format!("actor-v1:{}", "a".repeat(64))),
            now(),
        )
        .unwrap();
        command.gate_context = Some(gate::JobContext::from_session(job, &scope).unwrap());
        command.gate_scope = Some(Box::new(scope));
        command
    }

    fn takeover(&self, team: &str, from: &str, actor_byte: char, retry: &str) -> Value {
        self.panes
            .teams
            .lock()
            .unwrap()
            .entry(team.into())
            .or_insert_with(|| Team::new(team, "fixture-takeover-team", 8));
        self.panes
            .tokens
            .lock()
            .unwrap()
            .insert((team.into(), "%1".into()), capability_of("%1"));
        let command = PlanCommand::checked(
            vec![
                "run-takeover".into(),
                "--run".into(),
                self.run.clone(),
                "--from".into(),
                from.into(),
                "--reason".into(),
                "fixture coordinator generation change".into(),
                "--retry-request".into(),
                retry.into(),
            ],
            team,
            "%1",
            capability_of("%1"),
            Some(format!("actor-v1:{}", actor_byte.to_string().repeat(64))),
            now(),
        )
        .unwrap();
        let (answer, _) = self.actor.plan(command).unwrap();
        assert_eq!(answer.reply.exit_code, 0, "{}", answer.reply.stderr);
        serde_json::from_str(&answer.reply.stdout).unwrap()
    }

    fn miss(&self, clean: bool) -> Value {
        let output = self.production("verify", clean);
        assert!(!output.status.success());
        let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(answer["decision"], "miss", "{answer}");
        assert_eq!(answer["newExecutions"], 0);
        answer
    }
}

#[test]
fn fixture_child_cannot_create_a_completion_or_a_reusable_hit() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    fixture.install_probe();
    let output = fixture.production_test(
        "run",
        false,
        "runtime_actor::tests::gate_reuse::receipt_gate_child_cannot_forge_fixture_completion",
    );
    let view = fixture.actor.view().unwrap();
    let completions = view
        .projection()
        .messages
        .iter()
        .filter(|row| {
            let body: Value = serde_json::from_str(row.payload.as_str()).unwrap_or_default();
            ["registered", "started", "completed"]
                .iter()
                .any(|phase| body["gateEvidenceV1"]["phase"] == *phase)
        })
        .count();
    assert_eq!(
        completions, 3,
        "fixture child created extra registration, start or completion evidence"
    );
    let log = std::fs::read_to_string(fixture.root.join("out/gate.log")).unwrap();
    assert!(
        output.status.success(),
        "{}\n{log}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(log.contains("fixture probe began"), "probe never ran");
    assert!(
        log.contains("fixture probe denied missing authority"),
        "probe never reached an authority refusal"
    );
    assert!(
        !log.contains("fixture forged completion"),
        "fixture child obtained a reusable hit"
    );
}

#[test]
fn the_identical_probe_requires_a_scoped_job_even_with_full_fixture_authority() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    fixture.install_probe();
    let child_name =
        "runtime_actor::tests::gate_reuse::receipt_gate_child_cannot_forge_fixture_completion";
    let mut child = Command::new(&fixture.child);
    child
        .current_dir(&fixture.input)
        .args(["--exact", child_name, "--nocapture"]);
    fixture.environment(&mut child);
    let output = child.output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("fixture probe began"),
        "positive control never ran"
    );
    assert!(
        text.contains("fixture probe denied missing scoped job context"),
        "{text}"
    );
    assert!(!text.contains("fixture forged completion"), "{text}");
    let view = fixture.actor.view().unwrap();
    let phases: Vec<Value> = view
        .projection()
        .messages
        .iter()
        .filter_map(|row| {
            let body: Value = serde_json::from_str(row.payload.as_str()).ok()?;
            body["gateEvidenceV1"].get("phase").cloned()
        })
        .collect();
    assert_eq!(phases, vec![json!("policy_pinned")]);
    let produced = fixture.production_test("run", true, child_name);
    assert!(
        produced.status.success(),
        "{}",
        String::from_utf8_lossy(&produced.stdout)
    );
    let hit = fixture.production_test("verify", true, child_name);
    assert!(
        hit.status.success(),
        "{}",
        String::from_utf8_lossy(&hit.stdout)
    );
    let hit: Value = serde_json::from_slice(&hit.stdout).unwrap();
    assert_eq!(hit["decision"], "hit");
    assert_eq!(hit["newExecutions"], 0);
}

impl Drop for FixtureHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let Some(serving) = self.serving.take() else {
            return;
        };
        let deadline = Instant::now() + HTTP_IO_BUDGET;
        while !serving.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let failure = if serving.is_finished() {
            serving.join().err().map(|payload| {
                let reason = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic payload");
                format!("fixture HTTP server panicked: {reason}")
            })
        } else {
            Some("fixture HTTP server did not stop within 10 seconds".to_string())
        };
        if let Some(failure) = failure {
            if thread::panicking() {
                let _ = writeln!(io::stderr().lock(), "{failure}");
            } else {
                panic!("{failure}");
            }
        }
    }
}

fn fixture_socket_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client =
        TcpStream::connect_timeout(&listener.local_addr().unwrap(), Duration::from_secs(2))
            .unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    (listener.accept().unwrap().0, client)
}

fn wait_for_fixture(ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "fixture condition deadline expired"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn http_transport_completes_fragmented_io_on_an_inherited_nonblocking_socket() {
    let (mut server, mut client) = fixture_socket_pair();
    server.set_nonblocking(true).unwrap();
    let response = vec![b'x'; 16 * 1024];
    let expected = response.clone();
    let receiving = thread::spawn(move || {
        for part in [
            b"POST /fixture HTTP/1.1\r\nContent-Length: 6\r\n".as_slice(),
            b"\r\nabc",
            b"def",
        ] {
            client.write_all(part).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        let mut received = Vec::new();
        client.read_to_end(&mut received).unwrap();
        assert_eq!(received, expected);
    });
    let stop = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(2);
    let (headers, bytes, path) = http_request(&mut server, &stop, deadline).unwrap();
    assert_eq!(headers["content-length"], "6");
    assert_eq!(bytes, b"abcdef");
    assert_eq!(path, "/fixture");
    http_response(&mut server, &response, &stop, deadline).unwrap();
    drop(server);
    receiving.join().unwrap();
}

#[test]
fn http_transport_rejects_an_incomplete_request_at_the_deadline() {
    let (mut server, mut client) = fixture_socket_pair();
    client
        .write_all(b"POST /fixture HTTP/1.1\r\nContent-Length: 4\r\n\r\nab")
        .unwrap();
    let error = http_request(
        &mut server,
        &AtomicBool::new(false),
        Instant::now() + Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("fixture HTTP deadline expired"));
}

#[test]
fn fixture_server_shutdown_interrupts_an_incomplete_read() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureHttpServer::start(listener, |_, _, _| {
        panic!("an incomplete request reached the fixture handler")
    });
    let mut client = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client.write_all(b"P").unwrap();
    wait_for_fixture(|| server.accepted.load(Ordering::SeqCst) == 1);
    drop(server);
}

#[test]
fn fixture_server_shutdown_interrupts_a_pending_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, entered) = std::sync::mpsc::channel();
    let (release, pending) = std::sync::mpsc::channel();
    let server = FixtureHttpServer::start(listener, move |_, _, _| {
        ready.send(()).unwrap();
        pending.recv_timeout(Duration::from_secs(2)).unwrap();
        (200, "fixture response".to_string())
    });
    let mut client = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .write_all(b"POST /fixture HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
        .unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let stopping = Arc::clone(&server.stop);
    let teardown = thread::spawn(move || drop(server));
    wait_for_fixture(|| stopping.load(Ordering::SeqCst));
    release.send(()).unwrap();
    teardown.join().unwrap();
}

#[test]
fn fixture_server_preserves_an_incomplete_request_as_a_test_failure() {
    let failure = std::panic::catch_unwind(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = FixtureHttpServer::start(listener, |_, _, _| {
            panic!("an incomplete request reached the fixture handler")
        });
        let mut client = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.write_all(b"P").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        wait_for_fixture(|| server.serving.as_ref().unwrap().is_finished());
    })
    .unwrap_err();
    assert!(
        failure
            .downcast_ref::<String>()
            .unwrap()
            .contains("incomplete fixture request headers")
    );
}

#[test]
fn fixture_server_does_not_panic_again_while_a_test_unwinds() {
    let failure = std::panic::catch_unwind(|| {
        let _server = FixtureHttpServer {
            stop: Arc::new(AtomicBool::new(false)),
            accepted: Arc::new(AtomicUsize::new(0)),
            serving: Some(thread::spawn(|| {
                panic!("fixture server regression failure")
            })),
        };
        panic!("primary fixture regression failure");
    })
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<&str>(),
        Some(&"primary fixture regression failure")
    );
}

fn reuse_observation(fixture: &CliFixture) -> Value {
    let output = fixture.output.lock().unwrap().clone();
    let bytes = std::fs::read(output.join("metadata.json")).unwrap();
    let metadata: Value = serde_json::from_slice(&bytes).unwrap();
    let log = std::fs::read(output.join("gate.log")).unwrap();
    json!({"version":1,"registration":metadata["registration"],"lease":metadata["lease"],"inputs":metadata["inputs"],
        "metadataSha256":zerocode_core::user_preferences::content_key(&bytes),"logSha256":zerocode_core::user_preferences::content_key(&log),
        "logBytes":log.len(),"tests":metadata["tests"]})
}

fn scope_call(stream: &mut TcpStream, value: Value) -> Value {
    super::super::gate_session::write_frame(stream, &value).unwrap();
    super::super::gate_session::read_frame(stream)
        .unwrap()
        .expect("a complete scoped response")
}

#[test]
fn shared_native_open_validates_identity_fields_limits_and_listener_lifetime() {
    use super::super::gate_session as session;
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let body = json!({"intent":fixture.job_intent("run", None),"manifest":std::fs::read_to_string(fixture.root.join("trusted-policy.json")).unwrap(),"bootstrap":fixture.bootstrap_identity()});
    let before = fixture.actor.view().unwrap().projection().messages.len();
    let caller = Some(format!("actor-v1:{}", "a".repeat(64)));
    for actor in [None, Some(format!("actor-v1:{}", "b".repeat(64)))] {
        assert_eq!(
            session::open_native(
                &fixture.actor,
                "team-1",
                "%1",
                &capability_of("%1"),
                actor,
                &body.to_string()
            )
            .unwrap_err(),
            "authority_unavailable"
        );
    }
    for field in ["outer", "intent", "bootstrap"] {
        let mut bad = body.clone();
        if field == "outer" {
            bad["argv"] = json!(["send"]);
        } else {
            bad[field]["caller"] = "fixture-untrusted-caller".into();
        }
        assert_eq!(
            session::open_native(
                &fixture.actor,
                "team-1",
                "%1",
                &capability_of("%1"),
                caller.clone(),
                &bad.to_string()
            )
            .unwrap_err(),
            "unknown_field"
        );
    }
    assert_eq!(
        session::open_native(
            &fixture.actor,
            "team-1",
            "%1",
            &capability_of("%1"),
            caller.clone(),
            &"x".repeat(1024 * 1024 + 1)
        )
        .unwrap_err(),
        "invalid_request"
    );
    assert_eq!(
        fixture.actor.view().unwrap().projection().messages.len(),
        before
    );
    let opened = session::open_native(
        &fixture.actor,
        "team-1",
        "%1",
        &capability_of("%1"),
        caller,
        &body.to_string(),
    )
    .unwrap();
    let address =
        std::net::SocketAddr::from(([127, 0, 0, 1], opened["port"].as_u64().unwrap() as u16));
    let job = opened["job"].as_str().unwrap().to_string();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    session::write_frame(&mut stream, &json!({"capability":capability_of("%1")})).unwrap();
    assert_eq!(
        session::read_frame(&mut stream).unwrap().unwrap()["ok"],
        true
    );
    assert!(
        TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err(),
        "gate listener accepted another connection after binding"
    );
    drop(stream);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while fixture
        .actor
        .gate_session_alive(job.clone(), now())
        .unwrap()
        && std::time::Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!fixture.actor.gate_session_alive(job, now()).unwrap());
}

#[test]
fn native_handshake_accepts_delayed_header_and_body_fragments() {
    use super::super::gate_session as session;
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let observed = reuse_observation(&fixture);
    let registration = observed["registration"].as_str().unwrap();
    let body = json!({"intent":fixture.job_intent("verify", Some(registration)),"manifest":std::fs::read_to_string(fixture.root.join("trusted-policy.json")).unwrap(),"bootstrap":fixture.bootstrap_identity()});
    let opened = session::open_native(
        &fixture.actor,
        "team-1",
        "%1",
        &capability_of("%1"),
        Some(format!("actor-v1:{}", "a".repeat(64))),
        &body.to_string(),
    )
    .unwrap();
    let address =
        std::net::SocketAddr::from(([127, 0, 0, 1], opened["port"].as_u64().unwrap() as u16));
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut handshake = Vec::new();
    session::write_frame(&mut handshake, &json!({"capability":capability_of("%1")})).unwrap();
    let last_byte = handshake.len() - 1;
    for fragment in [
        &handshake[..0],
        &handshake[..1],
        &handshake[1..4],
        &handshake[4..last_byte],
    ] {
        stream.write_all(fragment).unwrap();
        let mut unsolicited_reply = [0_u8; 1];
        let pending = stream.peek(&mut unsolicited_reply).unwrap_err();
        assert!(
            matches!(
                pending.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ),
            "incomplete handshake did not remain open: {pending}"
        );
    }
    stream.write_all(&handshake[last_byte..]).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let bound = session::read_frame(&mut stream).unwrap().unwrap();
    assert_eq!(bound["ok"], true, "{bound}");
    let hit = scope_call(
        &mut stream,
        json!({"verb":"gate-reuse","body":observed,"retryRequest":"scope-fragmented-handshake"}),
    );
    assert_eq!(hit["ok"], true, "{hit}");
    assert_eq!(hit["value"]["decision"], "hit");
}

#[test]
fn stale_retained_context_is_checked_before_cached_registration_after_pin_replacement() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let observed = reuse_observation(&fixture);
    let (job, mut stream) = fixture.scoped_connection("run", None);
    let retry = "retained-context-cache";
    let registered = scope_call(
        &mut stream,
        json!({"verb":"gate-register","body":observed["inputs"],"retryRequest":retry}),
    );
    assert_eq!(registered["ok"], true);
    let mut retained = fixture.retained_registration_command(&job, &observed["inputs"], retry);
    let cached = fixture.actor.plan(retained.clone()).unwrap().0;
    assert_eq!(
        serde_json::from_str::<Value>(&cached.reply.stdout).unwrap()["registration"],
        registered["value"]["registration"]
    );
    fixture.pin_current_policy("retained-context-replaced-pin");
    retained.now_ms = now();
    let before = fixture.actor.view().unwrap().projection().messages.len();
    match fixture.actor.plan(retained) {
        Err(RuntimeError::GateJobRefused(reason)) => {
            assert!(reason.starts_with("stale_pin:"), "{reason}")
        }
        other => panic!("retained stale context reached the core retry path: {other:?}"),
    }
    assert_eq!(
        fixture.actor.view().unwrap().projection().messages.len(),
        before
    );
}

#[test]
fn a_real_run_takeover_invalidates_retained_context_without_a_restart() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let observed = reuse_observation(&fixture);
    let (job, mut stream) = fixture.scoped_connection("run", None);
    let retry = "takeover-context-cache";
    let registered = scope_call(
        &mut stream,
        json!({"verb":"gate-register","body":observed["inputs"],"retryRequest":retry}),
    );
    assert_eq!(registered["ok"], true);
    let mut retained = fixture.retained_registration_command(&job, &observed["inputs"], retry);
    assert_eq!(
        fixture
            .actor
            .plan(retained.clone())
            .unwrap()
            .0
            .reply
            .exit_code,
        0
    );
    let taken = fixture.takeover("team-2", "team-1/%1", 'b', "scope-real-takeover");
    assert_eq!(taken["generation"], 2);
    retained.now_ms = now();
    let before = fixture.actor.view().unwrap().projection().messages.len();
    match fixture.actor.plan(retained) {
        Err(RuntimeError::GateJobRefused(reason)) => {
            assert!(reason.starts_with("stale_pin:"), "{reason}")
        }
        other => panic!("takeover did not reject the retained context before retry: {other:?}"),
    }
    assert_eq!(
        fixture.actor.view().unwrap().projection().messages.len(),
        before
    );
}

#[test]
fn monotonic_deadline_expires_while_the_wall_clock_budget_is_still_valid() {
    use super::super::gate_session::Request;
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let owner = PlanCommand::checked(
        vec!["native-gate-open".into()],
        "team-1",
        "%1",
        capability_of("%1"),
        Some(format!("actor-v1:{}", "a".repeat(64))),
        now(),
    )
    .unwrap();
    let opened = fixture
        .actor
        .open_gate_session(
            owner,
            fixture.job_intent("run", None),
            std::fs::read_to_string(fixture.root.join("trusted-policy.json")).unwrap(),
            fixture.bootstrap_identity(),
        )
        .unwrap();
    assert_eq!(opened["ok"], true);
    assert!(opened["value"]["expiresMs"].as_i64().unwrap() > now() + 1000);
    let job = opened["value"]["job"].as_str().unwrap().to_string();
    assert!(
        fixture
            .actor
            .gate_session_alive(job.clone(), now())
            .unwrap()
    );
    fixture
        .actor
        .gate_session_request(Request::ExpireDeadline { job: job.clone() })
        .unwrap();
    let refused = fixture
        .actor
        .gate_session_request(Request::Alive { job, now_ms: now() })
        .unwrap();
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["code"], "expired");
    assert!(opened["value"]["expiresMs"].as_i64().unwrap() > now());
}

#[test]
fn general_cli_requires_a_server_owned_job_before_registration_or_cached_evidence() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    fixture.install_probe();
    let probe: Value =
        serde_json::from_slice(&std::fs::read(fixture.input.join("probe.json")).unwrap()).unwrap();
    fixture.cli(&[
        "gate-policy-pin",
        "--gate",
        "gate-root",
        "--body",
        &json!({"version":1,"policy":"b".repeat(64),"producer":"b".repeat(64)}).to_string(),
        "--retry-request",
        "generic-reviewed-pin",
    ]);
    let refused = fixture.raw_cli(&[
        "gate-register",
        "--task",
        &fixture.task,
        "--attempt",
        "none",
        "--body",
        &probe["inputs"].to_string(),
        "--retry-request",
        "generic-registration",
    ]);
    assert!(
        !refused.status.success(),
        "general CLI created gate evidence without a server-owned job"
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("out_of_scope:"));
    assert!(fixture.production("run", true).status.success());
    assert!(fixture.production("verify", true).status.success());
    let observed = reuse_observation(&fixture);
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("out/metadata.json")).unwrap())
            .unwrap();
    let (job, mut stream) = fixture.scoped_connection("run", None);
    let retry = "generic-cache-probe";
    let registered = scope_call(
        &mut stream,
        json!({"verb":"gate-register","body":observed["inputs"],"retryRequest":retry}),
    );
    assert_eq!(registered["ok"], true, "{registered}");
    let cached_retry = format!(
        "gate-job-{}",
        zerocode_core::user_preferences::content_key(format!("{job}:{retry}").as_bytes())
    );
    let before = fixture.actor.view().unwrap().projection().messages.len();
    for (verb, body, extra, retry) in [
        (
            "gate-register",
            observed["inputs"].clone(),
            Vec::new(),
            cached_retry,
        ),
        (
            "gate-record",
            observed["inputs"].clone(),
            vec![
                "--start".to_string(),
                metadata["registration"].as_str().unwrap().into(),
                "--lease".into(),
                metadata["lease"].as_str().unwrap().into(),
            ],
            "generic-start".into(),
        ),
        (
            "gate-record",
            metadata.clone(),
            Vec::new(),
            "generic-finish".into(),
        ),
        (
            "gate-reuse",
            observed.clone(),
            Vec::new(),
            "generic-reuse".into(),
        ),
    ] {
        let mut argv = vec![
            verb.to_string(),
            "--run".into(),
            fixture.run.clone(),
            "--task".into(),
            fixture.task.clone(),
            "--attempt".into(),
            "none".into(),
            "--body".into(),
            body.to_string(),
            "--retry-request".into(),
            retry,
        ];
        argv.extend(extra);
        let refused = fixture.raw_cli(&argv.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(!refused.status.success(), "general CLI accepted {verb}");
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("out_of_scope:"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
    }
    assert_eq!(
        fixture.actor.view().unwrap().projection().messages.len(),
        before
    );
}

#[test]
fn native_arguments_cannot_substitute_an_unpinned_producer_after_policy_verification() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let policy = fixture.policy.lock().unwrap().clone().unwrap();
    let original_path = fixture.root.join("trusted-policy.json");
    let mut shadow: Value =
        serde_json::from_slice(&std::fs::read(&original_path).unwrap()).unwrap();
    let marker = fixture.root.join("unpinned-producer-ran");
    let producer = fixture.root.join("unpinned-producer.py");
    std::fs::write(&producer, format!("from pathlib import Path\nPath({:?}).write_text('ran')\ndef main(trust=None, argv=None):\n    return 0\n", marker.to_str().unwrap())).unwrap();
    shadow["bundle"]["producer"] = json!({"path":producer,"sha256":zerocode_core::user_preferences::content_key(&std::fs::read(&producer).unwrap())});
    let shadow_path = fixture.root.join("unpinned-policy.json");
    let shadow_bytes = serde_json::to_vec(&shadow).unwrap();
    std::fs::write(&shadow_path, &shadow_bytes).unwrap();
    let shadow_sha = zerocode_core::user_preferences::content_key(&shadow_bytes);
    let observed = reuse_observation(&fixture);
    let intent = fixture.job_intent("verify", observed["registration"].as_str());
    let options: Vec<String> = serde_json::from_value(policy["bootstrap"].clone()).unwrap();
    let original_sha = policy["policySha256"].as_str().unwrap().to_string();
    let prefix = vec![
        "--policy".to_string(),
        original_path.to_str().unwrap().into(),
        "--policy-sha256".into(),
        original_sha.clone(),
        "--repo".into(),
        fixture.input.to_str().unwrap().into(),
    ];
    let mut producer_args = vec![
        "verify".to_string(),
        "--repo".into(),
        fixture.input.to_str().unwrap().into(),
        "--gate".into(),
        "gate-root".into(),
        "--task".into(),
        fixture.task.clone(),
        "--attempt".into(),
        "none".into(),
        "--scope".into(),
        "deterministic".into(),
        "--closure".into(),
        "declared".into(),
        "--profile".into(),
        "test-debug".into(),
        "--timeout".into(),
        "120".into(),
        "--metadata".into(),
        fixture
            .root
            .join("out/metadata.json")
            .to_str()
            .unwrap()
            .into(),
        "--log".into(),
        fixture.root.join("out/gate.log").to_str().unwrap().into(),
        "--".into(),
    ];
    producer_args.extend(serde_json::from_value::<Vec<String>>(policy["command"].clone()).unwrap());
    let invoke = |forwarded: &[String]| {
        let mut command = Command::new(native_bootstrap_binary());
        command
            .arg("gate-bootstrap")
            .args(&options)
            .arg("--gate-job")
            .arg(serde_json::to_string(&intent).unwrap())
            .arg("--")
            .args(forwarded)
            .arg("--")
            .args(&producer_args);
        fixture.environment(&mut command);
        command.output().unwrap()
    };
    let reordered = vec![
        prefix[4].clone(),
        prefix[5].clone(),
        prefix[2].clone(),
        prefix[3].clone(),
        prefix[0].clone(),
        prefix[1].clone(),
    ];
    let good = invoke(&reordered);
    assert!(
        good.status.success(),
        "{} {}",
        String::from_utf8_lossy(&good.stdout),
        String::from_utf8_lossy(&good.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&good.stdout).unwrap()["decision"],
        "hit"
    );
    let before = fixture.actor.view().unwrap().projection().messages.len();
    let opens = fixture.gate_open_attempts.load(Ordering::SeqCst);
    let mut equals = prefix.clone();
    equals.extend([
        format!("--policy={}", shadow_path.display()),
        format!("--policy-sha256={shadow_sha}"),
    ]);
    let mut duplicate = prefix.clone();
    duplicate.extend(["--policy".into(), shadow_path.to_str().unwrap().into()]);
    let mut abbreviated = prefix.clone();
    abbreviated[2] = "--policy-s".into();
    let mut reserved = prefix.clone();
    reserved.extend(["--authority-fd".into(), "3".into()]);
    let mut reserved_equals = prefix.clone();
    reserved_equals.extend([format!("--bootstrap-launcher={}", producer.display())]);
    let mut bound = prefix;
    bound.extend(["--bound-policy-sha256".into(), shadow_sha]);
    for malformed in [
        equals,
        duplicate,
        abbreviated,
        reserved,
        reserved_equals,
        bound,
    ] {
        let denied = invoke(&malformed);
        assert!(
            !denied.status.success(),
            "unpinned argument override was accepted"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&denied.stdout).unwrap()["code"],
            "invalid_request"
        );
        assert!(
            !marker.exists(),
            "unpinned producer ran after native policy verification"
        );
        assert_eq!(
            fixture.gate_open_attempts.load(Ordering::SeqCst),
            opens,
            "native argument rejection happened after opening job authority"
        );
        assert_eq!(
            fixture.actor.view().unwrap().projection().messages.len(),
            before
        );
    }
}

#[test]
fn native_bootstrap_binds_launcher_and_runtime_to_the_current_policy_before_python() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let before = fixture.actor.view().unwrap().projection().messages.len();
    let opens_before = fixture.gate_open_attempts.load(Ordering::SeqCst);
    let policy = fixture.policy.lock().unwrap().clone().unwrap();
    let manifest_path = fixture.root.join("trusted-policy.json");
    let original = std::fs::read(&manifest_path).unwrap();
    let original_manifest: Value = serde_json::from_slice(&original).unwrap();
    let marker = fixture.root.join("unapproved-python-ran");
    let launcher = fixture.root.join("unapproved-launcher.py");
    std::fs::write(
        &launcher,
        format!(
            "from pathlib import Path\nPath({:?}).write_text('ran')\n",
            marker.to_str().unwrap()
        ),
    )
    .unwrap();
    for role in ["launcher", "python", "stdlib", "libraries"] {
        let mut options: Vec<String> = serde_json::from_value(policy["bootstrap"].clone()).unwrap();
        let mut manifest = original_manifest.clone();
        if role == "launcher" {
            let path = options
                .iter()
                .position(|word| word == "--launcher")
                .unwrap()
                + 1;
            options[path] = launcher.to_str().unwrap().into();
            let hash = options
                .iter()
                .position(|word| word == "--launcher-sha256")
                .unwrap()
                + 1;
            options[hash] =
                zerocode_core::user_preferences::content_key(&std::fs::read(&launcher).unwrap());
        } else if role == "libraries" {
            assert!(
                !manifest["runtime"]["libraries"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            manifest["runtime"]["libraries"] = json!([]);
        } else {
            manifest["runtime"][role]["sha256"] = "0".repeat(64).into();
        }
        let bytes = if role == "launcher" {
            original.clone()
        } else {
            serde_json::to_vec(&manifest).unwrap()
        };
        std::fs::write(&manifest_path, &bytes).unwrap();
        let mut command = Command::new(native_bootstrap_binary());
        command
            .arg("gate-bootstrap")
            .args(&options)
            .arg("--gate-job")
            .arg(serde_json::to_string(&fixture.job_intent("run", None)).unwrap())
            .arg("--")
            .arg("--policy")
            .arg(&manifest_path)
            .arg("--policy-sha256")
            .arg(zerocode_core::user_preferences::content_key(&bytes))
            .arg("--repo")
            .arg(&fixture.input)
            .arg("--")
            .arg("run");
        fixture.environment(&mut command);
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "unapproved bootstrap executed Python for {role}"
        );
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["code"], "digest_mismatch", "{role}: {reply}");
        assert!(
            !marker.exists(),
            "unapproved launcher executed before native pin comparison"
        );
        assert_eq!(
            fixture.gate_open_attempts.load(Ordering::SeqCst),
            opens_before,
            "native bootstrap contacted the job server before comparing pinned identities"
        );
        assert_eq!(
            fixture.actor.view().unwrap().projection().messages.len(),
            before
        );
        std::fs::write(&manifest_path, &original).unwrap();
    }
    for field in [
        "overheadTimeoutSeconds",
        "verificationTimeoutSeconds",
        "maxJobTtlSeconds",
    ] {
        let mut manifest = original_manifest.clone();
        manifest["gates"]["gate-root"][field] = true.into();
        let bytes = serde_json::to_vec(&manifest).unwrap();
        std::fs::write(&manifest_path, &bytes).unwrap();
        let mut command = Command::new(native_bootstrap_binary());
        let options: Vec<String> = serde_json::from_value(policy["bootstrap"].clone()).unwrap();
        command
            .arg("gate-bootstrap")
            .args(options)
            .arg("--gate-job")
            .arg(serde_json::to_string(&fixture.job_intent("run", None)).unwrap())
            .arg("--")
            .arg("--policy")
            .arg(&manifest_path)
            .arg("--policy-sha256")
            .arg(zerocode_core::user_preferences::content_key(&bytes))
            .arg("--repo")
            .arg(&fixture.input)
            .arg("--")
            .arg("run");
        fixture.environment(&mut command);
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["code"], "invalid_request", "{field}: {reply}");
        assert_eq!(
            fixture.gate_open_attempts.load(Ordering::SeqCst),
            opens_before
        );
        assert!(!marker.exists());
        assert_eq!(
            fixture.actor.view().unwrap().projection().messages.len(),
            before
        );
        std::fs::write(&manifest_path, &original).unwrap();
    }
    let mut changed = fixture.bootstrap_identity();
    changed.python.sha256 = "0".repeat(64);
    let owner = PlanCommand::checked(
        vec!["native-gate-open".into()],
        "team-1",
        "%1",
        capability_of("%1"),
        Some(format!("actor-v1:{}", "a".repeat(64))),
        now(),
    )
    .unwrap();
    let refused = fixture
        .actor
        .open_gate_session(
            owner,
            fixture.job_intent("run", None),
            String::from_utf8(original).unwrap(),
            changed,
        )
        .unwrap();
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["code"], "digest_mismatch");
    assert_eq!(
        fixture.actor.view().unwrap().projection().messages.len(),
        before
    );
    assert!(fixture.production("verify", true).status.success());
}

#[test]
fn scoped_opening_requires_current_coordinator_identity_and_bounded_slots() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let intent = fixture.job_intent("run", None);
    let manifest = std::fs::read_to_string(fixture.root.join("trusted-policy.json")).unwrap();
    let open = |pane: &str, capability: String, actor: String| {
        let owner = PlanCommand::checked(
            vec!["native-gate-open".into()],
            "team-1",
            pane,
            capability,
            Some(actor),
            now(),
        )
        .unwrap();
        fixture
            .actor
            .open_gate_session(
                owner,
                intent.clone(),
                manifest.clone(),
                fixture.bootstrap_identity(),
            )
            .unwrap()
    };
    let actor = format!("actor-v1:{}", "a".repeat(64));
    for (pane, token, caller) in [
        ("%1", "fixture-wrong-capability".into(), actor.clone()),
        ("%2", capability_of("%2"), actor.clone()),
        (
            "%1",
            capability_of("%1"),
            format!("actor-v1:{}", "b".repeat(64)),
        ),
    ] {
        let rejected = open(pane, token, caller);
        assert_eq!(
            rejected["ok"], false,
            "non-coordinator opened a gate scope: {rejected}"
        );
    }
    let mut jobs = Vec::new();
    for _ in 0..32 {
        let opened = open("%1", capability_of("%1"), actor.clone());
        assert_eq!(opened["ok"], true, "{opened}");
        jobs.push(opened["value"]["job"].as_str().unwrap().to_string());
    }
    let full = open("%1", capability_of("%1"), actor.clone());
    assert_eq!(full["ok"], false, "gate scopes exceeded their bound");
    assert_eq!(full["code"], "out_of_scope");
    let rejected = fixture
        .actor
        .bind_gate_session(jobs[0].clone(), capability_of("%2"), now())
        .unwrap();
    assert_eq!(
        rejected["ok"], false,
        "another pane bound the gate connection"
    );
    assert!(
        !fixture
            .actor
            .gate_session_alive(jobs[0].clone(), now())
            .unwrap()
    );
    let replacement = open("%1", capability_of("%1"), actor);
    assert_eq!(
        replacement["ok"], true,
        "failed binding did not release capacity"
    );
    jobs.push(replacement["value"]["job"].as_str().unwrap().to_string());
    let bound = fixture
        .actor
        .bind_gate_session(jobs[1].clone(), capability_of("%1"), now())
        .unwrap();
    assert_eq!(bound["ok"], true);
    let twice = fixture
        .actor
        .bind_gate_session(jobs[1].clone(), capability_of("%1"), now())
        .unwrap();
    assert_eq!(
        twice["ok"], false,
        "a second connection bound the same scope"
    );
    assert!(
        !fixture
            .actor
            .gate_session_alive(jobs[1].clone(), now())
            .unwrap()
    );
    for job in jobs {
        fixture.actor.close_gate_session(job).unwrap();
    }
}

#[test]
fn a_scoped_connection_cannot_use_general_verbs_or_unknown_fields() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    let output = fixture.production("run", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let observed = reuse_observation(&fixture);
    let registration = observed["registration"].as_str().unwrap();
    let (_, mut valid) = fixture.scoped_connection("verify", Some(registration));
    let hit = scope_call(
        &mut valid,
        json!({"verb":"gate-reuse","body":observed,"retryRequest":"scope-valid-query"}),
    );
    assert_eq!(hit["ok"], true, "{hit}");
    assert_eq!(hit["value"]["decision"], "hit");
    fixture.cli(&[
        "send",
        "--type",
        "status",
        "--to",
        &format!("run:{}", fixture.run),
        "--body",
        "fixture coordinator control",
        "--retry-request",
        "scope-coordinator-control",
    ]);
    let count = fixture.actor.view().unwrap().projection().messages.len();
    for verb in [
        "send",
        "kill",
        "task-update",
        "worker-start",
        "worker-stop",
        "gate-policy-pin",
    ] {
        let (job, mut stream) = fixture.scoped_connection("verify", Some(registration));
        let refused = scope_call(
            &mut stream,
            json!({"verb":verb,"body":{},"retryRequest":"scope-forbidden-verb"}),
        );
        assert_eq!(refused["ok"], false, "{verb}: {refused}");
        assert_eq!(refused["code"], "out_of_scope");
        assert!(!fixture.actor.gate_session_alive(job, now()).unwrap());
    }
    let (_, mut stream) = fixture.scoped_connection("verify", Some(registration));
    let argv = vec![
        "send",
        "--type",
        "status",
        "--to",
        &format!("run:{}", fixture.run),
        "--body",
        "fixture scoped write probe",
        "--retry-request",
        "scope-write-probe",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let refused = scope_call(
        &mut stream,
        json!({"verb":"send","body":{},"retryRequest":"scope-raw-plan-probe","argv":argv}),
    );
    assert_eq!(
        refused["ok"], false,
        "scoped connection performed an unrelated ledger write: {refused}"
    );
    assert_eq!(refused["code"], "unknown_field");
    assert_eq!(
        fixture.actor.view().unwrap().projection().messages.len(),
        count,
        "scoped connection changed the ledger"
    );
}

#[test]
fn scoped_connections_bind_receipts_and_revoke_on_eof_cancel_pin_restart_and_expiry() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let observed = reuse_observation(&fixture);
    let registration = observed["registration"].as_str().unwrap();
    let count = fixture.actor.view().unwrap().projection().messages.len();
    let (job, mut stream) = fixture.scoped_connection("verify", Some(registration));
    let mut other = observed.clone();
    other["registration"] = "m-other-receipt".into();
    let refused = scope_call(
        &mut stream,
        json!({"verb":"gate-reuse","body":other,"retryRequest":"scope-other-receipt"}),
    );
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["code"], "out_of_scope");
    assert!(!fixture.actor.gate_session_alive(job, now()).unwrap());
    assert_eq!(
        fixture.actor.view().unwrap().projection().messages.len(),
        count
    );
    let (job, stream) = fixture.scoped_connection("verify", Some(registration));
    drop(stream);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while fixture
        .actor
        .gate_session_alive(job.clone(), now())
        .unwrap()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !fixture.actor.gate_session_alive(job, now()).unwrap(),
        "EOF did not revoke scope"
    );
    let (cancelled_job, mut cancelled) = fixture.scoped_connection("run", None);
    let registered = scope_call(
        &mut cancelled,
        json!({"verb":"gate-register","body":observed["inputs"],"retryRequest":"scope-to-cancel"}),
    );
    assert_eq!(registered["ok"], true);
    let registered = &registered["value"];
    fixture.cli(&[
        "gate-record",
        "--task",
        &fixture.task,
        "--attempt",
        "none",
        "--cancel",
        registered["registration"].as_str().unwrap(),
        "--lease",
        registered["lease"].as_str().unwrap(),
        "--retry-request",
        "scope-native-cancel",
    ]);
    assert!(
        !fixture
            .actor
            .gate_session_alive(cancelled_job, now())
            .unwrap(),
        "cancellation did not revoke scope"
    );
    let (job, _stream) = fixture.scoped_connection("verify", Some(registration));
    fixture.pin_current_policy("scope-replacement-pin");
    assert!(!fixture.actor.gate_session_alive(job, now()).unwrap());
    fixture.pin_current_policy("scope-restored-pin");
    let intent = fixture.job_intent("run", None);
    let owner = PlanCommand::checked(
        vec!["native-gate-open".into()],
        "team-1",
        "%1",
        capability_of("%1"),
        Some(format!("actor-v1:{}", "a".repeat(64))),
        now(),
    )
    .unwrap();
    let opened = fixture
        .actor
        .open_gate_session(
            owner,
            intent,
            std::fs::read_to_string(fixture.root.join("trusted-policy.json")).unwrap(),
            fixture.bootstrap_identity(),
        )
        .unwrap();
    assert_eq!(opened["ok"], true);
    let job = opened["value"]["job"].as_str().unwrap().to_string();
    let expired = fixture
        .actor
        .bind_gate_session(
            job.clone(),
            capability_of("%1"),
            opened["value"]["expiresMs"].as_i64().unwrap() + 1,
        )
        .unwrap();
    assert_eq!(expired["ok"], false);
    assert_eq!(expired["code"], "expired");
    assert!(!fixture.actor.gate_session_alive(job, now()).unwrap());
    let (job, _stream) = fixture.scoped_connection("run", None);
    fixture.actor.window_restarted(now()).unwrap();
    fixture.cli(&[
        "run-use",
        &fixture.run,
        "--retry-request",
        "scope-generation-reclaim",
    ]);
    assert!(!fixture.actor.gate_session_alive(job, now()).unwrap());
}

#[test]
fn scoped_registration_retries_are_exact_and_do_not_cross_job_connections() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let observed = reuse_observation(&fixture);
    let request = json!({"verb":"gate-register","body":observed["inputs"],"retryRequest":"same-scoped-registration"});
    let (one_job, mut one) = fixture.scoped_connection("run", None);
    let first = scope_call(&mut one, request.clone());
    assert_eq!(first["ok"], true, "{first}");
    let exact = scope_call(&mut one, request.clone());
    assert_eq!(
        exact["value"]["registration"],
        first["value"]["registration"]
    );
    let (_, mut two) = fixture.scoped_connection("run", None);
    let other = scope_call(&mut two, request);
    assert_eq!(other["ok"], true, "{other}");
    assert_ne!(
        other["value"]["registration"], first["value"]["registration"],
        "retry crossed job connections"
    );
    fixture.pin_current_policy("scope-retry-pin-replacement");
    assert!(
        !fixture
            .actor
            .gate_session_alive(one_job.clone(), now())
            .unwrap()
    );
    let replay = fixture.actor.command_gate_session(one_job, json!({"verb":"gate-register","body":observed["inputs"],"retryRequest":"same-scoped-registration"}), now()).unwrap();
    assert_eq!(
        replay["ok"], false,
        "cached registration bypassed current scope"
    );
    assert_eq!(replay["code"], "out_of_scope");
}

#[test]
fn generation_change_requires_one_measured_fixture_recheck() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let original_bytes = std::fs::read(fixture.root.join("out/metadata.json")).unwrap();
    let original: Value = serde_json::from_slice(&original_bytes).unwrap();
    assert!(fixture.production("verify", true).status.success());
    let manifest = std::fs::read(fixture.root.join("trusted-policy.json")).unwrap();
    assert_eq!(
        fixture.takeover("team-2", "team-1/%1", 'b', "generation-fixture-away")["generation"],
        2
    );
    assert_eq!(
        fixture.takeover("team-1", "team-2/%1", 'a', "generation-fixture-return")["generation"],
        3
    );
    let miss = fixture.miss(true);
    assert_eq!(miss["code"], "stale_pin");
    fixture.pin_current_policy("generation-fixture-new-pin");
    *fixture.output.lock().unwrap() = fixture.root.join("out-generation");
    let started = std::time::Instant::now();
    let output = fixture.production("run", true);
    let elapsed_ms = started.elapsed().as_millis();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let current: Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("out-generation/metadata.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        current["inputs"], original["inputs"],
        "recheck changed more than coordinator generation and pin ID"
    );
    assert_ne!(current["policyPin"], original["policyPin"]);
    assert_eq!(
        std::fs::read(fixture.root.join("trusted-policy.json")).unwrap(),
        manifest
    );
    assert!(fixture.production("verify", true).status.success());
    let view = fixture.actor.view().unwrap();
    let completions = view
        .projection()
        .messages
        .iter()
        .filter(|row| {
            serde_json::from_str::<Value>(row.payload.as_str())
                .ok()
                .is_some_and(|value| value["gateEvidenceV1"]["phase"] == "completed")
        })
        .count();
    assert_eq!(completions, 2);
    println!(
        "{}",
        json!({"fixtureGenerationRecheck":{"extraExecutions":1,"wallMs":elapsed_ms,"gateDurationMs":current["durationMs"],
        "originalGateDurationMs":original["durationMs"],"productionMeasured":false}})
    );
}

#[test]
fn production_cli_real_authority_and_real_process_issue_and_reuse_one_receipt() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    let initial = fixture.actor.view().unwrap().revision();
    let output = fixture.production("run", true);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let produced: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(produced["tests"], 5);
    let hit = fixture.production("verify", true);
    assert!(
        hit.status.success(),
        "{}",
        String::from_utf8_lossy(&hit.stdout)
    );
    let hit: Value = serde_json::from_slice(&hit.stdout).unwrap();
    assert_eq!(hit["decision"], "hit");
    assert_eq!(hit["original"], produced["original"]);
    assert_eq!(
        hit["mandatoryChecks"],
        json!(["native", "windows", "flake-judgment", "publish"])
    );
    assert!(fixture.actor.view().unwrap().revision() > initial);

    let metadata_path = fixture.root.join("out/metadata.json");
    let log_path = fixture.root.join("out/gate.log");
    let metadata_bytes = std::fs::read(&metadata_path).unwrap();
    let metadata: Value = serde_json::from_slice(&metadata_bytes).unwrap();
    let log = std::fs::read(&log_path).unwrap();
    std::fs::write(&metadata_path, [metadata_bytes.as_slice(), b"\n"].concat()).unwrap();
    fixture.miss(true);
    std::fs::write(&metadata_path, &metadata_bytes).unwrap();
    std::fs::write(&log_path, [log.as_slice(), b"changed log\n"].concat()).unwrap();
    fixture.miss(true);
    std::fs::write(&log_path, log).unwrap();
    assert!(fixture.production("verify", false).status.success());
    assert!(
        zerocode_core::host::lock_free_git(Command::new("git"))
            .env_clear()
            .env(zerocode_core::host::GIT_OPTIONAL_LOCKS, "0")
            .env("HOME", fixture.root.join("home"))
            .env("PATH", std::env::var("PATH").unwrap())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .arg("-C")
            .arg(&fixture.input)
            .args(["commit", "--allow-empty", "-qm", "another candidate"])
            .status()
            .unwrap()
            .success()
    );
    fixture.miss(true);

    let id = metadata["registration"].as_str().unwrap();
    let lease = metadata["lease"].as_str().unwrap();
    fixture.cli(&[
        "gate-record",
        "--task",
        &fixture.task,
        "--attempt",
        "none",
        "--cancel",
        id,
        "--lease",
        lease,
        "--retry-request",
        "fixture-cancel",
    ]);
    fixture.miss(true);

    let (job, mut stream) = fixture.scoped_connection("run", None);
    let registered = scope_call(
        &mut stream,
        json!({"verb":"gate-register","body":metadata["inputs"],"retryRequest":"fixture-register-race"}),
    );
    assert_eq!(registered["ok"], true, "{registered}");
    let registered = &registered["value"];
    let start = |retry: &str| {
        json!({"verb":"gate-record","body":metadata["inputs"],"retryRequest":retry,
        "start":registered["registration"],"lease":registered["lease"]})
    };
    let successes = thread::scope(|scope| {
        let first = start("fixture-race-one");
        let second = start("fixture-race-two");
        let actor = &fixture.actor;
        let job = &job;
        let one = scope.spawn(move || {
            actor
                .command_gate_session(job.clone(), first, now())
                .unwrap()
        });
        let two = scope.spawn(move || {
            actor
                .command_gate_session(job.clone(), second, now())
                .unwrap()
        });
        [one.join().unwrap(), two.join().unwrap()]
            .iter()
            .filter(|answer| answer["ok"] == true)
            .count()
    });
    assert_eq!(successes, 1, "one scoped job authorizes one start");
    fixture.old_api.store(true, Ordering::SeqCst);
    fixture.miss(true); // The same product shim against an old API causes a miss, never a hit.
}

#[test]
fn a_zero_ui_recipe_cannot_reuse_an_actual_passing_rust_recipe() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    let output = fixture.production_fixture("run", true, CHILD, false, true);
    let produced: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(produced["newExecutions"], 1);
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("out/metadata.json")).unwrap())
            .unwrap();
    assert_eq!(metadata["rc"], 0);
    assert_eq!(metadata["executed"], true);
    assert_eq!(metadata["inputsUnchanged"], true);
    let log = std::fs::read_to_string(fixture.root.join("out/gate.log")).unwrap();
    assert!(
        log.contains("test result: ok. 1 passed;"),
        "actual Rust fixture did not pass"
    );
    assert!(
        log.contains("1/1 passed"),
        "actual UI positive report did not run"
    );
    assert!(
        log.contains("0/0 passed"),
        "actual zero UI report did not run"
    );
    assert!(log.contains("<== verify recipe rust rc=0"));
    assert!(log.contains("<== verify recipe ui rc=0"));
    let verified = fixture.production("verify", true);
    let decision: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(
        decision["decision"], "miss",
        "actual zero UI recipe obtained a reusable hit: {decision}"
    );
    assert_eq!(decision["newExecutions"], 0);
    assert!(!output.status.success());
    assert_eq!(produced["outcome"], "unknown");
    assert_eq!(produced["tests"], 0);
}

#[test]
fn offline_cargo_first_writes_and_ignored_tests_issue_a_hit_and_changed_config_misses() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    let output = fixture.production_fixture("run", true, CHILD, true, false);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let produced: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(produced["outcome"], "passed");
    assert_eq!(produced["tests"], 1);
    let log = std::fs::read_to_string(fixture.root.join("out/gate.log")).unwrap();
    assert!(
        log.contains("1 passed; 0 failed; 1 ignored;"),
        "actual Cargo fixture did not run: {log}"
    );
    assert!(fixture.input.join("cargo-fixture/target").is_dir());
    assert!(fixture.input.join("tools/release/__pycache__").is_dir());
    let output = fixture.production_fixture("verify", true, CHILD, true, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let hit: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(hit["decision"], "hit");
    assert_eq!(hit["original"], produced["original"]);
    assert_eq!(hit["newExecutions"], 0);
    let config = fixture.root.join("child-home/cargo/config.toml");
    let original = std::fs::read(&config).unwrap();
    std::fs::write(&config, b"[env]\nFIXTURE_INPUT = \"changed\"\n").unwrap();
    fixture.miss(true);
    std::fs::write(&config, original).unwrap();
    assert!(
        fixture
            .production_fixture("verify", true, CHILD, true, false)
            .status
            .success()
    );
    std::fs::write(
        fixture.input.join("cargo-fixture/src/lib.rs"),
        b"compile_error!(\"changed actual input\");\n",
    )
    .unwrap();
    fixture.miss(true);
    let view = fixture.actor.view().unwrap();
    let completions = view
        .projection()
        .messages
        .iter()
        .filter(|row| {
            let body: Value = serde_json::from_str(row.payload.as_str()).unwrap_or_default();
            body["gateEvidenceV1"]["phase"] == "completed"
        })
        .count();
    assert_eq!(completions, 1);
}

#[test]
fn concurrent_policy_replacement_and_registration_use_one_current_pin() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    assert!(fixture.production("run", true).status.success());
    let observed = reuse_observation(&fixture);
    let initial = fixture.policy.lock().unwrap().clone().unwrap()["pin"].clone();
    let (job, _stream) = fixture.scoped_connection("run", None);
    let barrier = std::sync::Barrier::new(3);
    let (replacement, registration) = thread::scope(|scope| {
        let one = scope.spawn(|| {
            barrier.wait();
            fixture.raw_cli(&[
                "gate-policy-pin",
                "--gate",
                "gate-root",
                "--body",
                &json!({"version":1,"policy":"c".repeat(64),"producer":"c".repeat(64)}).to_string(),
                "--retry-request",
                "fixture-concurrent-policy-pin",
            ])
        });
        let two = scope.spawn(|| {
            barrier.wait();
            fixture
                .actor
                .command_gate_session(
                    job.clone(),
                    json!({"verb":"gate-register","body":observed["inputs"],
                "retryRequest":"fixture-concurrent-registration"}),
                    now(),
                )
                .unwrap()
        });
        barrier.wait();
        (one.join().unwrap(), two.join().unwrap())
    });
    assert!(replacement.status.success());
    let replacement: Value = serde_json::from_slice(&replacement.stdout).unwrap();
    assert_ne!(replacement["pin"], initial);
    if registration["ok"] == true {
        let registered = &registration["value"];
        assert_eq!(registered["policyPin"], initial);
        let start = fixture.actor.command_gate_session(job, json!({"verb":"gate-record","body":observed["inputs"],
            "start":registered["registration"],"lease":registered["lease"],"retryRequest":"fixture-concurrent-old-start"}), now()).unwrap();
        assert_eq!(start["ok"], false);
    } else {
        assert_eq!(registration["code"], "out_of_scope");
    }
    fixture.pin_current_policy("fixture-concurrent-restored-manifest-pin");
    let (_, mut fresh) = fixture.scoped_connection("run", None);
    let registered = scope_call(
        &mut fresh,
        json!({"verb":"gate-register","body":observed["inputs"],"retryRequest":"fixture-concurrent-new-registration"}),
    );
    assert_eq!(registered["ok"], true, "{registered}");
    assert_ne!(registered["value"]["policyPin"], initial);
}
