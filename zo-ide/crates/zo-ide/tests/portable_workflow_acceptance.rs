#[allow(dead_code)]
#[path = "e2e/scripted.rs"]
mod scripted;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::time::Duration;

use scripted::ScriptedAnthropicService;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

const LIMIT: Duration = Duration::from_secs(90);
const ORIGINAL: &str = "PORTABLE_ORIGINAL_TASK: write the requested file and retain this instruction.";
const CONTENT: &str = "independently verified file contents\n";

struct Workspace {
    auto_plan: bool,
    objective_check: bool,
    _root: TempDir,
    cwd: PathBuf,
    home: PathBuf,
    sessions: PathBuf,
    state: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        let home = root.path().join("home");
        let sessions = root.path().join("sessions");
        let state = root.path().join("state");
        for path in [&cwd, &home, &sessions, &state] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(home.join("settings.json"), serde_json::to_vec(&json!({
            "smart": { "autoClassifier": "off", "orchestration": "model" }
        })).unwrap()).unwrap();
        Self { _root: root, cwd, home, sessions, state, auto_plan: false, objective_check: false }
    }

    async fn start(&self, service: &ScriptedAnthropicService, resume: bool, input: &str) -> Child {
        let binary = std::env::var_os("ZO_E2E_BIN")
            .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_zo")), PathBuf::from);
        let mut command = Command::new(binary);
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) { command.env(name, value); }
        }
        command
            .args(["--plain", "--permission-mode", "danger-full-access"])
            .current_dir(&self.cwd)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("APPDATA", &self.home)
            .env("LOCALAPPDATA", &self.home)
            .env("TMPDIR", &self.state)
            .env("TMP", &self.state)
            .env("TEMP", &self.state)
            .env("ZO_CONFIG_HOME", &self.home)
            .env("ZO_SESSION_ROOT", &self.sessions)
            .env("ZO_STATE_DIR", &self.state)
            .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
            .env("ZO_DISABLE_KEYCHAIN", "1")
            .env("ZO_DISABLE_EXTERNAL_CREDENTIALS", "1")
            .env("ANTHROPIC_BASE_URL", service.base_url())
            .env("ANTHROPIC_API_KEY", "test-dummy-key")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !self.auto_plan { command.args(["--model", "claude-sonnet-4-6"]); }
        if self.objective_check { command.env("ZO_AUTO_VERIFY", "on").env("ZO_AUTO_VERIFY_CMD", "echo checked"); }
        if resume { command.arg("--continue"); }
        let mut child = command.spawn().expect("start the real zo binary");
        child.stdin.as_mut().unwrap().write_all(input.as_bytes()).await.unwrap();
        child
    }

    async fn run(&self, service: &ScriptedAnthropicService, resume: bool, input: &str) -> Output {
        finish(self.start(service, resume, input).await).await
    }
}

