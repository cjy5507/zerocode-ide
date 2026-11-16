//! The harness profile's prompt set, tool set and finish rule.
//!
//! Each value is resolved the way the edit shape is (model row, name family,
//! serving lineup, default) and shown with its evidence by `--prompt-input`.
//! `core` · `core` given through the catalog overlay must send the very
//! request the experiment switches send (`ZO_ABLATE_PROMPT=all` +
//! `--allowed-tools bash,read_file,write_file,apply_patch`), so what an
//! experiment measures and what a profile ships are one request. The finish
//! rule closes `# Task work` with the two lines the experiment registered, and
//! leaves no trace when it is off.

#[expect(dead_code, reason = "the env guard is shared; its HTTP server and model reader are not used here")]
#[path = "support/custom_models.rs"]
mod fixture;
#[path = "support/profile_sources.rs"]
mod profile_sources;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use zo_ide::prompt_input::PromptInput;

/// The two lines the experiment registered, word for word.
const FINISH_RULE: [&str; 2] = [
    "Check your result once, with one focused check that covers the stated requirements.",
    "Once that check passes, finish and report. Repeat or broaden checking only when you changed something afterwards, a check failed, or a stated requirement is still unchecked.",
];

/// The model the experiment measured, and the core list it was measured with.
const MODEL: &str = "deepseek-flash";
const ARM_TOOLS: &str = "bash,read_file,write_file,apply_patch";
/// The experiment's prompt switch (`runtime::prompt::ablation`).
const ABLATION_ENV: &str = "ZO_ABLATE_PROMPT";

fn report(model: &str) -> PromptInput {
    PromptInput::measure(model, "/w", &["# Zo\nfixed".to_string()], &[], &[])
}

/// The value column of the profile row `label`, as `--prompt-input` prints it.
fn profile_value(rendered: &str, label: &str) -> Option<String> {
    rendered.lines().map(str::trim_start).find_map(|row| {
        let (head, value) = (row.get(..18)?, row.get(18..)?);
        (head.trim_end() == label).then(|| value.trim().to_string())
    })
}

#[test]
fn prompt_input_shows_each_new_value_with_its_evidence() {
    let _own = fixture::OwnProviders::new();
    for model in ["claude-opus-5-5", "gpt-6.1-sol", "gemini-3.6-flash", MODEL, "acme-coder-1"] {
        let rendered = report(model).render();
        profile_sources::assert_sources(&rendered);
        for (label, value) in [("Prompt set", "full (default)"), ("Tool set", "full (default)"), ("Finish rule", "off (default)")] {
            assert_eq!(profile_value(&rendered, label).as_deref(), Some(value), "{model}: {label}\n{rendered}");
        }
    }
    std::env::set_var(
        api::MODEL_CONTEXT_WINDOWS_ENV,
        json!({"models": [{"ids": [MODEL], "harness": {
            "prompt_set": "core", "prompt_set_source": "measured", "tool_set": "core", "finish_rule": "on"
        }}]})
        .to_string(),
    );
    let rendered = report(MODEL).render();
    profile_sources::assert_sources(&rendered);
    for (label, value) in [("Prompt set", "core (measured)"), ("Tool set", "core (declared)"), ("Finish rule", "on (declared)")] {
        assert_eq!(profile_value(&rendered, label).as_deref(), Some(value), "{label}\n{rendered}");
    }
}

/// A prompt built the way a headless session builds it, in `workspace`.
fn system_prompt(workspace: &Path) -> Vec<String> {
    runtime::load_system_prompt_for_main_with_mode(
        workspace,
        "2026-01-01",
        "test-os",
        "unknown",
        Some(MODEL),
        runtime::PromptMode::Autonomous,
    )
    .expect("system prompt builds")
}

fn task_work(sections: &[String]) -> Option<&String> {
    sections.iter().find(|section| section.starts_with("# Task work"))
}

#[test]
fn the_finish_rule_closes_task_work_when_on_and_leaves_no_trace_when_off() {
    let _own = fixture::OwnProviders::new();
    std::env::remove_var(ABLATION_ENV);
    let workspace = tempfile::tempdir().expect("workspace");
    let off = system_prompt(workspace.path());
    for line in FINISH_RULE {
        assert!(!off.iter().any(|section| section.contains(line)), "off must leave no trace of: {line}");
    }

    let overlay = |harness: Value| json!({"models": [{"ids": [MODEL], "harness": harness}]}).to_string();
    std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, overlay(json!({"finish_rule": "on"})));
    let on = system_prompt(workspace.path());
    let rule = FINISH_RULE.map(|line| format!("\n - {line}")).concat();
    let task = task_work(&on).expect("on keeps # Task work");
    assert!(task.ends_with(&rule), "the rule must close # Task work:\n{task}");
    // Nothing else moves: the same prompt, less the two lines, is the off one.
    let unruled: Vec<String> = on.iter().map(|section| section.replace(&rule, "")).collect();
    assert_eq!(unruled, off, "the finish rule must add its two lines and nothing else");

    // A core prompt holds `# Task work` out, but a declared rule is a value of
    // its own: it stays, under its heading, and the safety sections stay too.
    std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, overlay(json!({"finish_rule": "on", "prompt_set": "core"})));
    let core_on = system_prompt(workspace.path());
    assert_eq!(task_work(&core_on).map(String::as_str), Some(format!("# Task work{rule}").as_str()));
    std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, overlay(json!({"prompt_set": "core"})));
    let core_off = system_prompt(workspace.path());
    assert!(task_work(&core_off).is_none(), "a core prompt without the rule holds # Task work out");
    for kept in ["# Action scope and permission", "# Context Trust Label v1"] {
        for (arm, sections) in [("core", &core_off), ("core + rule", &core_on)] {
            assert!(sections.iter().any(|section| section.starts_with(kept)), "{arm} must keep {kept}");
        }
    }
    assert!(core_off.len() < off.len(), "core must hold the ablatable sections out");
}

