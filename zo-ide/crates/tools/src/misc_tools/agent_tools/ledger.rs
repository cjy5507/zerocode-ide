//! 창의 원장 워커를 기존 에이전트 완료 채널에 잇는다.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AgentInput, AgentJob, AgentOutput, completion::AgentCompletion};
use crate::ToolError;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Launch {
    pub agent: Option<String>,
    pub effort: Option<String>,
    pub worktree: Option<bool>,
    #[serde(skip)]
    pub selected: bool,
}

impl Launch {
    pub(crate) fn model(&self, input: &AgentInput) -> Option<String> {
        input.model.clone().or_else(|| {
            if self.agent.as_deref().unwrap_or("zo") == "zo" { input.route_model.clone() } else { None }
        })
    }

    pub(crate) fn inherit(&self, parent: &Self) -> Self {
        Self { agent: self.agent.clone().or_else(|| parent.agent.clone()),
            effort: self.effort.clone().or_else(|| parent.effort.clone()),
            worktree: self.worktree.or(parent.worktree), selected: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Seat {
    pub run: String,
    pub worker: String,
    pub dispatch: String,
    pub task: String,
}

#[derive(Clone)]
struct Client {
    program: PathBuf,
    run: Option<String>,
}

fn error(message: impl Into<String>) -> ToolError {
    ToolError::Execution(message.into())
}

fn validate_words(words: &[String]) -> Result<(), ToolError> {
    if words.iter().any(|word| word.contains(['\0', '\u{1f}'])) {
        return Err(ToolError::InvalidInput(
            "ledger arguments cannot contain NUL or the unit separator byte".into(),
        ));
    }
    Ok(())
}

impl Client {
    fn call(&self, words: &[String]) -> Result<Value, ToolError> {
        validate_words(words)?;
        if let Some(run) = &self.run { validate_words(std::slice::from_ref(run))?; }
        let mut command = command_for(&self.program, cfg!(windows));
        command.args(words);
        #[cfg(test)]
        command.env_clear();
        if let Some(run) = &self.run {
            command.args(["--run", run]);
        }
        let output = command.output().map_err(|e| error(format!("ledger: {e}")))?;
        if !output.status.success() {
            return Err(error(format!(
                "ledger {} failed: {}{}", words[0],
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        serde_json::from_slice(&output.stdout).map_err(|e| error(format!("ledger reply: {e}")))
    }

    fn read(&self, words: &[&str]) -> Result<Value, ToolError> {
        self.call(&words.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>())
    }

    fn change(&self, words: &[&str], retry: &str) -> Result<Value, ToolError> {
        let mut args: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        args.extend(["--retry-request".into(), retry.into()]);
        self.call(&args)
    }
}

fn command_for(program: &Path, windows: bool) -> Command {
    if windows {
        // Rust의 .cmd 실행은 줄바꿈 인자를 거절한다. 기존 문과 같은 프로세스 정책으로 .ps1을 직접 부른다.
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]);
        command.arg(program);
        command
    } else {
        Command::new(program)
    }
}

fn program_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|directory| {
        let program = directory.join(if cfg!(windows) { "zerocode-orc.ps1" } else { "zerocode-orc" });
        program.is_file().then_some(program)
    })
}

fn has_grant(lookup: impl Fn(&str) -> Option<String>) -> bool {
    let held = |key| lookup(key).is_some_and(|value| !value.is_empty());
    held("ZEROCODE_AGENT_TEAM_ID")
        && (held("ZEROCODE_AGENT_TEAM_PANE") || held("TMUX_PANE"))
        && (held("ZEROCODE_AGENT_TEAM_TOKEN") || held("ZEROCODE_AGENT_TEAM_TOKEN_FILE"))
}

fn available_from(lookup: impl Fn(&str) -> Option<String>, executable: bool) -> bool {
    executable && has_grant(lookup)
}

pub(crate) fn available() -> bool {
    // 크레이트 시험이 사람의 창에서 물려받은 권한으로 판을 열면 안 된다.
    !cfg!(test) && available_from(|key| std::env::var(key).ok(), program_on_path().is_some())
}

fn required(value: &Value, key: &str) -> Result<String, ToolError> {
    value[key].as_str().filter(|s| !s.is_empty()).map(str::to_owned)
        .ok_or_else(|| error(format!("ledger reply omitted {key}")))
}

fn bind(client: &Client, id: &str) -> Result<Client, ToolError> {
    static BIND: Mutex<()> = Mutex::new(());
    let _binding = BIND.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = client.read(&["run-current"])?;
    // 원장은 startedBy로 직계 부모에게 완료를 보낸다. 워커도 자기 run을 그대로 쓴다.
    let run = if let Some(run) = current["runId"].as_str() {
        run.to_owned()
    } else {
        let created = client.change(&["run-create", "--name", "zo delegation"], &format!("zo-run-{id}"))?;
        required(&created, "runId")?
    };
    Ok(Client { program: client.program.clone(), run: Some(run) })
}

fn validate_catalog(client: &Client, launch: &Launch, model: Option<&str>) -> Result<(), ToolError> {
    let agent = launch.agent.as_deref().unwrap_or("zo");
    let catalog = client.read(&["agent-list", "--agent", agent])?;
    let entry = catalog["agents"].as_array().and_then(|entries| entries.iter().find(|entry| entry["id"] == agent))
        .ok_or_else(|| error(format!("agent {agent} is absent from the ZeroCode catalog")))?;
    if entry["installed"] == "no" || entry["unsupportedHere"] == true {
        return Err(error(format!("agent {agent} is unavailable: {entry}")));
    }
    if model.is_some() && entry["takesModel"] != true {
        return Err(error(format!("agent {agent} cannot honor --model")));
    }
    if launch.effort.is_some() && entry["takesEffort"] != true {
        return Err(error(format!("agent {agent} cannot honor --effort")));
    }
    Ok(())
}

fn start_args(launch: &Launch, model: Option<&str>, task: &str, prompt: &str, retry: &str) -> Result<Vec<String>, ToolError> {
    let mut args = vec!["worker-start".into(), "--agent".into(), launch.agent.as_deref().unwrap_or("zo").into(),
        "--task".into(), task.into(), "--prompt".into(), prompt.into(), "--retry-request".into(), retry.into()];
    if let Some(model) = model { args.extend(["--model".into(), model.into()]); }
    if let Some(effort) = &launch.effort { args.extend(["--effort".into(), effort.clone()]); }
    if launch.worktree.unwrap_or(true) { args.push("--worktree".into()); }
    validate_words(&args)?;
    Ok(args)
}

/// What the window says when it refuses a worker-start because it is still
/// cutting another pane — "ask again in a moment". Nothing was started, so the
/// launch asks again under a new request name and cannot start two.
const PANE_CUT_REFUSAL: &str = "another pane is still being cut";

/// How often and how far apart the launch asks again after that refusal: a cut
/// takes a second or two, so eight asks a second and a half apart outlast it,
/// and a window that never stops cutting still answers within about twelve
/// seconds instead of hanging the spawn.
const PANE_CUT_ASKS: u32 = 8;
const PANE_CUT_PAUSE: Duration = Duration::from_millis(1500);

/// The worker-start, asked again while the window is cutting another pane.
fn start_worker(client: &Client, launch: &Launch, model: Option<&str>, task: &str, prompt: &str, id: &str) -> Result<Value, ToolError> {
    let mut ask = 0;
    loop {
        let retry = if ask == 0 { format!("zo-start-{id}") } else { format!("zo-start-{id}-{ask}") };
        match client.call(&start_args(launch, model, task, prompt, &retry)?) {
            Err(failure) if ask + 1 < PANE_CUT_ASKS && failure.to_string().contains(PANE_CUT_REFUSAL) => {
                ask += 1;
                std::thread::sleep(PANE_CUT_PAUSE);
            }
            answered => return answered,
        }
    }
}

/// Whether a launch receipt is the worker this launch asked for: the agent and
/// the worktree always; the model and the effort only where the launch pinned
/// them. One left open is the ledger's to fill — Jev picks it (t-14437) — and
/// its choice is not a deviation.
fn receipt_honours(receipt: &Value, agent: &str, model: Option<&str>, effort: Option<&str>, worktree: bool) -> bool {
    receipt["agent"] == agent
        && model.is_none_or(|model| receipt["model"].as_str() == Some(model))
        && effort.is_none_or(|effort| receipt["effort"].as_str() == Some(effort))
        && receipt["worktree"].as_bool() == Some(worktree)
}

pub(crate) fn execute(
    mut input: AgentInput,
    parent: Option<&str>,
    lsp: Option<&runtime::lsp_client::LspRegistry>,
    hooks: Option<&runtime::RuntimeHookConfig>,
) -> Result<AgentOutput, ToolError> {
    if super::is_fork_type(input.subagent_type.as_deref()) {
        return Err(error("a ledger worker cannot inherit a native conversation fork; supply its task context in prompt"));
    }
    if input.parent_permission_mode.is_some_and(|mode| mode != runtime::PermissionMode::DangerFullAccess) {
        return Err(error("ledger worker launch cannot enforce this session's restricted permission mode"));
    }
    input.launch.selected = true;
    let launch = input.launch.clone();
    let model = launch.model(&input);
    let client = Client { program: program_on_path().ok_or_else(|| error("zerocode-orc is unavailable"))?, run: None };
    validate_catalog(&client, &launch, model.as_deref())?;
    super::execute_agent_with_spawn_and_parent_model_and_hooks(input, move |job| spawn(&client, &launch, model, job), parent, lsp, hooks)
}

fn spawn(client: &Client, launch: &Launch, model: Option<String>, mut job: AgentJob) -> Result<(), ToolError> {
    if job.permission_rules.is_some() || job.permission_mode.is_some_and(|mode| mode != runtime::PermissionMode::DangerFullAccess) {
        return Err(error("ledger worker launch cannot enforce a custom restricted harness"));
    }
    // 원장이 만든 워크트리가 실행 위치다. 별도 로컬 격리 트리를 조용히 버리지 않는다.
    if job.cwd.is_some() {
        return Err(error("ledger workers use worktree:true; native workflow isolation cannot be combined with a ledger launch"));
    }
    let id = job.manifest.agent_id.clone();
    let mut prompt = job.prompt.clone();
    if !job.system_prompt.is_empty() {
        prompt = format!("{}\n\nTask:\n{prompt}", job.system_prompt.join("\n"));
    }
    if let Some(schema) = &job.schema {
        let _ = write!(prompt, "\nReturn a JSON value matching this schema in worker_done's structured field: {schema}");
    }
    prompt.push_str("\nReport through zerocode-orc send --type worker_done, with ok and summary in its JSON body. Preserve any requested agent, model and effort exactly. Use a unique --retry-request for each mutation.");
    start_args(launch, model.as_deref(), "pending", &prompt, &id)?;
    let client = bind(client, &id)?;
    let task = client.change(&["task-create", "--title", &job.manifest.description, "--spec", &prompt], &format!("zo-task-{id}"))?;
    let task = required(&task, "taskId")?;
    let ready = client.read(&["dispatch-show", "--task", &task])?;
    lifecycle(&ready)?;
    if !ready["dispatchId"].is_null() {
        return Err(error(format!("ledger task {task} was assigned before this exact launch")));
    }
    let receipt = start_worker(&client, launch, model.as_deref(), &task, &prompt, &id)?;
    let worker = required(&receipt, "workerId")?;
    let seat = Seat { run: client.run.clone().ok_or_else(|| error("ledger run missing"))?, worker,
        dispatch: required(&receipt, "dispatchId")?, task };
    let expected = launch.agent.as_deref().unwrap_or("zo");
    if !receipt_honours(&receipt, expected, model.as_deref(), launch.effort.as_deref(), launch.worktree.unwrap_or(true)) {
        let _ = stop(&client, &seat, "launch receipt did not honor the pinned choice");
        return Err(error(format!("ledger launch receipt differs from the requested agent/model/effort: {receipt}")));
    }
    job.manifest.pane = receipt["pane"].as_str().map(str::to_owned);
    job.manifest.model.clone_from(&model);
    // What actually runs: the pinned model, or the one the ledger chose.
    job.manifest.resolved_model = model.or_else(|| receipt["model"].as_str().map(str::to_owned));
    job.manifest.lifecycle.execution = Some("ledger".into());
    job.manifest.lifecycle.ledger = Some(seat.clone());
    if let Err(failure) = super::manifest::write_agent_manifest(&job.manifest) {
        let stopped = stop(&client, &seat, "could not persist the worker binding");
        return Err(error(format!("{failure}; worker-stop: {stopped:?}")));
    }
    let cleanup_client = client.clone();
    let cleanup_seat = seat.clone();
    std::thread::Builder::new().name(format!("zo-ledger-{id}")).spawn(move || watch(&client, &seat, &job))
        .map(|_| ()).map_err(|e| {
            let _ = stop(&cleanup_client, &cleanup_seat, "could not start the completion watcher");
            error(e.to_string())
        })
}

fn lifecycle(observation: &Value) -> Result<&[Value], ToolError> {
    observation["lifecycle"].as_array().map(Vec::as_slice).ok_or_else(|| error(
        "the window does not expose dispatch lifecycle observations; update/restart ZeroCode before using ledger delegation",
    ))
}

fn observe_dispatch(client: &Client, seat: &Seat) -> Result<Value, ToolError> {
    let observation = client.read(&["dispatch-show", "--task", &seat.task])?;
    if observation["dispatchId"] != seat.dispatch {
        return Err(error(format!("ledger task {} changed attempts; the replacement was not adopted", seat.task)));
    }
    lifecycle(&observation)?;
    Ok(observation)
}

fn stop(client: &Client, seat: &Seat, reason: &str) -> Result<(), ToolError> {
    if observe_dispatch(client, seat)?["open"] == false { return Ok(()); }
    client.change(&["worker-stop", "--worker", &seat.worker, "--dispatch", &seat.dispatch, "--reason", reason],
        &format!("zo-stop-{}-{}-{}", seat.worker, seat.dispatch, reason.replace(' ', "-"))).map(|_| ())
}

#[derive(Debug)]
struct Report {
    status: &'static str,
    result: Option<String>,
    structured: Option<Value>,
    failure: Option<String>,
}

fn report_text(message: &Value, body: &Value) -> Option<String> {
    let mut text = body["summary"].as_str().unwrap_or_default().to_owned();
    let mut metadata = body.as_object().cloned().unwrap_or_default();
    for key in ["ok", "summary", "structured"] { metadata.remove(key); }
    if let Some(payload) = message.get("payload").filter(|value| !value.is_null() && value.as_str() != Some("")) {
        metadata.insert("payload".into(), payload.as_str().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_else(|| payload.clone()));
    }
    if !metadata.is_empty() {
        let _ = write!(text, "\nLedger report metadata: {}", Value::Object(metadata));
    }
    (!text.is_empty()).then_some(text)
}

fn message_result(message: &Value, seat: &Seat) -> Option<Report> {
    let body = message["body"].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok()).unwrap_or_else(|| message["body"].clone());
    let dispatch = match message.get("dispatchId") {
        Some(Value::String(id)) => Some(id.as_str()),
        Some(Value::Null) | None => body["dispatchId"].as_str(),
        _ => None,
    };
    if dispatch != Some(seat.dispatch.as_str()) { return None; }
    match message["type"].as_str()? {
        "worker_done" if message["from"] == zerocode_core::orchestration::worker_address(&seat.worker) => {
            let ok = body["ok"].as_bool() == Some(true);
            let summary = body["summary"].as_str().map(str::to_owned);
            Some(Report { status: if ok { "completed" } else { "failed" }, result: report_text(message, &body),
                structured: body.get("structured").cloned(),
                failure: (!ok).then(|| summary.unwrap_or_else(|| "worker reported ok:false or omitted ok".into())) })
        }
        "worker_died" | "quota_walled" if message["source"] == "ledger" && body["workerId"] == seat.worker =>
            Some(Report { status: "failed", result: None, structured: None, failure: Some(format!("{}: {body}", message["type"].as_str()?)) }),
        _ => None,
    }
}

fn prepare_completion(job: &AgentJob, report: Report) -> Result<AgentCompletion, ToolError> {
    super::manifest::persist_agent_terminal_state(&job.manifest, report.status, report.result.as_deref(), report.failure.clone())?;
    let held = super::manifest::load_agent_manifest_from_scanned_path(std::path::Path::new(&job.manifest.manifest_file))?;
    Ok(AgentCompletion { agent_id: job.manifest.agent_id.clone(), name: job.manifest.name.clone(),
        status: held.status, result: report.result, structured: report.structured,
        error: report.failure.or(held.error), run: core_types::helper_run::HelperRun::default() })
}

fn notify(job: &AgentJob, completion: AgentCompletion) {
    super::completion::notify_agent_completion(completion,
        Some(job.manifest.run_generation), Some(PathBuf::from(&job.manifest.manifest_file)));
}

fn fail(job: &AgentJob, status: &'static str, failure: &str) {
    let report = Report { status, result: None, structured: None, failure: Some(failure.to_owned()) };
    let completion = prepare_completion(job, report).unwrap_or_else(|e| AgentCompletion {
        agent_id: job.manifest.agent_id.clone(), name: job.manifest.name.clone(), status: "failed".into(),
        result: None, structured: None, error: Some(format!("{failure}; could not persist completion: {e}")),
        run: core_types::helper_run::HelperRun::default(),
    });
    notify(job, completion);
}

fn matching_message<'a>(messages: &'a [Value], seat: &Seat) -> Option<&'a Value> {
    ["worker_done", "worker_died", "quota_walled"].into_iter().find_map(|kind| {
        messages.iter().find(|message| message["type"] == kind && message_result(message, seat).is_some())
    })
}

