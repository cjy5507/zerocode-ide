use super::*;
use serde_json::json;

fn seat() -> Seat {
    Seat { run: "run-1".into(), worker: "w-2".into(), dispatch: "dp-3".into(), task: "t-4".into() }
}

#[test]
fn ledger_road_requires_both_a_team_grant_and_a_door() {
    assert!(!available_from(|_| None, true));
    assert!(!available_from(|_| Some("fixture".into()), false));
    assert!(available_from(|_| Some("fixture".into()), true));
    assert!(has_grant(|key| match key {
        "ZEROCODE_AGENT_TEAM_ID" | "ZEROCODE_AGENT_TEAM_PANE" | "ZEROCODE_AGENT_TEAM_TOKEN_FILE" => Some("fixture".into()),
        _ => None,
    }));
    assert!(!has_grant(|key| (key == "ZEROCODE_PANE_KEY").then(|| "fixture".into())));
}

#[test]
fn argv_preserves_pins_and_prompt_without_shell_interpretation() {
    let launch = Launch { agent: Some("codex".into()), effort: Some("low".into()), worktree: Some(true), selected: true };
    let prompt = "quotes ' \" and $(false) `false`\nnext line";
    let args = start_args(&launch, Some("exact-model"), "t-4", prompt, "start-4").unwrap();
    assert_eq!(args[2], "codex");
    assert_eq!(args[6], prompt);
    assert!(args.windows(2).any(|words| words == ["--model", "exact-model"]));
    assert!(args.windows(2).any(|words| words == ["--effort", "low"]));
    assert!(args.contains(&"--worktree".into()));
    for prompt in ["a\u{1f}b", "a\0b"] {
        assert!(start_args(&launch, None, "t-4", prompt, "start-4").is_err());
    }
}

#[test]
fn ledger_mail_keeps_failure_and_structured_output() {
    for ok in [true, false] {
        let message = json!({"type":"worker_done", "from":"worker:w-2", "dispatchId":"dp-3", "body":json!({"ok":ok,"summary":"answer","structured":{"n":2}}).to_string()});
        let result = message_result(&message, &seat()).unwrap();
        assert_eq!(result.status, if ok { "completed" } else { "failed" });
        assert_eq!(result.result.as_deref(), Some("answer"));
        assert_eq!(result.structured, Some(json!({"n":2})));
        assert_eq!(result.failure.is_some(), !ok);
    }
    for kind in ["worker_died", "quota_walled"] {
        let message = json!({"type":kind,"source":"ledger","dispatchId":"dp-3","body":{"workerId":"w-2"}});
        assert_eq!(message_result(&message, &seat()).unwrap().status, "failed");
    }
}

#[test]
fn peer_text_and_other_dispatches_cannot_settle_a_worker() {
    for message in [
        json!({"type":"worker_done","from":"w-other","dispatchId":"dp-3","body":{"ok":true}}),
        json!({"type":"worker_done","from":"worker:w-2","dispatchId":"dp-old","body":{"ok":true}}),
        json!({"type":"worker_done","from":"worker:w-2","dispatchId":"dp-new","body":{"ok":true,"dispatchId":"dp-3"}}),
        json!({"type":"worker_died","source":"agent","dispatchId":"dp-3","body":{"workerId":"w-2"}}),
    ] {
        assert!(message_result(&message, &seat()).is_none());
    }
}