/// A loopback chat-completions endpoint: every POST is answered with one short
/// streamed reply, every GET with an empty model list, and each request body
/// is kept.
struct ChatServer {
    base: String,
    bodies: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ChatServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback endpoint");
        listener.set_nonblocking(true).expect("nonblocking");
        let base = format!("http://{}/v1", listener.local_addr().expect("address"));
        let stop = Arc::new(AtomicBool::new(false));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let (stopped, kept) = (stop.clone(), bodies.clone());
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let request = read_request(&mut stream);
                let response = if request.starts_with("POST ") {
                    if let Some(body) = request.split_once("\r\n\r\n").and_then(|(_, body)| serde_json::from_str(body).ok()) {
                        kept.lock().expect("bodies").push(body);
                    }
                    let events = [
                        json!({"id": "c1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}, "finish_reason": null}]}),
                        json!({"id": "c1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                               "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
                    ];
                    let stream_body = events.iter().map(|event| format!("data: {event}\n\n")).collect::<String>() + "data: [DONE]\n\n";
                    format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{stream_body}", stream_body.len())
                } else {
                    let list = r#"{"data":[]}"#;
                    format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{list}", list.len())
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { base, bodies, stop, worker: Some(worker) }
    }

    /// The bodies kept since the last call, and forget them.
    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut *self.bodies.lock().expect("bodies"))
    }
}

impl Drop for ChatServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0; 16_384];
    loop {
        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&bytes);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                })
                .unwrap_or(0);
            if body.len() >= length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// One isolated home, workspace and session store, shared by the runs of a
/// scene so the only difference between them is what each run is given.
struct Scene {
    _root: tempfile::TempDir,
    cwd: PathBuf,
    home: PathBuf,
    store: PathBuf,
}

