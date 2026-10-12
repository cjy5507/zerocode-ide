//! Real SQLite authority + generated product CLI + production Python functions.
//! The HTTP listener is test transport. All capabilities and the HOME belong to this fixture.
//! This does not launch the installed window, use its accounts, or validate a release gate.
use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use zerocode_core::agent_teams::{TEAM_ID_VAR, TEAM_PANE_VAR, TEAM_TOKEN_VAR, shim_script};

const HOOK: &str = "test-only-hook-capability";
const CHILD: &str = "runtime_actor::tests::gate_reuse::receipt_gate_child";

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
    actor: Arc<RuntimeActor>,
    stop: Arc<AtomicBool>,
    old_api: Arc<AtomicBool>,
    gate_open_attempts: Arc<AtomicUsize>,
    panes: TableFixture,
    serving: Option<thread::JoinHandle<()>>,
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

fn http_request(
    stream: &mut TcpStream,
) -> (std::collections::HashMap<String, String>, Vec<u8>, String) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut bytes = Vec::new();
    let split = loop {
        let mut block = [0_u8; 4096];
        let n = stream.read(&mut block).unwrap();
        assert!(
            n > 0 && bytes.len() + n <= 128 * 1024,
            "bounded complete fixture request"
        );
        bytes.extend_from_slice(&block[..n]);
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers: std::collections::HashMap<String, String> = std::str::from_utf8(&bytes[..split])
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().into()))
        .collect();
    let length = headers["content-length"].parse::<usize>().unwrap();
    assert!(length <= 64 * 1024);
    while bytes.len() < split + length {
        let mut block = [0_u8; 4096];
        let n = stream.read(&mut block).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&block[..n]);
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
    (headers, bytes[split..split + length].to_vec(), path)
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
                Command::new("git")
                    .env_clear()
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
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let old_api = Arc::new(AtomicBool::new(false));
        let gate_open_attempts = Arc::new(AtomicUsize::new(0));
        let serving = {
            let actor = Arc::clone(&actor);
            let stop = Arc::clone(&stop);
            let old_api = Arc::clone(&old_api);
            let gate_open_attempts = Arc::clone(&gate_open_attempts);
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(held) => held,
                        Err(why) if why.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(why) => panic!("fixture transport: {why}"),
                    };
                    let (headers, bytes, path) = http_request(&mut stream);
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
                    let (status, body) = if headers.get("x-zerocode-hook-token").map(String::as_str)
                        != Some(HOOK)
                    {
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
                    };
                    write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            })
        };
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/release/gate_receipt.py");
        let mut held = Self {
            actor,
            stop,
            old_api,
            gate_open_attempts,
            panes,
            serving: Some(serving),
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
                Command::new("git")
                    .env_clear()
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
                for key in [
                    "HERDR_FIXTURE_TOOLCHAIN",
                    "HERDR_FIXTURE_DEVELOPER_DIR",
                    "HERDR_FIXTURE_SDKROOT",
                    "HERDR_FIXTURE_LINKER",
                ] {
                    prepare.env(
                        key,
                        std::env::var_os(key).expect("approved offline fixture input"),
                    );
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
        let native =
            std::env::var_os("HERDR_FIXTURE_BOOTSTRAP").expect("approved native fixture binary");
        let mut command = Command::new(native);
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

impl Drop for CliFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.serving.take().unwrap().join().unwrap();
    }
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
    for role in ["launcher", "python", "stdlib"] {
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
        } else {
            manifest["runtime"][role]["sha256"] = "0".repeat(64).into();
        }
        let bytes = if role == "launcher" {
            original.clone()
        } else {
            serde_json::to_vec(&manifest).unwrap()
        };
        std::fs::write(&manifest_path, &bytes).unwrap();
        let mut command = Command::new(std::env::var_os("HERDR_FIXTURE_BOOTSTRAP").unwrap());
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
            .arg(&fixture.input);
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
        let mut command = Command::new(std::env::var_os("HERDR_FIXTURE_BOOTSTRAP").unwrap());
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
            .arg(&fixture.input);
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
fn scoped_connections_bind_receipts_and_revoke_on_eof_pin_generation_and_expiry() {
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
        Command::new("git")
            .env_clear()
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
    assert!(!output.status.success());
    let produced: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(produced["outcome"], "unknown");
    assert_eq!(produced["tests"], 0);
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
    fixture.miss(true);
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