#[cfg(unix)]
fn fake_door(root: &std::path::Path, kind: &str, ok: bool) -> Client {
    use std::os::unix::fs::PermissionsExt;
    let program = root.join("zerocode-orc");
    let script = r"#!/usr/bin/python3
import json, os, sys
from pathlib import Path
base = Path(__file__).parent
args = sys.argv[1:]
assert not any(k.startswith('ZEROCODE_') or k in ('TMUX', 'TMUX_PANE') for k in os.environ)
with open(base / 'argv.jsonl', 'a') as f: f.write(json.dumps(args) + '\n')
verb = args[0]
config = json.loads((base / 'config.json').read_text())
kind = config['kind']
def message(event):
    body = {'ok':config['ok'],'summary':'ledger final answer','workerId':'w-2'}
    return {'messageId':'m-' + base.name + '-' + event, 'from':'worker:w-2' if event == 'worker_done' else 'ledger', 'dispatchId':'dp-3','type':event,'source':'agent' if event == 'worker_done' else 'ledger','body':json.dumps(body)}
event = {'observed_death':'worker_died','observed_quota':'quota_walled','observed_done':'worker_done','ack_failure':'worker_done','cut_once':'worker_done','slow_start':'worker_done'}.get(kind,kind)
events = [message(event)] if event in ('worker_done','worker_died','quota_walled') else []
if kind == 'quota_then_done': events = [message('quota_walled'),message('worker_done')]
if verb == 'run-current': out = {'runId':'run-1', 'seated':False}
elif verb == 'agent-list': out = {'agents':[{'id':'codex','takesModel':True,'takesEffort':True,'installed':'yes'}]}
elif verb == 'task-create': out = {'taskId':'t-4'}
elif verb == 'dispatch-show':
    if not (base/'started').exists(): out = {'dispatchId':None,'lifecycle':[]}
    else:
        ended = kind in ('observed_death','observed_done') or (base/'stopped').exists()
        out = {'dispatchId':'dp-3','open':not ended,'lifecycle':events if (base/'peeked').exists() else []}
elif verb == 'worker-start':
    if kind == 'cut_once' and not (base/'cut').exists():
        (base/'cut').write_text('yes')
        print('orchestration: another pane is still being cut \u2014 ask again in a moment', file=sys.stderr)
        sys.exit(1)
    if kind == 'start_fails':
        print('fixture: the window could not open a pane for this agent', file=sys.stderr)
        sys.exit(1)
    if kind == 'slow_start':
        import time
        until = time.time() + 10
        while not (base/'release').exists() and time.time() < until: time.sleep(0.02)
    (base/'started').write_text('yes')
    out = {'workerId':'w-2','dispatchId':'dp-3','agent':'codex','model':'exact-model','effort':'low','pane':'%2','worktree':True}
elif verb == 'check':
    if '--ack' in args:
        if kind == 'ack_failure':
            print('fixture acknowledgement unavailable', file=sys.stderr)
            sys.exit(1)
        (base / 'acked').write_text('yes')
        out = {'messages':[]}
    else:
        (base/'peeked').write_text('yes')
        out = {'messages':[] if kind.startswith('observed_') or kind == 'timeout' else events, 'deliveryId':'delivery-' + base.name}
elif verb == 'worker-stop':
    assert args[args.index('--dispatch')+1] == 'dp-3'
    (base / 'stopped').write_text('yes')
    out = {'state':'stopped'}
else: raise Exception(args)
print(json.dumps(out))
";
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join("config.json"), json!({"kind":kind,"ok":ok}).to_string()).unwrap();
    Client { program, run: None }
}

#[cfg(unix)]
#[test]
fn fake_ledger_launch_settles_the_existing_completion_channel_and_acks_afterward() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for (kind, ok, expected) in [("worker_done",true,"completed"),("worker_done",false,"failed"),("worker_died",false,"failed"),("quota_walled",false,"failed"),("timeout",false,"failed"),("cancelled",false,"stopped"),("observed_death",false,"failed"),("observed_quota",false,"failed"),("observed_done",true,"completed"),("quota_then_done",true,"completed"),("ack_failure",true,"completed")] {
        let root = tempfile::tempdir().unwrap();
        let client = fake_door(root.path(), kind, ok);
        let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","agent":"codex","model":"exact-model","effort":"low","worktree":true})).unwrap();
        input.launch.selected = true;
        input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-fixture", &root.path().join("store")));
        if kind == "timeout" { input.time_budget = Some(Duration::ZERO); }
        let launch = input.launch.clone();
        validate_catalog(&client, &launch, input.model.as_deref()).unwrap();
        let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input, move |job| { if kind == "cancelled" { job.cancel_signal.abort(); } spawn(&client, &launch, Some("exact-model".into()), job) }, None, None, None).unwrap();
        let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(10));
        assert_eq!(results.len(), 1, "{kind}");
        assert_eq!(results[0].status, expected, "{kind}: {results:?}");
        if kind.starts_with("observed_") || kind == "ack_failure" {
            assert!(!root.path().join("acked").exists(), "another reader's observation was acknowledged");
            if kind == "observed_quota" { assert!(root.path().join("stopped").exists()); }
        } else {
            let until = Instant::now() + Duration::from_secs(5);
            let receipt = root.path().join(if matches!(kind, "timeout" | "cancelled") { "stopped" } else { "acked" });
            while !receipt.exists() && Instant::now() < until { std::thread::sleep(Duration::from_millis(10)); }
            assert!(receipt.exists(), "{kind} left no receipt");
        }
        if matches!(kind, "quota_then_done" | "ack_failure" | "observed_done") {
            assert!(!root.path().join("stopped").exists(), "a completed worker was stopped");
        }
        let args = std::fs::read_to_string(root.path().join("argv.jsonl")).unwrap();
        assert!(args.contains("--worktree"));
        // The ledger road never reaches for tmux: no word of any call is tmux or a tmux flag. Words, not the
        // file's text — a checkout's folder can spell tmux (t-18917-zo-tmux-tmux went red on that alone).
        let calls: Vec<Vec<String>> = args.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert!(calls.iter().flatten().all(|word| word != "tmux" && !word.starts_with("--tmux")), "{calls:?}");
        assert_eq!(manifest.lifecycle.execution.as_deref(), Some("ledger"));
    }
}

