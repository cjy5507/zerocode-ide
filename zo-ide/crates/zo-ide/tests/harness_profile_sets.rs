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
//!
//! A tool set of `headless_core` is `core` for a run nobody attends and `full`
//! for every other session. Whatever the tool set, the system text names only
//! tools the session can reach: the four sections that teach a tool, the
//! deferred-tool paragraph and a background command's start line follow the
//! tools the session was given. A reopened session sends the text it stored
//! when it opened, and is given the tools its profile chose for that text: a
//! session a launch list opened keeps no list and reopens on every tool, as
//! 1.1.159 did.

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
/// What the light-surface arm (⑨) held out with that switch: the four sections
/// that each teach a tool the arm's list did not offer.
const TOOL_SECTIONS: &str =
    "delegation_and_workflow_routing,clarifying_questions,persistent_memory,skills_and_library_documentation";
/// Those four sections and the deferred-tool paragraph, by the heading each
/// opens with in the system text. A session that reaches none of their tools
/// is sent none of them.
const UNREACHED_TEXT: [&str; 5] = [
    "# Clarifying questions",
    "# Delegation and workflow routing",
    "# Skills and library documentation",
    "# Persistent memory",
    "# Deferred tools",
];
/// The line a background command starts with while the task tools reach it.
const TASK_TOOLS_LINE: &str = "Poll output with TaskOutput(task_id), stop with TaskStop.";

