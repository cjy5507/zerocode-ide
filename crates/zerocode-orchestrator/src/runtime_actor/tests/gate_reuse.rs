//! Real SQLite authority + generated product CLI + production Python functions.
//! The HTTP listener is test transport. All capabilities and the HOME belong to this fixture.
//! This does not launch the installed window, use its accounts, or validate a release gate.
use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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
            assert!(
                reason.contains("this shell is not part of an agent team"),
                "probe failed without an authority refusal: {reason}"
            );
            println!("fixture probe denied missing authority");
            None
        }
    };
    let inputs = &probe["inputs"];
    let Some(registered) = call("gate-register", inputs, &[]) else {
        println!("fixture child has no ledger authority");
        return;
    };
    let id = registered["registration"].as_str().unwrap();
    let lease = registered["lease"].as_str().unwrap();
    let Some(started) = call("gate-record", inputs, &["--start", id, "--lease", lease]) else {
        return;
    };
    let metadata = json!({"version":1,"registration":id,"lease":lease,"execution":started["execution"],
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

struct CliFixture {
    actor: Arc<RuntimeActor>,
    stop: Arc<AtomicBool>,
    old_api: Arc<AtomicBool>,
    advance_ms: Arc<AtomicI64>,
    serving: Option<thread::JoinHandle<()>>,
    port: u16,
    root: PathBuf,
    script: PathBuf,
    input: PathBuf,
    child: PathBuf,
    task: String,
    policy: std::sync::Mutex<Option<Value>>,
}

fn http_request(
    stream: &mut TcpStream,
) -> (std::collections::HashMap<String, String>, Vec<String>) {
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
    let argv = std::str::from_utf8(&bytes[split..split + length])
        .unwrap()
        .split('\u{1f}')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    (headers, argv)
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
            shim_script("HERDR_TEST_PORT", "HERDR_TEST_HOOK", "test-only-orc"),
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
        let actor = Arc::new(start(authority, RuntimeBoot::Fresh { now_ms: now() }, 4));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let old_api = Arc::new(AtomicBool::new(false));
        let advance_ms = Arc::new(AtomicI64::new(0));
        let serving = {
            let actor = Arc::clone(&actor);
            let stop = Arc::clone(&stop);
            let old_api = Arc::clone(&old_api);
            let advance_ms = Arc::clone(&advance_ms);
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
                    let (headers, argv) = http_request(&mut stream);
                    let (status, body) = if headers.get("x-zerocode-hook-token").map(String::as_str)
                        != Some(HOOK)
                    {
                        (403, "test capability refused".to_string())
                    } else if old_api.load(Ordering::SeqCst)
                        && argv.first().is_some_and(|verb| verb.starts_with("gate-"))
                    {
                        (400, "unknown verb in old fixture API".to_string())
                    } else {
                        let verb = argv.first().cloned().unwrap_or_default();
                        let command = PlanCommand::checked(
                            argv,
                            headers["x-zerocode-team-id"].clone(),
                            headers["x-zerocode-team-pane"].clone(),
                            headers["x-zerocode-team-token"].clone(),
                            Some(format!("actor-v1:{}", "a".repeat(64))),
                            now() + advance_ms.load(Ordering::SeqCst),
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
            advance_ms,
            serving: Some(serving),
            port,
            root,
            script,
            input,
            child,
            task: String::new(),
            policy: std::sync::Mutex::new(None),
        };
        held.cli(&[
            "run-create",
            "--name",
            "isolated-gates",
            "--retry-request",
            "fixture-run",
        ]);
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
            .env("HERDR_TEST_PORT", self.port.to_string())
            .env("HERDR_TEST_HOOK", HOOK)
            .env(TEAM_ID_VAR, "team-1")
            .env(TEAM_PANE_VAR, "%1")
            .env(TEAM_TOKEN_VAR, capability_of("%1"));
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
        if action == "run" {
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
                .arg(child_test);
            self.environment(&mut prepare);
            let output = prepare.output().unwrap();
            assert!(
                output.status.success(),
                "fixture trust setup: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            *self.policy.lock().unwrap() = Some(serde_json::from_slice(&output.stdout).unwrap());
        }
        let policy = self.policy.lock().unwrap().clone().unwrap();
        let mut command = Command::new("python3");
        command
            .args(["-I", "-S", "-B"])
            .arg(self.script.with_file_name("gate_launcher.py"))
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
        if clean {
            command.arg("--clean-env");
        }
        if action == "run" {
            command.arg("--output").arg(self.root.join("out"));
        } else {
            command
                .arg("--metadata")
                .arg(self.root.join("out/metadata.json"))
                .arg("--log")
                .arg(self.root.join("out/gate.log"));
        }
        command.arg("--").args(
            policy["command"]
                .as_array()
                .unwrap()
                .iter()
                .map(|word| word.as_str().unwrap()),
        );
        self.environment(&mut command);
        command.output().unwrap()
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let log = std::fs::read_to_string(fixture.root.join("out/gate.log")).unwrap();
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
fn the_identical_fixture_probe_completes_and_hits_with_fixture_authority() {
    let authority = Fixture::new();
    let fixture = CliFixture::new(&authority);
    fixture.install_probe();
    let mut child = Command::new(&fixture.child);
    child.current_dir(&fixture.input).args([
        "--exact",
        "runtime_actor::tests::gate_reuse::receipt_gate_child_cannot_forge_fixture_completion",
        "--nocapture",
    ]);
    fixture.environment(&mut child);
    let output = child.output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("fixture probe began"),
        "positive control never ran"
    );
    assert!(
        text.contains("fixture forged completion"),
        "positive control did not complete"
    );
    assert!(
        text.contains("\"decision\":\"hit\""),
        "positive control did not reuse: {text}"
    );
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
    assert_eq!(
        phases,
        vec![
            json!("registered"),
            json!("started"),
            json!("completed"),
            json!("reuse")
        ]
    );
}

impl Drop for CliFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.serving.take().unwrap().join().unwrap();
    }
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
    assert_eq!(produced["tests"], 4);
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

    let inputs = metadata["inputs"].to_string();
    let registered = fixture.cli(&[
        "gate-register",
        "--task",
        &fixture.task,
        "--attempt",
        "none",
        "--body",
        &inputs,
        "--retry-request",
        "fixture-register-race",
    ]);
    let id = registered["registration"].as_str().unwrap();
    let lease = registered["lease"].as_str().unwrap();
    let start = |retry: &str| {
        vec![
            "gate-record".to_string(),
            "--task".into(),
            fixture.task.clone(),
            "--attempt".into(),
            "none".into(),
            "--start".into(),
            id.into(),
            "--lease".into(),
            lease.into(),
            "--body".into(),
            inputs.clone(),
            "--retry-request".into(),
            retry.into(),
        ]
    };
    let successes = thread::scope(|scope| {
        let first = start("fixture-race-one");
        let second = start("fixture-race-two");
        let fixture_ref = &fixture;
        let one = scope.spawn(move || {
            fixture_ref.raw_cli(&first.iter().map(String::as_str).collect::<Vec<_>>())
        });
        let two = scope.spawn(move || {
            fixture_ref.raw_cli(&second.iter().map(String::as_str).collect::<Vec<_>>())
        });
        [one.join().unwrap(), two.join().unwrap()]
            .iter()
            .filter(|output| output.status.success())
            .count()
    });
    assert_eq!(successes, 1, "one server-issued lease authorizes one start");

    let registered = fixture.cli(&[
        "gate-register",
        "--task",
        &fixture.task,
        "--attempt",
        "none",
        "--body",
        &inputs,
        "--retry-request",
        "fixture-register-expiry",
    ]);
    fixture
        .advance_ms
        .store(2 * 60 * 60 * 1000 + 1000, Ordering::SeqCst);
    let expired = fixture.raw_cli(&[
        "gate-record",
        "--task",
        &fixture.task,
        "--attempt",
        "none",
        "--start",
        registered["registration"].as_str().unwrap(),
        "--lease",
        registered["lease"].as_str().unwrap(),
        "--body",
        &inputs,
        "--retry-request",
        "fixture-expired-start",
    ]);
    assert!(!expired.status.success());
    fixture.old_api.store(true, Ordering::SeqCst);
    fixture.miss(true); // The same product shim against an old API causes a miss, never a hit.
}