/// A ledger worker is a CLI of its own that builds its own session prompt for its own checkout. The task it is
/// handed carries this agent's role and the delegated work — not a second copy of the launching session's prompt,
/// which the worker would send again in every request (about 17 KB a turn on 2026-10-14) and which names the
/// launching session's directory instead of the worker's.
#[cfg(unix)]
#[test]
fn a_ledger_task_carries_the_agent_role_not_a_second_session_prompt() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "worker_done", true);
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","subagent_type":"general-purpose","agent":"codex","model":"exact-model","effort":"low","worktree":true})).unwrap();
    input.launch.selected = true;
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-role", &root.path().join("store")));
    let launch = input.launch.clone();
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input, move |job| spawn(&client, &launch, Some("exact-model".into()), job), None, None, None).unwrap();
    let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(10));
    assert_eq!(results.len(), 1);
    let until = Instant::now() + Duration::from_secs(5);
    while !root.path().join("acked").exists() && Instant::now() < until { std::thread::sleep(Duration::from_millis(10)); }

    let calls: Vec<Vec<String>> = std::fs::read_to_string(root.path().join("argv.jsonl")).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let spec = calls.iter().find(|call| call[0] == "task-create").and_then(|call| call.windows(2).find(|words| words[0] == "--spec")).map(|words| words[1].clone()).expect("task-create carries the task text");
    assert!(calls.iter().any(|call| call[0] == "worker-start" && call.contains(&spec)), "worker-start hands over the same task text");
    let role = super::super::subagent_profile::builtin_harness_instruction("general-purpose");
    assert!(spec.starts_with(&format!("{role}\n\nTask:\n")), "{spec}");
    assert!(spec.contains("report the fixture"));
    let session_prompt = runtime::load_system_prompt(std::env::current_dir().unwrap(), core_types::date::current_local_date(), std::env::consts::OS, "unknown", Some("exact-model")).unwrap();
    for section in &session_prompt {
        let section = section.trim();
        assert!(section.is_empty() || !spec.contains(section), "the launching session's prompt reached the worker's task: {section}");
    }
}

#[test]
fn worktree_defaults_to_isolation_and_an_explicit_reader_can_omit_it() {
    assert!(start_args(&Launch::default(), None, "t-4", "read", "start-4").unwrap().contains(&"--worktree".into()));
    let reader = Launch { worktree: Some(false), ..Launch::default() };
    assert!(!start_args(&reader, None, "t-4", "read", "start-4").unwrap().contains(&"--worktree".into()));
}

#[cfg(unix)]
#[test]
fn an_absent_catalog_agent_is_a_blocker_before_any_worker_is_started() {
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "worker_done", true);
    let launch = Launch { agent: Some("unavailable-agent".into()), ..Launch::default() };
    let failure = validate_catalog(&client, &launch, None).unwrap_err().to_string();
    assert!(failure.contains("absent from the ZeroCode catalog"), "{failure}");
    let calls = std::fs::read_to_string(root.path().join("argv.jsonl")).unwrap();
    assert!(!calls.contains("worker-start"));
}