/// A catalog overlay that declares `harness` for the measured model.
fn overlay(harness: Value) -> String {
    let mut row = json!({"ids": [MODEL]});
    row["harness"] = harness;
    json!({"models": [row]}).to_string()
}

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
        Self::answering(None)
    }

    /// The endpoint, but a turn that offers tools and holds no tool result yet
    /// is answered with one `Bash` call of `arguments`.
    fn calling_bash(arguments: &Value) -> Self {
        Self::answering(Some(arguments.to_string()))
    }

    fn answering(bash_arguments: Option<String>) -> Self {
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
                    let body: Option<Value> = request.split_once("\r\n\r\n").and_then(|(_, body)| serde_json::from_str(body).ok());
                    let calls = bash_arguments
                        .as_deref()
                        .filter(|_| body.as_ref().is_some_and(|body| body.get("tools").is_some() && !holds_tool_result(body)));
                    if let Some(body) = body {
                        kept.lock().expect("bodies").push(body);
                    }
                    let events = calls.map_or_else(reply_events, bash_call_events);
                    let mut stream_body = String::new();
                    for event in &events {
                        stream_body.push_str("data: ");
                        stream_body.push_str(&event.to_string());
                        stream_body.push_str("\n\n");
                    }
                    stream_body.push_str("data: [DONE]\n\n");
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

fn holds_tool_result(body: &Value) -> bool {
    body["messages"].as_array().is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"))
}

/// One short streamed reply that ends the turn.
fn reply_events() -> Vec<Value> {
    vec![
        json!({"id": "c1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}, "finish_reason": null}]}),
        json!({"id": "c1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
    ]
}

/// One streamed `Bash` call with `arguments` (JSON text).
fn bash_call_events(arguments: &str) -> Vec<Value> {
    vec![
        json!({"id": "c2", "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [
            {"index": 0, "id": "call_bash", "type": "function", "function": {"name": "Bash", "arguments": arguments}}
        ]}, "finish_reason": null}]}),
        json!({"id": "c2", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
               "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
    ]
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

    /// Every request body one headless run sends, in order, and what it printed.
    fn bodies(&self, server: &ChatServer, env: &[(&str, &str)], args: &[&str]) -> (Vec<Value>, String) {
        let providers = json!([{"name": "Acme", "base_url": server.base, "requires_auth": false, "models": [MODEL]}]).to_string();
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_zo")));
        command.env(api::CUSTOM_PROVIDERS_ENV, providers).envs(env.iter().copied());
        command.args(["--plain", "--model", &format!("Acme/{MODEL}")]).args(args).arg("x");
        let output = finish(command);
        let told = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        (server.take(), told)
    }

    /// The system text and the tools of the turn request one headless run sends.
    fn request(&self, server: &ChatServer, env: &[(&str, &str)], args: &[&str]) -> (Vec<Value>, Value) {
        let (bodies, told) = self.bodies(server, env, &[&["--permission-mode", "workspace-write"][..], args].concat());
        let turn = bodies
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

    /// The text of the first tool result a headless run sends back, the run
    /// allowed every call so the tool runs.
    fn tool_result(&self, server: &ChatServer, env: &[(&str, &str)]) -> String {
        let (bodies, told) = self.bodies(server, env, &["--permission-mode", "danger-full-access"]);
        let message = bodies
            .iter()
            .filter_map(|body| body["messages"].as_array())
            .flatten()
            .find(|message| message["role"] == "tool")
            .unwrap_or_else(|| panic!("no tool result reached the endpoint:\n{told}"));
        content_text(&message["content"])
    }

    /// Write `skills/scene-skill/SKILL.md` under the scene's zo home: a skill
    /// the person installed.
    fn install_skill(&self) {
        let folder = self.home.join("skills").join("scene-skill");
        std::fs::create_dir_all(&folder).expect("skill folder");
        std::fs::write(folder.join("SKILL.md"), "---\nname: scene-skill\ndescription: A skill the person installed.\n---\nSay hello.\n")
            .expect("skill");
    }

    /// Add an MCP server to the scene's settings: one the person attached.
    /// Its command does not exist, and discovery runs behind the turn, so the
    /// run goes on without its tools.
    fn attach_mcp_server(&self) {
        let path = self.home.join("settings.json");
        let mut settings: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("settings")).expect("settings json");
        settings["mcpServers"] = json!({"scene-docs": {"command": self.home.join("no-such-mcp-server").display().to_string(), "args": []}});
        std::fs::write(&path, settings.to_string()).expect("settings");
    }
}

/// A message's text, whether its content is a string or a list of parts.
fn content_text(content: &Value) -> String {
    content.as_str().map_or_else(
        || content.as_array().into_iter().flatten().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n\n"),
        str::to_string,
    )
}

/// The system text of a request, its system messages in order.
fn system_text(system: &[Value]) -> String {
    system.iter().map(|message| content_text(&message["content"])).collect::<Vec<_>>().join("\n\n")
}

/// `text` less the section that opens with `heading`: from the blank line
/// before it to the next blank line, or to the end.
fn cut_section(text: &str, heading: &str) -> String {
    let start = text
        .find(&format!("\n\n{heading}\n"))
        .unwrap_or_else(|| panic!("no `{heading}` section in:\n{text}"));
    let body = start + 2;
    let end = text[body..].find("\n\n").map_or(text.len(), |offset| body + offset);
    format!("{}{}", &text[..start], &text[end..])
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

/// A run nobody attends, given no tool list, on a model whose profile says
/// `headless_core`, sends the request the light-surface arm measured: the
/// shell, `Read`, `Write` and the edit shape, and the full system text less the
/// four sections that teach a tool it does not reach and the deferred-tool
/// paragraph whose doors (`CapabilityInvoke`, `ToolSearch`) it does not have —
/// nothing else moves. The arm's own switches send that request byte for byte
/// on this build, and a launch list of the same four tools drops the same text.
#[test]
fn a_headless_core_run_sends_the_request_the_light_surface_arm_measured() {
    let server = ChatServer::new();
    let scene = Scene::new();
    let headless_core = overlay(json!({"tool_set": "headless_core"}));
    let full = overlay(json!({"tool_set": "full"}));
    let overlay_env = api::MODEL_CONTEXT_WINDOWS_ENV;

    let everything = scene.request(&server, &[(overlay_env, &full)], &[]);
    let unattended = scene.request(&server, &[(overlay_env, &headless_core)], &[]);
    let arm = scene.request(&server, &[(overlay_env, &full), (ABLATION_ENV, TOOL_SECTIONS)], &["--allowed-tools", ARM_TOOLS]);
    let listed = scene.request(&server, &[(overlay_env, &full)], &["--allowed-tools", ARM_TOOLS]);

    // A full tool set is every other session's request, as it was.
    let full_tools = tool_names(&everything.1);
    for tool in ["Agent", "AskUserQuestion", "Skill", "ToolSearch", "CapabilityInvoke"] {
        assert!(full_tools.iter().any(|name| name == tool), "a full run offers {tool}: {full_tools:?}");
    }
    let full_text = system_text(&everything.0);
    let expected = UNREACHED_TEXT.iter().fold(full_text, |text, heading| cut_section(&text, heading));

    assert_eq!(tool_names(&unattended.1), ["Bash", "Read", "Write", "apply_patch"], "headless_core offers the core four");
    assert_eq!(
        system_text(&unattended.0),
        expected,
        "headless_core sends the full text less the four sections and the deferred-tool paragraph"
    );
    assert_eq!(
        (serde_json::to_string(&unattended.0).expect("system"), serde_json::to_string(&unattended.1).expect("tools")),
        (serde_json::to_string(&arm.0).expect("system"), serde_json::to_string(&arm.1).expect("tools")),
        "headless_core sends the request the arm's switches send"
    );
    assert_eq!(system_text(&listed.0), expected, "a launch list of the four drops the same text and nothing else");
}

/// What `headless_core` leaves as it was: a launch list wins over it, and a
/// run the person attached an MCP server or a skill to keeps the full request.
#[test]
fn headless_core_leaves_a_launch_list_and_the_persons_attachments_as_they_were() {
    let server = ChatServer::new();
    let headless_core = overlay(json!({"tool_set": "headless_core"}));
    let full = overlay(json!({"tool_set": "full"}));
    let overlay_env = api::MODEL_CONTEXT_WINDOWS_ENV;

    let flagged = Scene::new().request(&server, &[(overlay_env, &headless_core)], &["--allowed-tools", "bash,read_file"]);
    assert_eq!(tool_names(&flagged.1), ["Bash", "Read"], "the launch list wins over the profile");

    for (attachment, attach) in [("an MCP server", Scene::attach_mcp_server as fn(&Scene)), ("a skill", Scene::install_skill)] {
        let scene = Scene::new();
        attach(&scene);
        let declared = scene.request(&server, &[(overlay_env, &headless_core)], &[]);
        let as_it_was = scene.request(&server, &[(overlay_env, &full)], &[]);
        assert!(tool_names(&declared.1).len() > 4, "with {attachment} attached the run keeps every tool");
        assert_eq!(
            (serde_json::to_string(&declared.0).expect("system"), serde_json::to_string(&declared.1).expect("tools")),
            (serde_json::to_string(&as_it_was.0).expect("system"), serde_json::to_string(&as_it_was.1).expect("tools")),
            "with {attachment} attached, headless_core sends the full request"
        );
    }
}

/// A background command started in a run that reaches no task tool is told
/// how its output does come back — when it exits — and how to stop it, and
/// names no tool the run was not given; a run with the task tools reads as
/// before.
#[test]
fn a_background_start_names_only_what_the_run_reaches() {
    let server = ChatServer::calling_bash(&json!({"command": "sleep 3", "run_in_background": true, "description": "wait"}));
    let overlay_env = api::MODEL_CONTEXT_WINDOWS_ENV;

    let core = Scene::new().tool_result(&server, &[(overlay_env, &overlay(json!({"tool_set": "headless_core"})))]);
    assert!(core.contains("Started background task"), "the command started in the background: {core}");
    for tool in ["TaskOutput", "TaskStop"] {
        assert!(!core.contains(tool), "a run without {tool} is not told to use it: {core}");
    }

    let full = Scene::new().tool_result(&server, &[(overlay_env, &overlay(json!({"tool_set": "full"})))]);
    assert!(full.contains(TASK_TOOLS_LINE), "a run with the task tools reads as before: {full}");
}

/// The request a run sends when it opens under `opened_on`, and the one it
/// sends when that conversation is reopened (`--continue`) under
/// `reopened_on`, in one scene.
fn opened_and_reopened(server: &ChatServer, opened_on: &str, reopened_on: &str) -> ((Vec<Value>, Value), (Vec<Value>, Value)) {
    let scene = Scene::new();
    let overlay_env = api::MODEL_CONTEXT_WINDOWS_ENV;
    let opened = scene.request(server, &[(overlay_env, opened_on)], &[]);
    let reopened = scene.request(server, &[(overlay_env, reopened_on)], &["--continue"]);
    (opened, reopened)
}

/// A reopened run sends the system text it stored when it opened, so it is
/// given the tools that text was built for. A run 1.1.158 opened on every tool
/// — every deepseek run nobody attends — and reopened where a new run would
/// get the core four (`headless_core`) keeps every tool, and its text teaches
/// no tool it lacks.
#[test]
fn a_run_opened_on_every_tool_reopens_on_every_tool() {
    let server = ChatServer::new();
    let (opened, reopened) = opened_and_reopened(&server, &overlay(json!({"tool_set": "full"})), &overlay(json!({"tool_set": "headless_core"})));

    let text = system_text(&reopened.0);
    assert!(text.starts_with(&system_text(&opened.0)), "a reopened run sends the text it stored:\n{text}");
    for heading in UNREACHED_TEXT {
        assert!(text.contains(heading), "the stored text teaches `{heading}`");
    }
    assert_eq!(tool_names(&reopened.1), tool_names(&opened.1), "a run opened on every tool reopens on every tool");
}

/// The other way: a run opened on the core four, reopened where a new run
/// would get every tool — a person reopening it at the terminal — keeps the
/// core four its stored text was built for.
#[test]
fn a_run_opened_on_the_core_four_reopens_on_them() {
    let server = ChatServer::new();
    let (opened, reopened) = opened_and_reopened(&server, &overlay(json!({"tool_set": "headless_core"})), &overlay(json!({"tool_set": "full"})));

    let text = system_text(&reopened.0);
    assert!(text.starts_with(&system_text(&opened.0)), "a reopened run sends the text it stored:\n{text}");
    for heading in UNREACHED_TEXT {
        assert!(!text.contains(heading), "the stored text was built without `{heading}`");
    }
    assert_eq!(tool_names(&reopened.1), ["Bash", "Read", "Write", "apply_patch"], "a run opened on the core four reopens on them");
}

/// Every file under `folder`, at any depth, whose name ends with `suffix`.
fn files_ending(folder: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for path in std::fs::read_dir(&folder).expect("a scene folder").map(|entry| entry.expect("a folder entry").path()) {
            if path.is_dir() {
                folders.push(path);
            } else if path.to_string_lossy().ends_with(suffix) {
                found.push(path);
            }
        }
    }
    found
}

/// A run a launch list opened (`--allowed-tools`, the way `zo review` opens
/// its run) stores no tool list beside its text: the list is the launch's to
/// name again. Reopened without one, it is given every tool, as 1.1.159 gave
/// it — even where a new run would get the core four (`headless_core`) — and
/// it sends the text it stored. Only a list the profile chose is stored.
#[test]
fn a_run_a_launch_list_opened_reopens_on_every_tool() {
    let server = ChatServer::new();
    let overlay_env = api::MODEL_CONTEXT_WINDOWS_ENV;
    let headless_core = overlay(json!({"tool_set": "headless_core"}));
    let stored_list = format!(".{}", runtime::session_control::SESSION_TOOLS_EXTENSION);

    let scene = Scene::new();
    let opened = scene.request(&server, &[(overlay_env, &headless_core)], &["--allowed-tools", "bash,read_file"]);
    let (texts, lists) = (files_ending(&scene.store, ".system-prompt.json"), files_ending(&scene.store, &stored_list));
    let reopened = scene.request(&server, &[(overlay_env, &headless_core)], &["--continue"]);
    let every_tool = Scene::new().request(&server, &[(overlay_env, &overlay(json!({"tool_set": "full"})))], &[]);
    let held = Scene::new();
    held.request(&server, &[(overlay_env, &headless_core)], &[]);

    assert_eq!(tool_names(&opened.1), ["Bash", "Read"], "the launch list opens the run");
    let text = system_text(&reopened.0);
    assert!(text.starts_with(&system_text(&opened.0)), "a reopened run sends the text it stored:\n{text}");
    assert_eq!(tool_names(&reopened.1), tool_names(&every_tool.1), "a run a launch list opened reopens on every tool");
    assert_eq!(texts.len(), 1, "the run stored its text: {texts:?}");
    assert_eq!(lists, Vec::<PathBuf>::new(), "a launch list is not stored beside the text");
    assert_eq!(files_ending(&held.store, &stored_list).len(), 1, "a list the profile chose is stored beside the text");
}

/// The `--prompt-input` digest of every shipped model row id, every shipped
/// alias and a crowd of names the catalog does not list, from this build and
/// from a baseline build (`ZO_BASELINE_BIN`, a `zo` built from the commit
/// before a harness change). A change whose defaults keep the harness passes
/// with every digest equal. A name whose shipped profile says `headless_core`
/// is, here, a run nobody attends with nothing attached (a piped
/// `--prompt-input` in a bare home), so its digest must move — and only those
/// names' may.
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
    let headless_core: Vec<bool> = {
        let _own = fixture::OwnProviders::new();
        names.iter().map(|name| api::harness_profile_for_model(name).tool_set == api::ToolSet::HeadlessCore).collect()
    };
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
            .env("XAI_API_KEY", "test-dummy")
            .env("DEEPSEEK_API_KEY", "test-dummy")
            .env("OLLAMA_BASE_URL", "http://127.0.0.1:9")
            .args(["--prompt-input", "--model", name]);
        let output = finish(command);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let digest = stdout.lines().find_map(|row| row.trim_start().strip_prefix("Digest").map(|value| value.trim().to_string()));
        (digest, format!("{stdout}\n{}", String::from_utf8_lossy(&output.stderr)))
    };
    let (mut compared, mut moved) = (0, 0);
    let mut lines = Vec::new();
    for (name, moves) in names.iter().zip(headless_core) {
        let (before, before_told) = digest(&baseline, name);
        let (after, after_told) = digest(Path::new(env!("CARGO_BIN_EXE_zo")), name);
        if moves && after.is_some() {
            assert_ne!(before, after, "{name}: headless_core must send the core request\n-- baseline --\n{before_told}\n-- this build --\n{after_told}");
            moved += 1;
        } else {
            assert_eq!(before, after, "{name}: the digest moved\n-- baseline --\n{before_told}\n-- this build --\n{after_told}");
        }
        if after.is_some() {
            compared += 1;
        }
        let mark = if moves { "\tmoved (headless_core)" } else { "" };
        lines.push(format!("{name}\t{}{mark}", after.unwrap_or_else(|| "no digest (both builds)".to_string())));
    }
    println!("{}", lines.join("\n"));
    println!("compared {compared} of {} names, {moved} moved by headless_core", names.len());
    assert!(compared * 4 >= names.len() * 3, "too few names opened to compare: {compared} of {}", names.len());
}
