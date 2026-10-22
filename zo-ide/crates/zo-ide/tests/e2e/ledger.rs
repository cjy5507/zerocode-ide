use super::*;
use std::os::unix::fs::PermissionsExt;
use serde_json::{Value, json};

fn write_ledger_door(directory: &std::path::Path, expected: &Value) {
    fs::create_dir_all(directory).unwrap();
    let door = directory.join("zerocode-orc");
    fs::write(&door, r"#!/usr/bin/python3
import json, os, sys
from pathlib import Path
base = Path(__file__).parent
args = sys.argv[1:]
assert 'TMUX' not in os.environ and 'TMUX_PANE' not in os.environ
expected = json.loads((base/'expected.json').read_text())
assert os.environ['ZEROCODE_AGENT_TEAM_ID'] == 'fixture-team'
assert os.environ['ZEROCODE_AGENT_TEAM_PANE'] == '%1'
if expected['tokenFile'] is None:
    assert os.environ['ZEROCODE_AGENT_TEAM_TOKEN'] == 'fixture-grant'
    assert 'ZEROCODE_AGENT_TEAM_TOKEN_FILE' not in os.environ
else:
    assert 'ZEROCODE_AGENT_TEAM_TOKEN' not in os.environ
    assert os.environ['ZEROCODE_AGENT_TEAM_TOKEN_FILE'] == expected['tokenFile']
with open(base / 'calls.jsonl', 'a') as f: f.write(json.dumps(args)+'\n')
verb = args[0]
if verb == 'agent-list': out={'agents':[{'id':expected['agent'],'takesModel':True,'takesEffort':True,'installed':'yes'}]}
elif verb == 'run-current': out={'runId':None}
elif verb == 'run-create': out={'runId':'run-fixture'}
elif verb == 'task-create': out={'taskId':'t-fixture'}
elif verb == 'dispatch-show': out={'dispatchId':None,'lifecycle':[]}
elif verb == 'worker-start':
    assert args[args.index('--agent')+1] == expected['agent']
    assert args[args.index('--model')+1] == expected['model']
    assert args[args.index('--effort')+1] == expected['effort']
    assert ('--worktree' in args) == expected['worktree']
    out={'workerId':'w-fixture','dispatchId':'dp-fixture','pane':'%2','worktree':expected['worktree'],'agent':expected['agent'],'model':expected['model'],'effort':expected['effort']}
elif verb == 'check':
    if '--ack' in args:
        (base/'acked').write_text('yes')
        out={'messages':[]}
    elif (base/'acked').exists(): out={'messages':[]}
    else:
        out={'deliveryId':'delivery-fixture','messages':[{'messageId':'m-fixture','from':'worker:w-fixture','source':'agent','trust':'data','dispatchId':'dp-fixture','type':'worker_done','body':json.dumps({'ok':True,'summary':'fixture ledger final report'})}]}
else: raise Exception(args)
if verb in ('run-create','task-create','worker-start') or '--ack' in args: assert '--retry-request' in args
print(json.dumps(out))
").unwrap();
    fs::set_permissions(&door, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(directory.join("expected.json"), expected.to_string()).unwrap();
}

fn assert_one_launch(directory: &std::path::Path, output: &std::process::Output, context: &str) {
    let calls = fs::read_to_string(directory.join("calls.jsonl")).unwrap_or_default();
    let calls: Vec<Value> = calls.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(calls.iter().filter(|args| args[0] == "worker-start").count(), 1,
        "{context}: {calls:?}; {}", String::from_utf8_lossy(&output.stdout));
    assert!(directory.join("acked").exists(), "completion was not acknowledged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_delegation_tools_summon_through_a_hermetic_ledger_door() {
    let member = json!({"description":"survey fixture", "prompt":"report fixture", "agent":"codex", "model":"exact-model", "effort":"low", "worktree":true, "background":false});
    for (tool, input) in [
        ("Agent", member.clone()),
        ("SpawnMultiAgent", json!({"agents":[member]})),
        ("Workflow", json!({"name":"ledger fixture","phases":[{"id":"survey","prompt":"report fixture","agent":"codex","model":"exact-model","effort":"low","worktree":true}]})),
    ] {
        let layout = Layout::new();
        layout.model_led();
        write_ledger_door(layout.root.path(), &json!({"agent":"codex", "model":"exact-model", "effort":"low", "worktree":true, "tokenFile":null}));
        let fake_path = format!("{}:/usr/bin:/bin", layout.root.path().display());
        let service = ScriptedAnthropicService::ledger_spawn(tool, input).await.unwrap();
        let output = run_pipe_with_env(&layout.cwd, &layout.home, &layout.sessions, &layout.state, service.base_url(),
            &["--permission-mode", "danger-full-access"], b"Delegate the fixture survey\n",
            &[("PATH", &fake_path), ("ZEROCODE_AGENT_TEAM_ID", "fixture-team"), ("ZEROCODE_AGENT_TEAM_PANE", "%1"), ("ZEROCODE_AGENT_TEAM_TOKEN", "fixture-grant")]).unwrap();
        assert!(output.status.success(), "{tool}: {}", String::from_utf8_lossy(&output.stderr));
        assert_one_launch(layout.root.path(), &output, "environment-grant ledger should launch once");
    }
}

/// A hand-started zo has only the pane and endpoint pointers. Its own grant
/// arrives later by file, and the installed door is outside the shell's PATH.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_a_hand_started_agent_uses_its_own_file_grant_and_installed_door() {
    let layout = Layout::new();
    layout.model_led();
    let application = layout.root.path().join("fixture's application data");
    let hooks = application.join("agent-hooks");
    fs::create_dir_all(hooks.join("panes")).unwrap();
    let endpoint = hooks.join("endpoint.env");
    fs::write(&endpoint, "ZEROCODE_HOOK_PORT=0\n").unwrap();
    let token_file = application.join("private-team-token");
    fs::write(&token_file, "fixture-grant").unwrap();
    let quoted_path = token_file.to_string_lossy().replace('\'', "'\\''");
    fs::write(hooks.join("panes/term-7.env"), format!(
        "ZEROCODE_AGENT_TEAM_ID='fixture-team'\nZEROCODE_AGENT_TEAM_PANE='%1'\nZEROCODE_AGENT_TEAM_TOKEN_FILE='{quoted_path}'\n"
    )).unwrap();
    let door = application.join("orchestration-bin");
    write_ledger_door(&door, &json!({"agent":"zo", "model":"exact-model", "effort":"high", "worktree":false, "tokenFile":token_file}));
    let input = json!({"description":"survey fixture", "prompt":"report fixture", "model":"exact-model", "effort":"high", "worktree":false, "background":false});
    let service = ScriptedAnthropicService::ledger_spawn("Agent", input).await.unwrap();
    let endpoint = endpoint.to_string_lossy();
    let output = run_pipe_with_env(&layout.cwd, &layout.home, &layout.sessions, &layout.state, service.base_url(),
        &["--permission-mode", "danger-full-access"], b"Delegate the fixture survey\n",
        &[("PATH", "/usr/bin:/bin"), ("ZEROCODE_HOOK_ENDPOINT", endpoint.as_ref()), ("ZEROCODE_PANE_KEY", "term-7")]).unwrap();
    assert_one_launch(&door, &output, "pane-file-grant ledger should launch once");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}