async fn finish(mut child: Child) -> Output {
    drop(child.stdin.take());
    timeout(LIMIT, child.wait_with_output()).await.expect("zo process timed out").unwrap()
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "zo exited {:?}: {}", output.status, String::from_utf8_lossy(&output.stderr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn measured_plan_changes_the_real_wire_then_falls_back_and_respects_a_pin() {
    let mut workspace = Workspace::new();
    workspace.auto_plan = true;
    workspace.objective_check = true;
    fs::write(workspace.home.join("settings.json"), serde_json::to_vec(&json!({
        "smart": { "autoClassifier": "off", "orchestration": "model", "plan": { "apply": true } }
    })).unwrap()).unwrap();
    let input = "Implement a small low-risk Rust helper in a single file without delegation.";
    let assessment = tools::assess_turn_deterministic(input);
    let hint = tools::assess_turn_orchestration(input);
    let cohort = tools::plan_cohort_for_turn(input, assessment.complexity, hint.risk).unwrap();
    let current = api::builtin_provider_catalog().iter()
        .find(|entry| entry.alias == api::ANTHROPIC_LATEST_MODEL_ALIAS).unwrap().canonical_model_id.to_string();
    let models: BTreeSet<_> = api::builtin_provider_catalog().iter()
        .filter(|entry| entry.provider == api::ProviderKind::Anthropic)
        .map(|entry| entry.canonical_model_id)
        .filter(|model| api::model_accepts_effort(model, api::EffortLevel::High)).collect();
    assert!(models.contains(current.as_str()) && models.len() >= 2);
    let now = u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap();
    let mut receipts = Vec::new();
    for model in &models {
        for index in 0..12 {
            receipts.push(tools::PlanRunReceipt {
                version: 1, at_ms: now - 2_000, attempt: format!("fixture-{model}@{index}"), cohort: cohort.clone(),
                plan: tools::PlanShadowActual { model: (*model).into(), effort: Some("high".into()), shape: "solo".into(), verify: "objective".into() },
                verified: Some(true), duration_ms: 10_000, total_tokens: Some(5_000),
                total_usd: Some(if *model == current { 100.0 } else { 1.0 }), held: Vec::new(),
            });
        }
    }
    let ledger = workspace.state.join("projects").join(runtime::project_slug(&workspace.cwd))
        .join("state/smart-router/plan-receipts.jsonl");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let write = |receipts: &[tools::PlanRunReceipt]| {
        fs::write(&ledger, receipts.iter().map(|receipt| format!("{}\n", serde_json::to_string(receipt).unwrap())).collect::<String>()).unwrap();
    };
    write(&receipts);
    let service = ScriptedAnthropicService::text("fixture response without a completion claim").await.unwrap();
    let output = workspace.run(&service, false, &format!("{input}\n/exit\n")).await;
    assert_success(&output);
    let requests = service.request_bodies().await;
    let selected: Value = serde_json::from_str(&requests[0]).unwrap();
    assert_ne!(selected["model"].as_str(), Some(current.as_str()),
        "stdout: {}\nstderr: {}\nreceipt: {}\nshadow: {}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&ledger).unwrap_or_default().lines().last().unwrap_or("missing"),
        fs::read_to_string(ledger.with_file_name("plan-shadow.jsonl")).unwrap_or_default().lines().last().unwrap_or("missing"));
    assert!(models.contains(selected["model"].as_str().unwrap()));
    for receipt in &mut receipts {
        if receipt.plan.model != current { receipt.verified = Some(false); }
    }
    write(&receipts);
    let before = requests.len();
    assert_success(&workspace.run(&service, false, &format!("{input}\n/exit\n")).await);
    let requests = service.request_bodies().await;
    let fallback: Value = serde_json::from_str(&requests[before]).unwrap();
    assert_eq!(fallback["model"].as_str(), Some(current.as_str()));
    workspace.auto_plan = false;
    for receipt in &mut receipts { receipt.verified = Some(true); }
    write(&receipts);
    let before = requests.len();
    assert_success(&workspace.run(&service, false, &format!("{input}\n/exit\n")).await);
    let requests = service.request_bodies().await;
    let pinned: Value = serde_json::from_str(&requests[before]).unwrap();
    assert_eq!(pinned["model"], "claude-sonnet-4-6");
}

fn assert_restored_tool_pairs(body: &str) {
    let request: Value = serde_json::from_str(body).unwrap();
    assert!(request["messages"].to_string().contains(ORIGINAL), "the original user instruction was not restored");
    let mut calls = BTreeSet::new();
    let mut results = BTreeSet::new();
    for message in request["messages"].as_array().unwrap() {
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" {
                assert!(calls.insert(block["id"].as_str().unwrap().to_owned()), "a tool call was duplicated");
            }
            if block["type"] == "tool_result" {
                assert!(results.insert(block["tool_use_id"].as_str().unwrap().to_owned()), "a tool result was duplicated");
            }
        }
    }
    assert!(!calls.is_empty(), "no executed tool was restored");
    assert_eq!(calls, results, "restored tool calls and results must remain paired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_process_reopens_a_completed_tool_conversation() {
    let workspace = Workspace::new();
    let service = ScriptedAnthropicService::writes_then_bash(
        vec![("result.txt".into(), CONTENT.into())], None, Duration::ZERO, "PORTABLE_DONE",
    ).await.unwrap();
    let first = workspace.run(&service, false, &format!("{ORIGINAL}\n/exit\n")).await;
    assert_success(&first);
    assert_eq!(fs::read_to_string(workspace.cwd.join("result.txt")).unwrap(), CONTENT);
    let prior = service.request_bodies().await.len();
    assert!(prior >= 2, "a tool execution must reach the next provider request");
    let resumed = workspace.run(&service, true, "Confirm the saved result without changing it.\n/exit\n").await;
    assert_success(&resumed);
    let requests = service.request_bodies().await;
    assert!(requests.len() > prior);
    assert_restored_tool_pairs(&requests[prior]);
    assert_eq!(fs::read_to_string(workspace.cwd.join("result.txt")).unwrap(), CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_process_recovers_after_termination_between_tool_and_answer() {
    let workspace = Workspace::new();
    let parked = ScriptedAnthropicService::writes_then_bash(
        vec![("result.txt".into(), CONTENT.into())], None, LIMIT * 2, "NOT_REACHED",
    ).await.unwrap();
    let mut interrupted = workspace.start(&parked, false, &format!("{ORIGINAL}\n")).await;
    timeout(LIMIT, async {
        while parked.request_bodies().await.len() < 2 { sleep(Duration::from_millis(10)).await; }
    }).await.expect("the completed tool never reached the next request");
    assert_eq!(fs::read_to_string(workspace.cwd.join("result.txt")).unwrap(), CONTENT);
    assert!(interrupted.try_wait().unwrap().is_none(), "the process must still be live before termination");
    interrupted.kill().await.unwrap();
    let stopped = finish(interrupted).await;
    assert!(!stopped.status.success());
    let resumed_service = ScriptedAnthropicService::text("PORTABLE_RESUMED").await.unwrap();
    let resumed = workspace.run(&resumed_service, true, "Finish the interrupted request from its saved state.\n/exit\n").await;
    assert_success(&resumed);
    let requests = resumed_service.request_bodies().await;
    assert!(!requests.is_empty());
    assert_restored_tool_pairs(&requests[0]);
    assert_eq!(fs::read_to_string(workspace.cwd.join("result.txt")).unwrap(), CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_process_preserves_tool_failure_even_when_the_model_claims_done() {
    let workspace = Workspace::new();
    let service = ScriptedAnthropicService::tool_calls(
        &[("read_file", json!({"path":"missing-file.txt"}))], "MODEL_SAYS_DONE",
    ).await.unwrap();
    let output = workspace.run(&service, false, "Read the missing file.\n/exit\n").await;
    assert_success(&output);
    assert!(!workspace.cwd.join("missing-file.txt").exists());
    let requests = service.request_bodies().await;
    assert!(requests.len() >= 2);
    let after_tool: Value = serde_json::from_str(&requests[1]).unwrap();
    assert!(after_tool["messages"].as_array().unwrap().iter().any(|message| {
        message["content"].as_array().into_iter().flatten().any(|block| {
            block["type"] == "tool_result" && block["is_error"] == true
        })
    }), "a failed real tool must not become a successful tool result: {}", after_tool["messages"]);
}

fn write_preferences(workspace: &Workspace, book: &zerocode_core::user_preferences::PreferenceBook) {
    use zerocode_core::user_preferences::{DIRECTORY_NAME, FILE_NAME};
    let home = workspace.home.canonicalize().unwrap();
    runtime::secure_fs::ensure_private_dir(&home, std::path::Path::new(DIRECTORY_NAME)).unwrap();
    runtime::secure_fs::write_atomic_owner_only(&home, &std::path::Path::new(DIRECTORY_NAME).join(FILE_NAME), &book.encode().unwrap()).unwrap();
}

fn latest_preference_context(request: &Value) -> String {
    request["messages"].as_array().unwrap().iter()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter_map(|block| block["text"].as_str())
        .rfind(|text| text.contains(runtime::memory::user_preferences::PREFERENCE_REMINDER_PREFIX))
        .expect("a current preference snapshot reached the provider").to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_process_applies_and_revokes_saved_preferences_without_rewriting_the_system_prefix() {
    use zerocode_core::user_preferences::{FeedbackOrigin, PreferenceBook, PreferenceScope, project_key};
    let workspace = Workspace::new();
    let mut book = PreferenceBook::default();
    let id = book.save("PREFERENCE_SENTINEL: keep headings concise", PreferenceScope::Project {
        key: project_key(&workspace.cwd).unwrap(),
    }, FeedbackOrigin {
        artifact_id: "page-example".into(), version: 2, sha256: Some("c".repeat(64)),
        feedback_key: "d".repeat(64), followed_link: false,
    }, 10).unwrap();
    write_preferences(&workspace, &book);
    let service = ScriptedAnthropicService::text("PREFERENCE_WORKFLOW_DONE").await.unwrap();
    let first = workspace.run(&service, false, "Describe the next page.\n/exit\n").await;
    assert_success(&first);
    let first_requests = service.request_bodies().await;
    let first_request: Value = serde_json::from_str(&first_requests[0]).unwrap();
    assert!(latest_preference_context(&first_request).contains("PREFERENCE_SENTINEL"));
    book.revoke(&id).unwrap();
    write_preferences(&workspace, &book);
    let before_resume = first_requests.len();
    let resumed = workspace.run(&service, true, "Describe another page after the preference was revoked.\n/exit\n").await;
    assert_success(&resumed);
    let requests = service.request_bodies().await;
    let resumed_request: Value = serde_json::from_str(&requests[before_resume]).unwrap();
    let active = latest_preference_context(&resumed_request);
    assert!(!active.contains("PREFERENCE_SENTINEL"), "the new snapshot retained a revoked preference: {active}");
    assert!(active.contains("[]"));
    assert_eq!(first_request["system"], resumed_request["system"]);
    fs::write(workspace.home.join("settings.json"), serde_json::to_vec(&json!({
        "autoMemoryEnabled": false,
        "smart": { "autoClassifier": "off", "orchestration": "model" }
    })).unwrap()).unwrap();
    let before_disabled = requests.len();
    let disabled = workspace.run(&service, true, "Continue with saved memory disabled.\n/exit\n").await;
    assert_success(&disabled);
    let requests = service.request_bodies().await;
    let disabled_request: Value = serde_json::from_str(&requests[before_disabled]).unwrap();
    assert!(latest_preference_context(&disabled_request).contains("disabled"));
    assert_eq!(first_request["system"], disabled_request["system"]);
}