#[cfg(unix)]
#[test]
fn a_different_model_in_the_launch_receipt_stops_that_worker_without_fallback() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "worker_done", true);
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report fixture","agent":"codex","model":"requested-model","effort":"low"})).unwrap();
    input.launch.selected = true;
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-fixture", &root.path().join("store")));
    let launch = input.launch.clone();
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
        move |job| spawn(&client, &launch, Some("requested-model".into()), job), None, None, None)
        .unwrap_or_else(|failure| panic!("the launch hands back its starting receipt: {failure}"));
    let failure = settled_failure(&manifest);
    assert!(failure.contains("differs from the requested"), "{failure}");
    assert!(root.path().join("stopped").exists());
    let calls = std::fs::read_to_string(root.path().join("argv.jsonl")).unwrap();
    assert_eq!(calls.lines().filter(|line| line.contains("worker-start")).count(), 1);
}

#[test]
fn the_final_report_keeps_its_commit_and_external_report_pointer() {
    let message = json!({"type":"worker_done", "from":"worker:w-2", "dispatchId":"dp-3",
        "body": {"ok":true,"summary":"implemented","head":"abc123","structured":{"n":2}},
        "payload": "{\"reportPath\":\"/tmp/fixture-report.md\",\"lifetime\":\"ephemeral\"}"});
    let report = message_result(&message, &seat()).unwrap();
    let text = report.result.unwrap();
    assert!(text.contains("implemented"));
    assert!(text.contains("abc123"));
    assert!(text.contains("/tmp/fixture-report.md"));
    assert_eq!(report.structured, Some(json!({"n":2})));
}

#[test]
fn every_native_fork_spelling_is_refused_before_the_ledger_door() {
    for kind in ["fork", "Fork", " FORK "] {
        let input: AgentInput = serde_json::from_value(json!({"description":"fork fixture", "prompt":"continue", "subagent_type":kind})).unwrap();
        let failure = execute(input, None, None, None).unwrap_err().to_string();
        assert!(failure.contains("cannot inherit a native conversation fork"), "{failure}");
    }
}

#[test]
fn windows_calls_the_powershell_script_without_cmd_rejecting_multiline_prompts() {
    let prompt = "first line\nquotes \" and %PATH% $(literal)";
    let mut command = command_for(Path::new("zerocode-orc.ps1"), true);
    command.args(["worker-start", "--prompt", prompt]);
    assert_eq!(command.get_program(), "powershell.exe");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", "zerocode-orc.ps1", "worker-start", "--prompt", prompt]);
}

#[test]
fn the_door_accepts_the_tmux_pane_spelling_of_a_complete_team_grant() {
    assert!(available_from(|key| match key {
        "ZEROCODE_AGENT_TEAM_ID" | "ZEROCODE_AGENT_TEAM_TOKEN_FILE" | "TMUX_PANE" => Some("fixture".into()),
        _ => None,
    }, true));
}

#[test]
fn a_separator_in_the_captured_run_is_refused_before_starting_a_process() {
    let client = Client { program: PathBuf::from("missing-ledger-fixture"), run: Some("run-1\u{1f}other".into()) };
    let failure = client.read(&["run-show"]).unwrap_err().to_string();
    assert!(failure.contains("separator byte"), "{failure}");
}

/// The ledger fills what a launch left open — Jev picks the effort, or the model,
/// when the caller named none (t-14437) — and that choice is not a deviation: a
/// receipt is held only to what this launch pinned. Before, every launch that
/// named no effort was stopped as "differs from the requested" the moment its
/// worker came up (a person's zo session, 2026-10-01).
#[cfg(unix)]
#[test]
fn what_the_ledger_chose_for_an_unpinned_choice_is_honoured() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for (pinned, model) in [(json!({"agent":"codex","model":"exact-model"}), Some("exact-model")), (json!({"agent":"codex"}), None)] {
        let root = tempfile::tempdir().unwrap();
        let client = fake_door(root.path(), "worker_done", true);
        let mut fields = json!({"description":"survey fixture","prompt":"report fixture"});
        for (key, value) in pinned.as_object().unwrap() { fields[key] = value.clone(); }
        let mut input: AgentInput = serde_json::from_value(fields).unwrap();
        input.launch.selected = true;
        input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-fixture", &root.path().join("store")));
        let launch = input.launch.clone();
        let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
            move |job| spawn(&client, &launch, model.map(str::to_owned), job), None, None, None)
            .unwrap_or_else(|failure| panic!("{pinned}: {failure}"));
        let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(10));
        assert_eq!(results[0].status, "completed", "{pinned}: {results:?}");
        assert!(!root.path().join("stopped").exists(), "{pinned}: the worker the ledger chose for was stopped");
    }
}

