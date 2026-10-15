//! How the manager watches the server it owns, against fake runtimes only: a
//! runtime's end is told when it happens, not when a status read finds it,
//! and how it ended stays with the stopped state and on disk — the stop that
//! fired or the runtime's own exit, its code or signal, and the last of its
//! stderr, bounded and scrubbed.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::Manager;
use crate::engine::{Engine, Limits};
use crate::manager::{EngineFactory, test_support};

/// A runtime that floods its stderr before it says it is ready, then waits
/// for the file `end` and ends the way that file says: `code N` exits with N,
/// `signal N` sends itself signal N, `mute` closes its output and runs on,
/// `orphan N` leaves a process holding its output open for N seconds and
/// exits with 7. What the file `stderr` holds is the last it writes there.
const RUNTIME: &str = r#"import os,sys,time
root=sys.argv[1]
sys.stderr.write('x'*(1<<20)+'\n')
print('{"ready":true}',flush=True)
end=os.path.join(root,'end')
while not os.path.exists(end): time.sleep(0.005)
sys.stderr.write(open(os.path.join(root,'stderr')).read())
sys.stderr.flush()
how,_,value=open(end).read().partition(' ')
if how=='code': os._exit(int(value))
if how=='signal': os.kill(os.getpid(),int(value))
if how=='mute': os.close(1)
if how=='orphan':
 import subprocess
 subprocess.Popen(['sleep',value])
 os._exit(7)
time.sleep(60)
"#;

/// How long a step the manager takes by itself may take on a slow machine.
const SETTLE: Duration = Duration::from_secs(10);

/// What a stopped server keeps of its runtime's stderr at most.
const TAIL_LINES: usize = 20;
const TAIL_BYTES: usize = 4 * 1024;
const LINE_BYTES: usize = 512;

/// Every snapshot the manager told by itself, as the window receives it.
type Told = Arc<Mutex<Vec<Value>>>;

fn telling() -> (Told, Arc<dyn Fn(crate::Snapshot) + Send + Sync>) {
    let told = Told::default();
    let sink = told.clone();
    (
        told,
        Arc::new(move |snapshot: crate::Snapshot| {
            sink.lock()
                .unwrap()
                .push(serde_json::to_value(snapshot).unwrap());
        }),
    )
}