impl Scene {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("scene root");
        let (cwd, home, store) = (root.path().join("workspace"), root.path().join("home"), root.path().join("store"));
        for path in [&cwd, &home, &store.join("sessions")] {
            std::fs::create_dir_all(path).expect("scene folders");
        }
        std::fs::write(home.join("settings.json"), json!({"smart": {"autoClassifier": "off", "orchestration": "model"}}).to_string())
            .expect("settings");
        let cwd = cwd.canonicalize().expect("workspace path");
        Self { _root: root, cwd, home, store }
    }

    fn command(&self, binary: &Path) -> Command {
        let mut command = Command::new(binary);
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .current_dir(&self.cwd)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("ZO_CONFIG_HOME", &self.home)
            .env("ZO_SESSION_ROOT", &self.store)
            .env("ZO_STATE_DIR", self.store.join("state"))
            .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
            .env("ZO_DISABLE_KEYCHAIN", "1")
            .env("ZO_DISABLE_EXTERNAL_CREDENTIALS", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// The system text and the tools of the turn request one headless run sends.
    fn request(&self, server: &ChatServer, env: &[(&str, &str)], args: &[&str]) -> (Vec<Value>, Value) {
        let providers = json!([{"name": "Acme", "base_url": server.base, "requires_auth": false, "models": [MODEL]}]).to_string();
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_zo")));
        command.env(api::CUSTOM_PROVIDERS_ENV, providers).envs(env.iter().copied());
        command.args(["--plain", "--model", &format!("Acme/{MODEL}"), "--permission-mode", "workspace-write"]).args(args).arg("x");
        let output = finish(command);
        let told = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let turn = server
            .take()
            .into_iter()
            .find(|body| body.get("tools").is_some())
            .unwrap_or_else(|| panic!("no turn request reached the endpoint:\n{told}"));
        let system = turn["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter(|message| matches!(message["role"].as_str(), Some("system" | "developer")))
            .cloned()
            .collect();
        (system, turn["tools"].clone())
    }
}

/// Run `command` to its end, or stop it after two minutes: a run that hangs
/// fails here instead of holding the suite.
fn finish(mut command: Command) -> std::process::Output {
    let mut child = command.spawn().expect("zo starts");
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while child.try_wait().expect("zo state").is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().expect("zo output")
}

fn tool_names(tools: &Value) -> Vec<String> {
    let mut names: Vec<String> = tools
        .as_array()
        .expect("a tool list")
        .iter()
        .filter_map(|tool| tool.pointer("/function/name").or_else(|| tool.get("name")).and_then(Value::as_str).map(str::to_string))
        .collect();
    names.sort();
    names
}

/// What the experiment measured is what a profile ships: `prompt_set: core` +
/// `tool_set: core` declared in the overlay sends the system text and the tools
/// the experiment switches send, byte for byte.
#[test]
fn a_core_profile_sends_the_request_the_experiment_switches_send() {
    let server = ChatServer::new();
    let scene = Scene::new();
    let overlay = |harness: Value| json!({"models": [{"ids": [MODEL], "harness": harness}]}).to_string();
    let plain = overlay(json!({}));
    let core = overlay(json!({"prompt_set": "core", "tool_set": "core"}));

    let full = scene.request(&server, &[(api::MODEL_CONTEXT_WINDOWS_ENV, &plain)], &[]);
    let switches = scene.request(
        &server,
        &[(api::MODEL_CONTEXT_WINDOWS_ENV, &plain), (ABLATION_ENV, "all")],
        &["--allowed-tools", ARM_TOOLS],
    );
    let profile = scene.request(&server, &[(api::MODEL_CONTEXT_WINDOWS_ENV, &core)], &[]);

    assert_eq!(tool_names(&switches.1), ["Bash", "Read", "Write", "apply_patch"], "the switches' four tools");
    assert_ne!(full.0, switches.0, "the full system text must differ from the switches', or the scene shows nothing");
    assert_ne!(full.1, switches.1, "the full tool list must differ from the switches', or the scene shows nothing");
    assert_eq!(
        serde_json::to_string(&profile.0).expect("system"),
        serde_json::to_string(&switches.0).expect("system"),
        "a core prompt set must send the system text ZO_ABLATE_PROMPT=all sends"
    );
    assert_eq!(
        serde_json::to_string(&profile.1).expect("tools"),
        serde_json::to_string(&switches.1).expect("tools"),
        "a core tool set must send the tools --allowed-tools {ARM_TOOLS} sends"
    );
}

/// The `--prompt-input` digest of every shipped model row id, every shipped
/// alias and a crowd of names the catalog does not list, from this build and
/// from a baseline build (`ZO_BASELINE_BIN`, a `zo` built from the commit
/// before a harness change). A change whose defaults keep the harness passes
/// with every digest equal.
#[test]
#[ignore = "needs ZO_BASELINE_BIN: a zo built from the commit before the change"]
fn every_shipped_row_alias_and_unlisted_name_keeps_the_baseline_digest() {
    let baseline = PathBuf::from(std::env::var_os("ZO_BASELINE_BIN").expect("ZO_BASELINE_BIN names the baseline zo"));
    let catalog: Value = serde_json::from_str(include_str!("../../api/src/providers/model_context_windows.json")).expect("shipped catalog");
    let mut names: Vec<String> = catalog["models"]
        .as_array()
        .expect("model rows")
        .iter()
        .flat_map(|row| row["ids"].as_array().expect("ids").iter().filter_map(Value::as_str).map(str::to_string))
        .chain(catalog["aliases"].as_array().expect("aliases").iter().filter_map(|row| row["alias"].as_str()).map(str::to_string))
        .collect();
    let crowd = [MODEL, "acme-coder-1", "gpt-9.9-nova", "chatgpt-4o-latest", "local-gpt-helper", "local-codex-helper", "o1", "o3", "o4"];
    names.extend(crowd.map(str::to_string));
    let scene = Scene::new();
    let providers = json!([{"name": "Acme", "base_url": "http://127.0.0.1:9/v1", "requires_auth": false, "models": [MODEL, "acme-coder-1"]}]).to_string();
    let digest = |binary: &Path, name: &str| -> (Option<String>, String) {
        let mut command = scene.command(binary);
        command
            .env(api::CUSTOM_PROVIDERS_ENV, &providers)
            // Dummy keys open each lineup's client; the closed loopback port
            // keeps the token counter's request on this machine.
            .env("ANTHROPIC_API_KEY", "test-dummy")
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
            .env("OPENAI_API_KEY", "test-dummy")
            .env("GOOGLE_API_KEY", "test-dummy")
            .args(["--prompt-input", "--model", name]);
        let output = finish(command);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let digest = stdout.lines().find_map(|row| row.trim_start().strip_prefix("Digest").map(|value| value.trim().to_string()));
        (digest, format!("{stdout}\n{}", String::from_utf8_lossy(&output.stderr)))
    };
    let mut compared = 0;
    let mut lines = Vec::new();
    for name in &names {
        let (before, before_told) = digest(&baseline, name);
        let (after, after_told) = digest(Path::new(env!("CARGO_BIN_EXE_zo")), name);
        assert_eq!(before, after, "{name}: the digest moved\n-- baseline --\n{before_told}\n-- this build --\n{after_told}");
        if after.is_some() {
            compared += 1;
        }
        lines.push(format!("{name}\t{}", after.unwrap_or_else(|| "no digest (both builds)".to_string())));
    }
    println!("{}", lines.join("\n"));
    println!("compared {compared} of {} names", names.len());
    assert!(compared * 10 >= names.len() * 9, "too few names opened to compare: {compared} of {}", names.len());
}
