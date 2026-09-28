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
event = {'observed_death':'worker_died','observed_quota':'quota_walled','observed_done':'worker_done','ack_failure':'worker_done'}.get(kind,kind)
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
        assert!(!args.contains("tmux"));
        assert_eq!(manifest.lifecycle.execution.as_deref(), Some("ledger"));
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
    let failure = super::super::execute_agent_with_spawn_and_parent_model_and_hooks(input,
        move |job| spawn(&client, &launch, Some("requested-model".into()), job), None, None, None).unwrap_err();
    assert!(failure.to_string().contains("differs from the requested"), "{failure}");
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