fn acknowledge(client: &Client, acted: &mut HashSet<String>) -> Result<(), ToolError> {
    let batch = client.read(&["check", "--types", "worker_done,worker_died,quota_walled"])?;
    let messages = batch["messages"].as_array().ok_or_else(|| error("ledger check omitted messages"))?;
    // 다른 소환이나 모델이 읽어야 할 편지를 대신 소비하지 않는다.
    if !messages.is_empty() && messages.iter().all(|m| m["messageId"].as_str().is_some_and(|id| acted.contains(id))) {
        let delivery = required(&batch, "deliveryId")?;
        client.change(&["check", "--ack", &delivery, "--peek"], &format!("zo-ack-{delivery}"))?;
        for message in messages { if let Some(id) = message["messageId"].as_str() { acted.remove(id); } }
    }
    Ok(())
}

fn collect_report(client: &Client, seat: &Seat, job: &AgentJob, acted: &mut HashSet<String>, next_probe: &mut Instant) -> Result<Option<AgentCompletion>, ToolError> {
    let batch = client.read(&["check", "--peek", "--types", "worker_done,worker_died,quota_walled"])?;
    let messages = batch["messages"].as_array().ok_or_else(|| error("ledger check omitted messages"))?;
    let mut candidate = matching_message(messages, seat).cloned();
    let mut ended = false;
    if (candidate.is_none() && Instant::now() >= *next_probe)
        || candidate.as_ref().is_some_and(|message| message["type"] == "quota_walled") {
        // 원장 코디네이터에게 간 통지도 자기 시도로 읽는다. 전체 우편 감사 목록을 폴링하지 않는다.
        let observation = observe_dispatch(client, seat)?;
        *next_probe = Instant::now() + Duration::from_secs(5);
        ended = observation["open"] == false;
        candidate = matching_message(lifecycle(&observation)?, seat).cloned().or(candidate);
    }
    let completion = if let Some(mut message) = candidate {
        if message["type"] == "quota_walled" && !ended {
            if let Err(failure) = stop(client, seat, "ledger quota wall") {
                let observation = observe_dispatch(client, seat)?;
                match matching_message(lifecycle(&observation)?, seat) {
                    Some(done) if done["type"] == "worker_done" => message = done.clone(),
                    _ => return Err(failure),
                }
            }
        }
        let Some(report) = message_result(&message, seat) else { return Ok(None); };
        let completion = prepare_completion(job, report)?;
        acted.insert(required(&message, "messageId")?);
        for held in messages {
            if message_result(held, seat).is_some() { acted.insert(required(held, "messageId")?); }
        }
        completion
    } else if ended {
        prepare_completion(job, Report { status: "failed", result: None, structured: None,
            failure: Some("the ledger attempt ended without a recoverable worker_done report".into()) })?
    } else {
        return Ok(None);
    };
    // 보고서를 저장한 뒤 확인 응답이 끊겨도 이미 받은 결과를 실패로 덮지 않는다.
    if let Err(failure) = acknowledge(client, acted) {
        eprintln!("zo ledger report stored; delivery acknowledgement pending: {failure}");
    }
    Ok(Some(completion))
}