/// The first failed state the manager told for `server` — never a status
/// read of the test's own.
async fn told_failed(told: &Told, server: &str) -> Value {
    let began = Instant::now();
    loop {
        if let Some(failed) = told.lock().unwrap().iter().find(|snapshot| {
            snapshot["server"]["id"] == server && snapshot["server"]["state"] == "failed"
        }) {
            return failed.clone();
        }
        assert!(
            began.elapsed() < SETTLE,
            "the manager told no stopped state by itself within {SETTLE:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

struct Supervised {
    _home: tempfile::TempDir,
    control: tempfile::TempDir,
    data: PathBuf,
    manager: Arc<Manager>,
    model: crate::Model,
    server: String,
    told: Told,
}

impl Drop for Supervised {
    fn drop(&mut self) {
        self.manager.shutdown();
    }
}

/// A manager running the fake runtime as its started server.
async fn supervised() -> Supervised {
    let home = tempfile::tempdir().unwrap();
    let control = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let data = home.path().join("data");
    let model = crate::tests::fixture_model();
    let root = control.path().to_owned();
    let factory: EngineFactory = Arc::new(move |_| {
        Ok(Engine::from_command(
            Command::new("python3")
                .args(["-I", "-u", "-c", RUNTIME])
                .arg(&root),
        ))
    });
    let (told, sink) = telling();
    let manager =
        test_support::installed_manager(&data, &settings, vec![model.clone()], factory, sink);
    manager.start(&model.id, &model.revision).unwrap();
    let started = crate::tests::settled_snapshot(&manager).await;
    assert_eq!(
        started.server.state, "ready",
        "a runtime that floods its stderr still starts"
    );
    let server = started.server.id.unwrap();
    Supervised {
        _home: home,
        control,
        data,
        manager,
        model,
        server,
        told,
    }
}

impl Supervised {
    /// Ends the runtime as `how` says, after it writes `stderr`.
    fn end(&self, how: &str, stderr: &str) {
        fs::write(self.control.path().join("stderr"), stderr).unwrap();
        let pending = self.control.path().join("end.partial");
        fs::write(&pending, how).unwrap();
        fs::rename(pending, self.control.path().join("end")).unwrap();
    }

    async fn told_failed(&self) -> Value {
        told_failed(&self.told, &self.server).await
    }

    /// The exit records the store keeps, one JSON object a line.
    fn kept(&self) -> String {
        fs::read_to_string(self.data.join("managed-judgment/server-exits.jsonl"))
            .unwrap_or_default()
    }
}

fn tail(exit: &Value) -> Vec<String> {
    exit["stderr"]
        .as_array()
        .unwrap_or_else(|| panic!("no stderr tail: {exit}"))
        .iter()
        .map(|line| line.as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_that_ends_by_itself_is_told_at_once_with_its_code_and_a_scrubbed_bounded_tail() {
    let fixture = supervised().await;
    // Built here, so no private value stands in this file for the gate to find.
    let credential = format!("ghp_{}", "Q".repeat(36));
    let named = format!("{}={}", "api_key", "Zq7".repeat(8));
    let home = format!("/{}/{}/models/fixture", "Users", "zqowner");
    let mut stderr = String::new();
    for line in 0..60 {
        stderr.push_str(&format!("warning {line}: loading\n"));
    }
    stderr.push_str(&format!("{} {}\n", "w".repeat(300), "v".repeat(400)));
    stderr.push_str(&format!("loaded {credential} from {home}\n"));
    stderr.push_str(&format!("config {named}\n"));
    stderr.push_str("fixture: last words\n");
    fixture.end("code 7", &stderr);

    let failed = fixture.told_failed().await;
    let exit = &failed["server"]["exit"];
    assert_eq!(exit["cause"], "runtime_exited", "{failed}");
    assert_eq!(exit["code"], 7);
    assert_eq!(exit["signal"], Value::Null);
    assert_eq!(failed["server"]["provider"], Value::Null);
    let tail = tail(exit);
    assert_eq!(tail.last().map(String::as_str), Some("fixture: last words"));
    assert!(tail.len() <= TAIL_LINES, "{tail:?}");
    assert!(tail.iter().map(String::len).sum::<usize>() <= TAIL_BYTES);
    assert!(tail.iter().all(|line| line.len() <= LINE_BYTES), "{tail:?}");
    assert!(
        tail.iter()
            .any(|line| line.starts_with(&"w".repeat(300)) && !line.contains('v')),
        "a long line is cut at a word, and the word cut through is dropped: {tail:?}"
    );
    let said = tail.join("\n");
    for private in [credential.as_str(), "Zq7Zq7", "zqowner"] {
        assert!(
            !said.contains(private),
            "{private} reached the tail: {said}"
        );
    }
    assert!(said.contains("[redacted]"), "{said}");
    assert!(
        said.contains(&format!("/{}/dev/models/fixture", "Users")),
        "{said}"
    );

    let kept = fixture.kept();
    let record: Value = serde_json::from_str(kept.lines().last().unwrap_or("null")).unwrap();
    assert_eq!(record["cause"], "runtime_exited", "{kept}");
    assert_eq!(record["code"], 7);
    assert_eq!(record["serverId"], fixture.server.as_str());
    assert_eq!(record["modelId"], fixture.model.id.as_str());
    assert_eq!(record["stderr"], exit["stderr"]);
    for private in [credential.as_str(), "Zq7Zq7", "zqowner"] {
        assert!(!kept.contains(private), "{private} reached the disk record");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_memory_guards_own_exit_is_kept_as_the_guards_stop() {
    let fixture = supervised().await;
    fixture.end("code 75", "");
    let failed = fixture.told_failed().await;
    let exit = &failed["server"]["exit"];
    assert_eq!(exit["cause"], "runtime_memory_limit", "{failed}");
    assert_eq!(exit["code"], 75);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kill_this_side_never_sent_is_kept_as_the_runtimes_own_end() {
    let fixture = supervised().await;
    fixture.end("signal 9", "");
    let failed = fixture.told_failed().await;
    let exit = &failed["server"]["exit"];
    assert_eq!(exit["cause"], "runtime_exited", "{failed}");
    assert_eq!(exit["signal"], 9);
    assert_eq!(exit["code"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_that_closes_its_output_and_runs_on_is_stopped_as_a_runtime_failure() {
    let fixture = supervised().await;
    fixture.end("mute", "");
    let failed = fixture.told_failed().await;
    let exit = &failed["server"]["exit"];
    assert_eq!(exit["cause"], "runtime_failed", "{failed}");
    assert_eq!(exit["signal"], 9);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_whose_output_another_process_holds_open_is_still_told_when_it_ends() {
    let fixture = supervised().await;
    // The process left behind outlives the wait for the stopped state, so only
    // the runtime's own end can tell it.
    fixture.end("orphan 15", "");
    let failed = fixture.told_failed().await;
    let exit = &failed["server"]["exit"];
    assert_eq!(exit["cause"], "runtime_exited", "{failed}");
    assert_eq!(exit["code"], 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_stop_keeps_the_task_its_request_named_and_never_free_text() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let (told, sink) = telling();
    let manager = test_support::installed_manager(
        &home.path().join("data"),
        &settings,
        vec![crate::tests::fixture_model()],
        Arc::new(|_| panic!("a deadline stop never starts a runtime")),
        sink,
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for (task, kept) in [
        (json!("stall-seat"), json!("stall-seat")),
        (json!("a task written out in words"), Value::Null),
    ] {
        let engine = Engine::from_command(
            Command::new("sh").args(["-c", "while IFS= read -r line; do :; done"]),
        );
        let (server, provider) = test_support::running_limited(
            &manager,
            engine,
            Limits {
                request: Duration::from_millis(300),
                queue_wait: SETTLE,
                waiting: 1,
            },
        )
        .await;
        let response = client
            .post(provider.request_url())
            .json(&json!({
                "model": provider.model,
                "state": "fixture",
                "questions": {"only": {"type": "noul", "instructions": "fixture"}},
                "task": task,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        let failed = told_failed(&told, &server).await;
        let exit = &failed["server"]["exit"];
        assert_eq!(exit["cause"], "runtime_timeout", "{failed}");
        assert_eq!(exit["task"], kept, "{failed}");
        assert_eq!(exit["signal"], 9);
    }
    manager.shutdown();
}