/// What a launch did pin stays pinned: an effort the receipt changed stops that
/// worker, as a changed model always did.
#[cfg(unix)]
#[test]
fn a_pinned_effort_the_receipt_changed_stops_that_worker() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "worker_done", true);
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report fixture","agent":"codex","model":"exact-model","effort":"high"})).unwrap();
    input.launch.selected = true;
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-fixture", &root.path().join("store")));
    let launch = input.launch.clone();
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
        move |job| spawn(&client, &launch, Some("exact-model".into()), job), None, None, None)
        .unwrap_or_else(|failure| panic!("the launch hands back its starting receipt: {failure}"));
    let failure = settled_failure(&manifest);
    assert!(failure.contains("differs from the requested"), "{failure}");
    assert!(root.path().join("stopped").exists());
}

/// The window refuses a worker-start while it is still cutting another pane and
/// says to ask again in a moment; nothing was started, so the launch asks again
/// under a new request name and the worker comes up. Before, the refusal ended
/// the spawn ("another pane is still being cut", a person's zo session).
#[cfg(unix)]
#[test]
fn a_start_refused_while_another_pane_is_cut_is_asked_again() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "cut_once", true);
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report fixture","agent":"codex","model":"exact-model","effort":"low"})).unwrap();
    input.launch.selected = true;
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-fixture", &root.path().join("store")));
    let launch = input.launch.clone();
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
        move |job| spawn(&client, &launch, Some("exact-model".into()), job), None, None, None)
        .unwrap_or_else(|failure| panic!("{failure}"));
    let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(20));
    assert_eq!(results[0].status, "completed", "{results:?}");
    let starts: Vec<Vec<String>> = std::fs::read_to_string(root.path().join("argv.jsonl")).unwrap().lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .filter(|args| args[0] == "worker-start").collect();
    assert_eq!(starts.len(), 2, "{starts:?}");
    let retry = |args: &Vec<String>| args[args.iter().position(|word| word == "--retry-request").unwrap() + 1].clone();
    assert_ne!(retry(&starts[0]), retry(&starts[1]), "the second ask reused the refused request's name");
}

/// What a launch left in the store the screen reads: the manifest's JSON.
fn stored_manifest(manifest: &AgentOutput) -> Value {
    serde_json::from_str(&std::fs::read_to_string(&manifest.manifest_file).unwrap()).unwrap()
}

/// t-69472: the parent's spawn cell finds a running helper through the
/// manifest it reads in the session's store: `running`, under the session,
/// with the `tool_use` id of the call that spawned it. A general-purpose
/// `Agent` call that runs in this process leaves one.
#[test]
fn an_agent_call_in_this_process_leaves_a_running_manifest_with_its_call() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","subagent_type":"general-purpose"})).unwrap();
    input.tool_call_id = Some("toolu_c69472_inline".into());
    input.parent_session_id = Some("session-c69472".into());
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("session-c69472", &root.path().join("store")));
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input, |_job| Ok(()), None, None, None)
        .unwrap_or_else(|failure| panic!("{failure}"));
    let stored = stored_manifest(&manifest);
    assert_eq!(stored["status"], "running", "{stored}");
    assert_eq!(stored["toolCallId"], "toolu_c69472_inline", "{stored}");
    assert_eq!(stored["parentSessionId"], "session-c69472", "{stored}");
    assert_ne!(stored["execution"], "ledger", "{stored}");
}