fn watch(client: &Client, seat: &Seat, job: &AgentJob) {
    static ACTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let _registration = super::spawn::pane_worker_registration(job);
    let began = Instant::now();
    let mut next_probe = began;
    loop {
        let ended = if job.cancel_signal.is_aborted() { Some(("stopped", "parent cancelled the ledger worker")) }
            else if job.time_budget.is_some_and(|budget| began.elapsed() >= budget) { Some(("failed", "ledger worker timed out")) } else { None };
        if let Some((status, reason)) = ended {
            let failure = stop(client, seat, reason).err().map_or_else(|| reason.to_owned(), |e| format!("{reason}; worker-stop failed: {e}"));
            fail(job, status, &failure);
            return;
        }
        let outcome = {
            let mut acted = ACTED.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            collect_report(client, seat, job, &mut acted, &mut next_probe)
        };
        match outcome {
            Ok(Some(completion)) => { notify(job, completion); return; },
            Err(e) => {
                let stopped = stop(client, seat, "ledger completion watch failed");
                fail(job, "failed", &format!("{e}; worker-stop: {stopped:?}"));
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}

pub(crate) fn steer(seat: &Seat, text: &str) -> Result<(), ToolError> {
    let client = Client { program: program_on_path().ok_or_else(|| error("zerocode-orc is unavailable"))?, run: Some(seat.run.clone()) };
    client.change(&["send", "--to", &zerocode_core::orchestration::worker_address(&seat.worker), "--type", "status", "--body", text], &format!("zo-steer-{}", super::make_agent_id())).map(|_| ())
}

#[cfg(test)]
mod tests;
