use super::*;
use std::os::unix::fs::PermissionsExt;
use serde_json::{Value, json};

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
        let door = layout.root.path().join("zerocode-orc");
        fs::write(&door, r"#!/usr/bin/python3
import json, os, sys
from pathlib import Path
base = Path(__file__).parent
args = sys.argv[1:]
assert 'TMUX' not in os.environ and 'TMUX_PANE' not in os.environ
assert os.environ['ZEROCODE_AGENT_TEAM_TOKEN'] == 'fixture-grant'
assert 'ZEROCODE_AGENT_TEAM_TOKEN_FILE' not in os.environ
with open(base / 'calls.jsonl', 'a') as f: f.write(json.dumps(args)+'\n')
verb = args[0]
if verb == 'agent-list': out={'agents':[{'id':'codex','takesModel':True,'takesEffort':True,'installed':'yes'}]}
elif verb == 'run-current': out={'runId':None}
elif verb == 'run-create': out={'runId':'run-fixture'}
elif verb == 'task-create': out={'taskId':'t-fixture'}
elif verb == 'dispatch-show': out={'dispatchId':None,'lifecycle':[]}
elif verb == 'worker-start':
    assert args[args.index('--agent')+1] == 'codex'
    assert args[args.index('--model')+1] == 'exact-model'
    assert args[args.index('--effort')+1] == 'low'
    assert '--worktree' in args
    out={'workerId':'w-fixture','dispatchId':'dp-fixture','pane':'%2','worktree':True,'agent':'codex','model':'exact-model','effort':'low'}
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
        let fake_path = format!("{}:/usr/bin:/bin", layout.root.path().display());
        let service = ScriptedAnthropicService::ledger_spawn(tool, input).await.unwrap();
        let output = run_pipe_with_env(&layout.cwd, &layout.home, &layout.sessions, &layout.state, service.base_url(),
            &["--permission-mode", "danger-full-access"], b"Delegate the fixture survey\n",
            &[("PATH", &fake_path), ("ZEROCODE_AGENT_TEAM_ID", "fixture-team"), ("ZEROCODE_AGENT_TEAM_PANE", "%1"), ("ZEROCODE_AGENT_TEAM_TOKEN", "fixture-grant")]).unwrap();
        assert!(output.status.success(), "{tool}: {}", String::from_utf8_lossy(&output.stderr));
        let calls = fs::read_to_string(layout.root.path().join("calls.jsonl")).unwrap_or_default();
        let calls: Vec<Value> = calls.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(calls.iter().filter(|args| args[0] == "worker-start").count(), 1, "{tool}: {calls:?}; {}", String::from_utf8_lossy(&output.stdout));
        assert!(layout.root.path().join("acked").exists(), "{tool}: completion was not acknowledged");
    }
}