/// t-69472: the same call inside ZeroCode opens a ledger worker, and leaves
/// the same manifest while the worker runs — `running`, under the session,
/// with the call's `tool_use` id — and `execution: ledger`, which tells the
/// screen the helper runs in a tab of its own.
#[cfg(unix)]
#[test]
fn an_agent_call_that_opens_a_ledger_worker_leaves_a_running_manifest_with_its_call() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    // A worker that keeps running until it is stopped: it never reports.
    let client = fake_door(root.path(), "running", true);
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","subagent_type":"general-purpose","agent":"codex","model":"exact-model","effort":"low","worktree":true})).unwrap();
    input.launch.selected = true;
    input.tool_call_id = Some("toolu_c69472_ledger".into());
    input.parent_session_id = Some("session-c69472".into());
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("session-c69472", &root.path().join("store")));
    let launch = input.launch.clone();
    let cancel = std::sync::Arc::new(Mutex::new(None));
    let kept = std::sync::Arc::clone(&cancel);
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input, move |job| {
        *kept.lock().unwrap() = Some(job.cancel_signal.clone());
        spawn(&client, &launch, Some("exact-model".into()), job)
    }, None, None, None)
    .unwrap_or_else(|failure| panic!("{failure}"));
    let stored = stored_manifest(&manifest);
    assert_eq!(stored["status"], "running", "{stored}");
    assert_eq!(stored["toolCallId"], "toolu_c69472_ledger", "{stored}");
    assert_eq!(stored["parentSessionId"], "session-c69472", "{stored}");
    assert_eq!(stored["execution"], "ledger", "{stored}");

    // The worker is stopped, and its watch ends, before the store goes.
    cancel.lock().unwrap().take().expect("the launch's cancel signal").abort();
    let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(10));
    assert_eq!(results.len(), 1, "{results:?}");
}

/// The one completion a launch settles with, which must be a failure: its error.
fn settled_failure(manifest: &AgentOutput) -> String {
    let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(10));
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0].status, "failed", "{results:?}");
    results[0].error.clone().unwrap_or_default()
}

/// Adversarial review A (Z7): the window opening a worker's pane held the
/// parent's turn for the whole worker-start (8.6–9.8 s on 2026-10-27). The
/// start runs in the background now, so the `Agent` call hands back its
/// starting receipt at once. The fixture answers worker-start after ten
/// seconds unless the test lets it go, which it does once the call returned.
#[cfg(unix)]
#[test]
fn an_agent_call_returns_before_a_slow_worker_start_answers() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "slow_start", true);
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","agent":"codex","model":"exact-model","effort":"low","worktree":true})).unwrap();
    input.launch.selected = true;
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-slow-start", &root.path().join("store")));
    let launch = input.launch.clone();
    let began = Instant::now();
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
        move |job| spawn(&client, &launch, Some("exact-model".into()), job), None, None, None)
        .unwrap_or_else(|failure| panic!("{failure}"));
    let took = began.elapsed();
    std::fs::write(root.path().join("release"), "go").unwrap();
    assert!(took < Duration::from_secs(1), "the Agent call waited {took:?} for the worker-start");
    // The number a hand-in quotes: `cargo test ... -- --nocapture` prints it.
    println!("the Agent call returned in {} ms", took.as_millis());
    assert_eq!(manifest.status, "running");
    let start = Start { run: "run-1".into(), task: "t-4".into(), retry_request: format!("zo-start-{}", manifest.agent_id) };
    assert_eq!(manifest.lifecycle.ledger_start.as_ref(), Some(&start), "the starting receipt");
    assert!(manifest.lifecycle.ledger.is_none(), "no worker is bound yet");
    let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(20));
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0].status, "completed", "{results:?}");
    let stored = stored_manifest(&manifest);
    assert!(stored.get("ledgerStart").is_none(), "the bound worker left its starting receipt behind: {stored}");
    assert_eq!(stored["ledger"]["worker"], "w-2", "{stored}");
    // The tool result names the start, and nothing else of the launch.
    let receipt: Value = serde_json::from_str(&crate::misc_tools::running_background_agent_result(&manifest, None).unwrap()).unwrap();
    super::super::clear_background_agent(&manifest.agent_id);
    assert_eq!(receipt["ledgerStart"], json!({"run":"run-1","task":"t-4","retryRequest":start.retry_request}), "{receipt}");
}

