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
//! Response Style Contract's line that sends the model to `Skill` before it
//! writes a page, the deferred-tool paragraph and a background command's start
//! line follow the tools the session was given. A reopened session sends the
//! text it stored when it opened. A run nobody attends that reopens it is
//! given the tools its profile chose for that text; a session a launch list
//! opened keeps no list and reopens on every tool, as 1.1.159 did. A person
//! who reopens a session gets what the run decides then — every run here is
//! one nobody attends, so `plain_session`'s own table shows that side.

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
/// The section that teaches the tool only a person answers (`AskUserQuestion`).
/// A run nobody attends is given neither, whatever its tool set (t-120813).
const PERSON_ONLY_TEXT: &str = "# Clarifying questions";
/// The words of the Response Style Contract's line that sends the model to
/// `Skill` before it writes a page. A session that reaches no skill tool is not
/// sent that line.
const SKILL_LINE_WORDS: &str = "read artifact-design with Skill";
/// The line a background command starts with while the task tools reach it.
const TASK_TOOLS_LINE: &str = "Poll output with TaskOutput(task_id), stop with TaskStop.";
/// The experiment switch of the headless core profile: `core` is its one value.
const CORE_SWITCH: &str = "ZO_EXPERIMENT_HEADLESS_PROFILE";
/// The model the core profile's written bytes name: a model the catalog does
/// not list, at a person's own endpoint — it edits with `apply_patch` and its
/// identity names no maker, as the measured model's does.
const CORE_MODEL: &str = "acme-coder-1";
/// The core profile as its design wrote it, byte for byte: the system text
/// before its `# Environment` section (with this model's name where the
/// measured one stood) and the six tools.
const CORE_PROMPT_STATIC: &str = include_str!("fixtures/headless-core/prompt-static.txt");
const CORE_TOOLS: &str = include_str!("fixtures/headless-core/tools-apply-patch.json");

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
        // The measured model's family ships `headless_core`, from its measurement.
        let tool_set = if model == MODEL { "headless_core (measured)" } else { "full (default)" };
        for (label, value) in [("Prompt set", "full (default)"), ("Tool set", tool_set), ("Finish rule", "off (default)")] {
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
/// is kept as it arrived.
struct ChatServer {
    base: String,
    bodies: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ChatServer {
    fn new() -> Self {
        Self::answering(Vec::new(), false)
    }

    /// The endpoint, but a turn that offers tools and holds no tool result yet
    /// is answered with one `Bash` call of `arguments`.
    fn calling_bash(arguments: &Value) -> Self {
        Self::answering(vec![("Bash".to_string(), arguments.to_string())], false)
    }

    /// The endpoint, but a turn request that offers tools and holds `k` tool
    /// results is answered with the `k`-th of `calls` (a tool's name and its
    /// arguments), and once they are spent with the short reply. With
    /// `reasoned`, every answer streams a line of reasoning first, so the
    /// requests after it carry that reasoning back.
    fn calling(calls: &[(&str, Value)], reasoned: bool) -> Self {
        Self::answering(calls.iter().map(|(name, arguments)| ((*name).to_string(), arguments.to_string())).collect(), reasoned)
    }

    fn answering(calls: Vec<(String, String)>, reasoned: bool) -> Self {
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
                    let raw = request.split_once("\r\n\r\n").map(|(_, body)| body.to_string());
                    let body: Option<Value> = raw.as_deref().and_then(|body| serde_json::from_str(body).ok());
                    let done = body.as_ref().filter(|body| body.get("tools").is_some()).map(tool_results);
                    let call = done.and_then(|done| calls.get(done).map(|call| (done, call)));
                    let reasoning = reasoned.then(|| format!("Step {} reasoning.", done.unwrap_or_default()));
                    if let (Some(raw), Some(_)) = (raw, body) {
                        kept.lock().expect("bodies").push(raw);
                    }
                    let events = call.map_or_else(
                        || reply_events(reasoning.as_deref()),
                        |(index, (name, arguments))| call_events(index, name, arguments, reasoning.as_deref()),
                    );
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

    /// The bodies kept since the last call as they arrived, byte for byte, and
    /// forget them.
    fn take_raw(&self) -> Vec<String> {
        std::mem::take(&mut *self.bodies.lock().expect("bodies"))
    }

    fn parsed(raw: &[String]) -> Vec<Value> {
        raw.iter().map(|body| serde_json::from_str(body).expect("a kept body is JSON")).collect()
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

/// How many tool results a request carries: the step of its turn it is.
fn tool_results(body: &Value) -> usize {
    body["messages"].as_array().map_or(0, |messages| messages.iter().filter(|message| message["role"] == "tool").count())
}

/// The reasoning an answer streams before its text or call, as a delta of its own.
fn reasoning_event(id: &str, reasoning: Option<&str>) -> Option<Value> {
    reasoning.map(|text| json!({"id": id, "choices": [{"index": 0, "delta": {"reasoning_content": text}, "finish_reason": null}]}))
}

/// One short streamed reply that ends the turn.
fn reply_events(reasoning: Option<&str>) -> Vec<Value> {
    reasoning_event("c1", reasoning)
        .into_iter()
        .chain([
            json!({"id": "c1", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}, "finish_reason": null}]}),
            json!({"id": "c1", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
        ])
        .collect()
}

/// One streamed call of the tool `name` with `arguments` (JSON text), the
/// turn's `step`-th.
fn call_events(step: usize, name: &str, arguments: &str, reasoning: Option<&str>) -> Vec<Value> {
    reasoning_event("c2", reasoning)
        .into_iter()
        .chain([
            json!({"id": "c2", "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [
                {"index": 0, "id": format!("call_{step}"), "type": "function", "function": {"name": name, "arguments": arguments}}
            ]}, "finish_reason": null}]}),
            json!({"id": "c2", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}),
        ])
        .collect()
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
        Self::lay_out(&cwd, &home, &store);
        let cwd = cwd.canonicalize().expect("workspace path");
        Self { _root: root, cwd, home, store }
    }

    /// The scene's folders as a new scene has them: empty, and the settings
    /// every run of the scene reads.
    fn lay_out(cwd: &Path, home: &Path, store: &Path) {
        for path in [cwd, home, &store.join("sessions")] {
            std::fs::create_dir_all(path).expect("scene folders");
        }
        std::fs::write(home.join("settings.json"), json!({"smart": {"autoClassifier": "off", "orchestration": "model"}}).to_string())
            .expect("settings");
    }

    /// Put the scene back as new, at the same paths: what one run left
    /// (sessions, state, files) is gone before the next.
    fn reset(&self) {
        for path in [&self.cwd, &self.home, &self.store] {
            std::fs::remove_dir_all(path).expect("a scene folder is removed");
        }
        Self::lay_out(&self.cwd, &self.home, &self.store);
    }

    /// Merge `settings` into the scene's settings file.
    fn add_settings(&self, settings: &Value) {
        let path = self.home.join("settings.json");
        let mut merged: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("settings")).expect("settings json");
        for (key, value) in settings.as_object().expect("settings object") {
            merged[key] = value.clone();
        }
        std::fs::write(&path, merged.to_string()).expect("settings");
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

    /// One headless run of `binary` on the scene's endpoint, serving `model`:
    /// every request body it sends as it arrived, in order, and what it printed.
    fn run(&self, binary: &Path, server: &ChatServer, model: &str, env: &[(&str, &str)], args: &[&str]) -> Run {
        let providers = json!([{"name": "Acme", "base_url": server.base, "requires_auth": false, "models": [model]}]).to_string();
        let mut command = self.command(binary);
        command.env(api::CUSTOM_PROVIDERS_ENV, providers).envs(env.iter().copied());
        command.args(["--plain", "--model", &format!("Acme/{model}")]).args(args).arg("x");
        let output = finish(command);
        Run {
            bodies: server.take_raw(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Every request body one headless run sends, in order, and what it printed.
    fn bodies(&self, server: &ChatServer, env: &[(&str, &str)], args: &[&str]) -> (Vec<Value>, String) {
        let run = self.run(Path::new(env!("CARGO_BIN_EXE_zo")), server, MODEL, env, args);
        let told = run.told();
        (ChatServer::parsed(&run.bodies), told)
    }

    /// The system text and the tools of the turn request one headless run sends.
    fn request(&self, server: &ChatServer, env: &[(&str, &str)], args: &[&str]) -> (Vec<Value>, Value) {
        let (bodies, told) = self.bodies(server, env, &[&["--permission-mode", "workspace-write"][..], args].concat());
        turn_of(bodies).unwrap_or_else(|| panic!("no turn request reached the endpoint:\n{told}"))
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

/// What one headless run sent and printed ([`Scene::run`]).
struct Run {
    /// Every request body, byte for byte as it arrived.
    bodies: Vec<String>,
    stdout: String,
    stderr: String,
}

impl Run {
    /// The run's stdout and stderr, for a failure message.
    fn told(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }

    /// The system messages and the tools of the run's first turn request.
    fn turn(&self) -> (Vec<Value>, Value) {
        turn_of(ChatServer::parsed(&self.bodies))
            .unwrap_or_else(|| panic!("no turn request reached the endpoint:\n{}", self.told()))
    }

    /// The lines the run printed to stderr that name `words`.
    fn stderr_naming(&self, words: &str) -> Vec<&str> {
        self.stderr.lines().filter(|line| line.contains(words)).collect()
    }
}

/// The system messages and the tools of the first turn request among
/// `bodies` (the first that offers tools).
fn turn_of(bodies: Vec<Value>) -> Option<(Vec<Value>, Value)> {
    let turn = bodies.into_iter().find(|body| body.get("tools").is_some())?;
    let system = turn["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|message| matches!(message["role"].as_str(), Some("system" | "developer")))
        .cloned()
        .collect();
    Some((system, turn["tools"].clone()))
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
    let mut names = tool_order(tools);
    names.sort();
    names
}

/// The tools' names in the order the request lists them.
fn tool_order(tools: &Value) -> Vec<String> {
    tools
        .as_array()
        .expect("a tool list")
        .iter()
        .filter_map(|tool| tool.pointer("/function/name").or_else(|| tool.get("name")).and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// What the experiment measured is what a profile ships: `prompt_set: core` +
/// `tool_set: core` declared in the overlay sends the system text and the tools
/// the experiment switches send, byte for byte.
#[test]
fn a_core_profile_sends_the_request_the_experiment_switches_send() {
    let server = ChatServer::new();
    let scene = Scene::new();
    // Full said out loud: the measured model's family ships `headless_core`.
    let plain = overlay(json!({"tool_set": "full"}));
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

/// The one line of `text` that holds `words`, with the line break before it.
fn line_holding(text: &str, words: &str) -> Option<String> {
    text.lines().find(|line| line.contains(words)).map(|line| format!("\n{line}"))
}

/// A run nobody attends, given no tool list, on a model whose profile says
/// `headless_core`, sends the request the light-surface arm measured: the
/// shell, `Read`, `Write` and the edit shape, and the full system text less the
/// four sections that teach a tool it does not reach, the Response Style
/// Contract's line that sends the model to `Skill`, and the deferred-tool
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

    // A full tool set is every other session's request, as it was — less the
    // tool only a person answers, which no run nobody attends is given.
    let full_tools = tool_names(&everything.1);
    for tool in ["Agent", "Skill", "ToolSearch", "CapabilityInvoke"] {
        assert!(full_tools.iter().any(|name| name == tool), "a full run offers {tool}: {full_tools:?}");
    }
    assert!(!full_tools.iter().any(|name| name == "AskUserQuestion"), "nobody attends this run: {full_tools:?}");
    let full_text = system_text(&everything.0);
    assert!(!full_text.contains(PERSON_ONLY_TEXT), "nor is it taught to ask:\n{full_text}");
    let skill_line = line_holding(&full_text, SKILL_LINE_WORDS).expect("a full run is sent to Skill before it writes a page");
    assert_eq!(full_text.matches(&skill_line).count(), 1, "a full run is sent that line once");
    let unskilled = full_text.replacen(&skill_line, "", 1);
    let expected = UNREACHED_TEXT
        .iter()
        .filter(|heading| **heading != PERSON_ONLY_TEXT)
        .fold(unskilled, |text, heading| cut_section(&text, heading));

    assert_eq!(tool_names(&unattended.1), ["Bash", "Read", "Write", "apply_patch"], "headless_core offers the core four");
    assert_eq!(
        line_holding(&system_text(&unattended.0), SKILL_LINE_WORDS),
        None,
        "a run that reaches no skill tool is not sent to Skill before it writes a page"
    );
    assert_eq!(
        system_text(&unattended.0),
        expected,
        "headless_core sends the full text less the four sections, the Skill line and the deferred-tool paragraph"
    );
    assert_eq!(
        (serde_json::to_string(&unattended.0).expect("system"), serde_json::to_string(&unattended.1).expect("tools")),
        (serde_json::to_string(&arm.0).expect("system"), serde_json::to_string(&arm.1).expect("tools")),
        "headless_core sends the request the arm's switches send"
    );
    assert_eq!(system_text(&listed.0), expected, "a launch list of the four drops the same text and nothing else");
}

/// A run nobody attends, on every tool, is sent only what it can use: not the
/// tool only a person answers (its question would come back dismissed) nor
/// the section that teaches it, and — reaching no ZeroCode ledger — the two
/// lines of the delegation rubric its `Agent` still needs instead of the
/// rubric's ledger workers, `/goal` and mid-run requirements. `Agent`, the
/// skill tool and the deferred doors stay (t-120813).
#[test]
fn a_run_nobody_attends_is_sent_only_what_it_can_use() {
    let server = ChatServer::new();
    let full = overlay(json!({"tool_set": "full"}));
    let (system, tools) = Scene::new().request(&server, &[(api::MODEL_CONTEXT_WINDOWS_ENV, &full)], &[]);

    let names = tool_names(&tools);
    assert!(!names.iter().any(|name| name == "AskUserQuestion"), "no person answers this run: {names:?}");
    for kept in ["Agent", "Skill", "ToolSearch", "CapabilityInvoke"] {
        assert!(names.iter().any(|name| name == kept), "{kept} stays: {names:?}");
    }
    let text = system_text(&system);
    assert!(!text.contains(PERSON_ONLY_TEXT), "nor is it taught to ask:\n{text}");
    let rubric = text
        .split("\n\n")
        .find(|section| section.starts_with("# Delegation and workflow routing"))
        .unwrap_or_else(|| panic!("Agent is reached, so a rubric is sent:\n{text}"));
    assert!(rubric.contains("Work SOLO for common renames"), "the bar for working alone stays:\n{rubric}");
    for gone in ["zerocode-orc", "`/goal`", "adds a requirement while work is already in flight"] {
        assert!(!rubric.contains(gone), "a run without the ledger or a person is not told of {gone:?}:\n{rubric}");
    }
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
        // Every tool but the one only a person answers, which no run nobody
        // attends is given (t-120813).
        assert_eq!(text.contains(heading), heading != PERSON_ONLY_TEXT, "the stored text and `{heading}`");
    }
    assert_eq!(tool_names(&reopened.1), tool_names(&opened.1), "a run opened on every tool reopens on every tool");
}

/// The other way: a run opened on the core four, reopened by a run nobody
/// attends where a new run would get every tool (`tool_set: full`), keeps the
/// core four its stored text was built for. A person who reopens it gets what
/// the run decides then (`plain_session`'s table).
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
/// names' may. A change that means to move other names, or none, names them in
/// `ZO_BASELINE_MOVED`, comma-separated (empty: none moves): a change that
/// leaves the core request alone keeps those digests against a baseline that
/// already sends it.
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
    let expected_to_move: Option<Vec<String>> = std::env::var("ZO_BASELINE_MOVED")
        .ok()
        .map(|list| list.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect());
    let why = if expected_to_move.is_some() { "named in ZO_BASELINE_MOVED" } else { "headless_core" };
    let (mut compared, mut moved) = (0, 0);
    let mut lines = Vec::new();
    for (name, headless_core) in names.iter().zip(headless_core) {
        let must_move = expected_to_move.as_ref().map_or(headless_core, |listed| listed.contains(name));
        let (before, before_told) = digest(&baseline, name);
        let (after, after_told) = digest(Path::new(env!("CARGO_BIN_EXE_zo")), name);
        if must_move && after.is_some() {
            assert_ne!(before, after, "{name}: the change must move this digest ({why})\n-- baseline --\n{before_told}\n-- this build --\n{after_told}");
            moved += 1;
        } else {
            assert_eq!(before, after, "{name}: the digest moved\n-- baseline --\n{before_told}\n-- this build --\n{after_told}");
        }
        if after.is_some() {
            compared += 1;
        }
        let mark = if must_move { format!("\tmoved ({why})") } else { String::new() };
        lines.push(format!("{name}\t{}{mark}", after.unwrap_or_else(|| "no digest (both builds)".to_string())));
    }
    println!("{}", lines.join("\n"));
    println!("compared {compared} of {} names, {moved} moved ({why})", names.len());
    assert!(compared * 4 >= names.len() * 3, "too few names opened to compare: {compared} of {}", names.len());
}

/// A run of this build in `scene`: `--permission-mode workspace-write`, then
/// `args`.
fn core_scene_run(scene: &Scene, server: &ChatServer, env: &[(&str, &str)], args: &[&str]) -> Run {
    let args = [&["--permission-mode", "workspace-write"][..], args].concat();
    scene.run(Path::new(env!("CARGO_BIN_EXE_zo")), server, CORE_MODEL, env, &args)
}

/// The value of the environment bullet `label` (`Working directory: `) as the
/// full text writes it (` - Working directory: /w`).
fn environment_value<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(" - ").and_then(|rest| rest.strip_prefix(label)))
        .unwrap_or_else(|| panic!("no `{label}` bullet in:\n{text}"))
}

/// The fixed head of `run`'s first turn request: its first system message —
/// the prompt the session built or stored — and its tools, as the wire
/// carries them. A reopened run's later system messages are its transcript's.
fn prefix_of(run: &Run) -> String {
    let (system, tools) = run.turn();
    let first = system.first().map(|message| content_text(&message["content"]));
    serde_json::to_string(&(first, tools)).expect("a prefix")
}

/// The text of the first tool result among `run`'s requests.
fn first_tool_result(run: &Run) -> String {
    ChatServer::parsed(&run.bodies)
        .iter()
        .filter_map(|body| body["messages"].as_array())
        .flatten()
        .find(|message| message["role"] == "tool")
        .map_or_else(|| panic!("no tool result reached the endpoint:\n{}", run.told()), |message| content_text(&message["content"]))
}

/// E2 of the headless core profile: a run nobody attends and no window holds,
/// given `ZO_EXPERIMENT_HEADLESS_PROFILE=core`, sends the profile its design
/// wrote — the short system text byte for byte, then one `# Environment`
/// section with the working directory, the date, the platform and the sandbox
/// line the full text carries (less its model line), and six tools: the shell,
/// `Read`, `Write`, the model's edit shape and the two that reach a background
/// task. It says so once on stderr; a run without the switch says nothing.
#[test]
fn the_core_switch_sends_the_written_core_text_and_six_tools() {
    let server = ChatServer::new();
    let scene = Scene::new();
    let full = core_scene_run(&scene, &server, &[], &[]);
    let core = core_scene_run(&scene, &server, &[(CORE_SWITCH, "core")], &[]);

    let (core_system, core_tools) = core.turn();
    assert_eq!(
        tool_order(&core_tools),
        ["Bash", "Read", "Write", "apply_patch", "TaskStop", "TaskOutput"],
        "the core run's six tools, in the registry's order:\n{}",
        core.told()
    );
    let written: Value = serde_json::from_str(CORE_TOOLS).expect("the written tools");
    assert_eq!(core_tools, written, "the six tools as the design wrote them");

    let text = system_text(&core_system);
    let full_text = system_text(&full.turn().0);
    let environment = text
        .strip_prefix(CORE_PROMPT_STATIC)
        .unwrap_or_else(|| panic!("the core text opens with the written text, byte for byte:\n{text}"));
    let expected = ["Working directory: ", "Date: ", "Platform: ", "Sandbox: "]
        .iter()
        .fold("# Environment".to_string(), |section, label| {
            format!("{section}\n- {label}{}", environment_value(&full_text, label))
        });
    assert_eq!(environment, expected, "one environment section: the full text's lines, less its model line");
    assert_eq!(core.stderr_naming(CORE_SWITCH).len(), 1, "the arm says so once:\n{}", core.stderr);
    assert!(full.stderr_naming(CORE_SWITCH).is_empty(), "a run without the switch says nothing of it:\n{}", full.stderr);
}

/// E4 of the headless core profile: a value the switch does not know leaves
/// every request as it was, byte for byte, and is named once on stderr.
#[test]
fn a_core_switch_value_zo_does_not_know_leaves_every_request_as_it_was() {
    let server = ChatServer::new();
    let scene = Scene::new();
    let off = core_scene_run(&scene, &server, &[], &[]);
    assert!(!off.bodies.is_empty(), "the run reached the endpoint:\n{}", off.told());
    for value in ["foo", "all"] {
        scene.reset();
        let unknown = core_scene_run(&scene, &server, &[(CORE_SWITCH, value)], &[]);
        assert_eq!(unknown.bodies, off.bodies, "{CORE_SWITCH}={value} leaves every request as it was");
        let said = unknown.stderr_naming(CORE_SWITCH);
        assert!(said.len() == 1 && said[0].contains(value), "{CORE_SWITCH}={value} is named once:\n{}", unknown.stderr);
    }
}

/// One run of E5 as a scene stages it: what the person or the window brings,
/// how the scene gets it, and the run's environment and arguments.
struct Staged<'a> {
    what: &'a str,
    stage: &'a dyn Fn(&Scene),
    env: Vec<(&'a str, &'a str)>,
    args: &'a [&'a str],
}

/// E5 of the headless core profile, the part a scene can stage: a run given the
/// switch that a person or a window may be part of — a launch tool list, an MCP
/// server or a skill the person attached, a hook, an output style, a pane, an
/// event channel's address file, a second brain — sends what it sends without
/// the switch, byte for byte, and says once on stderr why the switch was not
/// read. (An attached MCP server's discovery runs behind the turn, so that run
/// is held to its turn request's text and tools.)
#[test]
fn the_core_switch_leaves_a_run_a_person_or_a_window_may_be_part_of_as_it_was() {
    let server = ChatServer::new();
    let outside = tempfile::tempdir().expect("a folder outside the scene");
    let vault = outside.path().display().to_string();
    let addr_file = outside.path().join("events.addr").display().to_string();
    let hook = |scene: &Scene| scene.add_settings(&json!({"hooks": {"PreToolUse": ["true"]}}));
    let style = |scene: &Scene| scene.add_settings(&json!({"outputStyle": "Explanatory"}));
    let nothing = |_: &Scene| {};
    let cases = [
        Staged { what: "a launch tool list", stage: &nothing, env: vec![], args: &["--allowed-tools", "bash,read_file"] },
        Staged { what: "an MCP server", stage: &Scene::attach_mcp_server, env: vec![], args: &[] },
        Staged { what: "a skill", stage: &Scene::install_skill, env: vec![], args: &[] },
        Staged { what: "a settings hook", stage: &hook, env: vec![], args: &[] },
        Staged { what: "an output style", stage: &style, env: vec![], args: &[] },
        Staged { what: "a pane", stage: &nothing, env: vec![("ZEROCODE_PANE_KEY", "e5")], args: &[] },
        Staged { what: "an event channel's address file", stage: &nothing, env: vec![("ZO_EVENTS_ADDR_FILE", addr_file.as_str())], args: &[] },
        Staged { what: "a second brain", stage: &nothing, env: vec![("ZEROCODE_SECOND_BRAIN", vault.as_str())], args: &[] },
    ];
    for Staged { what, stage, env, args } in cases {
        let scene = Scene::new();
        stage(&scene);
        let off = core_scene_run(&scene, &server, &env, args);
        scene.reset();
        stage(&scene);
        let switched: Vec<(&str, &str)> = env.iter().copied().chain([(CORE_SWITCH, "core")]).collect();
        let on = core_scene_run(&scene, &server, &switched, args);
        assert_eq!(
            serde_json::to_string(&on.turn()).expect("turn"),
            serde_json::to_string(&off.turn()).expect("turn"),
            "{what}: the switch leaves the turn request's text and tools as they were"
        );
        if what != "an MCP server" {
            assert_eq!(on.bodies, off.bodies, "{what}: the switch leaves every request as it was");
        }
        assert_eq!(on.stderr_naming(CORE_SWITCH).len(), 1, "{what}: one line says why the switch was not read:\n{}", on.stderr);
        assert!(off.stderr_naming(CORE_SWITCH).is_empty(), "{what}: a run without the switch says nothing of it:\n{}", off.stderr);
    }
}

/// The reopen side of the core profile. A core run reopened by a run nobody
/// attends, without the switch, sends the text and the six tools it opened on,
/// byte for byte (E5-R2). A run opened without the switch and reopened with it
/// sends what that reopen sends without it, and says once that the switch was
/// not read (E5-12): a reopened run keeps what it stored.
#[test]
fn a_core_run_reopens_on_what_it_opened_on_and_a_reopen_reads_no_switch() {
    let server = ChatServer::new();
    let scene = Scene::new();
    let opened = core_scene_run(&scene, &server, &[(CORE_SWITCH, "core")], &[]);
    let reopened = core_scene_run(&scene, &server, &[], &["--continue"]);
    assert_eq!(tool_order(&opened.turn().1).len(), 6, "the run opened on the core profile:\n{}", opened.told());
    assert_eq!(
        prefix_of(&reopened),
        prefix_of(&opened),
        "a reopened core run sends the text and the tools it opened on"
    );
    assert!(reopened.stderr_naming(CORE_SWITCH).is_empty(), "nothing to say without the switch:\n{}", reopened.stderr);

    let scene = Scene::new();
    core_scene_run(&scene, &server, &[], &[]);
    let plain = core_scene_run(&scene, &server, &[], &["--continue"]);
    let switched = core_scene_run(&scene, &server, &[(CORE_SWITCH, "core")], &["--continue"]);
    assert_eq!(prefix_of(&switched), prefix_of(&plain), "the switch leaves a reopened run as it was");
    assert_eq!(switched.stderr_naming(CORE_SWITCH).len(), 1, "one line says why:\n{}", switched.stderr);
}

/// E6 of the headless core profile: a background command started in a core run
/// is told to read and stop it with the two task tools the run is given.
#[test]
fn a_core_run_reaches_a_background_command_through_the_task_tools() {
    let server = ChatServer::calling_bash(&json!({"command": "sleep 3", "run_in_background": true}));
    let scene = Scene::new();
    let run = scene.run(
        Path::new(env!("CARGO_BIN_EXE_zo")),
        &server,
        CORE_MODEL,
        &[(CORE_SWITCH, "core")],
        &["--permission-mode", "danger-full-access"],
    );
    let result = first_tool_result(&run);
    assert!(result.contains("Started background task") && result.contains(TASK_TOOLS_LINE), "{result}");
}

/// E7 of the headless core profile, the shell's half: the arguments the core
/// shell does not show are not read — a call that carries one, even of a type
/// the full shell refuses, runs as its shown arguments say.
#[test]
fn a_core_run_reads_no_shell_argument_it_does_not_show() {
    let hidden = json!({"command": "echo shown-only", "description": 7, "dangerouslyDisableSandbox": "yes"});
    let args = ["--permission-mode", "danger-full-access"];
    let zo = Path::new(env!("CARGO_BIN_EXE_zo"));
    let core = Scene::new().run(zo, &ChatServer::calling_bash(&hidden), CORE_MODEL, &[(CORE_SWITCH, "core")], &args);
    let full = Scene::new().run(zo, &ChatServer::calling_bash(&hidden), CORE_MODEL, &[], &args);
    let (core_result, full_result) = (first_tool_result(&core), first_tool_result(&full));
    assert!(core_result.lines().any(|line| line.trim() == "shown-only"), "the core shell ran the command:\n{core_result}");
    assert!(!full_result.lines().any(|line| line.trim() == "shown-only"), "the full shell reads what it shows:\n{full_result}");
}

/// E1 of the headless core profile: without the switch, this build sends what
/// the baseline build sends, byte for byte — a run's first request, and every
/// request of a five-step run (four shell calls and an answer, each streamed
/// after a line of reasoning, so the later requests carry that reasoning
/// back). `ZO_BASELINE_BIN` names a `zo` built from the commit before the
/// change (1.1.174 for the change that brought the switch); each build runs in
/// the scene as new, at the same paths.
#[test]
#[ignore = "needs ZO_BASELINE_BIN: a zo built from the commit before the change"]
fn without_the_core_switch_a_run_sends_what_the_baseline_build_sends() {
    let baseline = PathBuf::from(std::env::var_os("ZO_BASELINE_BIN").expect("ZO_BASELINE_BIN names the baseline zo"));
    let this_build = Path::new(env!("CARGO_BIN_EXE_zo"));
    let steps: Vec<(&str, Value)> = ["pwd", "ls", "echo three", "printf 'four\\n'"]
        .into_iter()
        .map(|command| ("Bash", json!({"command": command, "description": "step"})))
        .collect();
    let args = ["--permission-mode", "danger-full-access"];
    for (what, calls) in [("the first request", &steps[..0]), ("a five-step run", &steps[..])] {
        let server = ChatServer::calling(calls, true);
        let scene = Scene::new();
        let before = scene.run(&baseline, &server, CORE_MODEL, &[], &args);
        scene.reset();
        let after = scene.run(this_build, &server, CORE_MODEL, &[], &args);
        let turns = ChatServer::parsed(&after.bodies).iter().filter(|body| body.get("tools").is_some()).count();
        assert_eq!(turns, calls.len() + 1, "{what}: every step reached the endpoint:\n{}", after.told());
        assert_eq!(before.bodies.len(), after.bodies.len(), "{what}: as many requests:\n{}\n{}", before.told(), after.told());
        for (index, (was, is)) in before.bodies.iter().zip(&after.bodies).enumerate() {
            assert_eq!(was, is, "{what}: request {index} differs from the baseline's");
        }
    }
}