/// A worker-start the window refuses reaches the parent through the
/// completion channel every agent settles through: the `Agent` call has
/// already returned its receipt by then.
#[cfg(unix)]
#[test]
fn a_refused_worker_start_comes_back_through_the_completion_channel() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "start_fails", true);
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","agent":"codex","model":"exact-model","effort":"low","worktree":true})).unwrap();
    input.launch.selected = true;
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("ledger-start-fails", &root.path().join("store")));
    let launch = input.launch.clone();
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
        move |job| spawn(&client, &launch, Some("exact-model".into()), job), None, None, None)
        .unwrap_or_else(|failure| panic!("a refused start still hands back the starting receipt: {failure}"));
    let failure = settled_failure(&manifest);
    assert!(failure.contains("could not open a pane for this agent"), "{failure}");
    assert!(!root.path().join("started").exists());
    assert!(!root.path().join("stopped").exists(), "no worker started, so none is stopped");
    let stored = stored_manifest(&manifest);
    assert_eq!(stored["status"], "failed", "{stored}");
    assert!(stored.get("ledgerStart").is_none(), "a failed start left its starting receipt behind: {stored}");
}

/// A launch that has no ledger start in flight writes no key for one: the
/// manifest and the background tool result are what they were before the
/// start went to the background.
#[test]
fn without_a_ledger_start_neither_the_manifest_nor_the_tool_result_names_one() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","subagent_type":"general-purpose"})).unwrap();
    input.registry = Some(super::super::AgentRegistry::at_root_for_tests("session-no-start", &root.path().join("store")));
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input, |_job| Ok(()), None, None, None)
        .unwrap_or_else(|failure| panic!("{failure}"));
    let stored = stored_manifest(&manifest);
    assert!(stored.get("ledgerStart").is_none(), "{stored}");
    let receipt: Value = serde_json::from_str(&crate::misc_tools::running_background_agent_result(&manifest, None).unwrap()).unwrap();
    super::super::clear_background_agent(&manifest.agent_id);
    let keys: std::collections::BTreeSet<&str> = receipt.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["agentId", "background", "name", "note", "outputFile", "status", "subagentType"].into_iter().collect(), "{receipt}");
}

/// A stop that wins while the worker is starting keeps its status. The start
/// no longer runs under the launch's manifest lock, so it finishes, finds the
/// agent stopped, stops the worker it brought up, and settles through the
/// completion channel with no starting receipt and no binding left behind.
#[cfg(unix)]
#[test]
fn a_stop_while_the_worker_starts_keeps_its_status_and_stops_that_worker() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = tempfile::tempdir().unwrap();
    let client = fake_door(root.path(), "slow_start", true);
    let session = "session-stop-while-starting";
    let registry = super::super::AgentRegistry::at_root_for_tests(session, &root.path().join("store"));
    let mut input: AgentInput = serde_json::from_value(json!({"description":"survey fixture","prompt":"report the fixture","agent":"codex","model":"exact-model","effort":"low","worktree":true})).unwrap();
    input.launch.selected = true;
    input.parent_session_id = Some(session.into());
    input.registry = Some(std::sync::Arc::clone(&registry));
    let launch = input.launch.clone();
    let manifest = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
        move |job| spawn(&client, &launch, Some("exact-model".into()), job), None, None, None)
        .unwrap_or_else(|failure| panic!("{failure}"));
    let outcome = super::super::stop_agent_for_session(&registry, &manifest.agent_id, session, "the parent stopped it while it started");
    assert!(matches!(outcome, super::super::AgentStopOutcome::Stopped { .. }), "{outcome:?}");
    std::fs::write(root.path().join("release"), "go").unwrap();
    let results = super::super::wait_for_agent_completions(std::slice::from_ref(&manifest.agent_id), Duration::from_secs(20));
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0].status, "stopped", "{results:?}");
    // The stop's own fallback may publish first; the start settles a moment later.
    let until = Instant::now() + Duration::from_secs(10);
    let mut stored = stored_manifest(&manifest);
    while (stored.get("ledgerStart").is_some() || !root.path().join("stopped").exists()) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
        stored = stored_manifest(&manifest);
    }
    assert!(root.path().join("stopped").exists(), "the worker the start brought up was left running");
    assert_eq!(stored["status"], "stopped", "{stored}");
    assert!(stored.get("ledgerStart").is_none(), "{stored}");
    assert!(stored.get("ledger").is_none(), "a stopped agent was bound to the worker it never used: {stored}");
}
