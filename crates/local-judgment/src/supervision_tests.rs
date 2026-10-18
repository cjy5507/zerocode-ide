//! How the manager watches the server it owns, against fake runtimes only: a
//! runtime's end is told when it happens, not when a status read finds it,
//! and how it ended stays with the stopped state and on disk — the stop that
//! fired or the runtime's own exit, its code or signal, and the last of its
//! stderr, bounded and scrubbed. A server the person's 「실행하고 적용」 still
//! wants comes back on its own port within its budget; nothing else does.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use judgment_provider::{Protocol, Provider, ProviderKind};
use serde_json::{Value, json};

use crate::engine::{Engine, Limits};
use crate::manager::{Clock, EngineFactory, test_support};
use crate::{Binding, Manager};

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_stop_keeps_a_seat_name_as_its_task_but_never_a_key_a_token_a_path_or_a_sentence()
 {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let data = home.path().join("data");
    let (told, sink) = telling();
    let manager = test_support::installed_manager(
        &data,
        &settings,
        vec![crate::tests::fixture_model()],
        Arc::new(|_| panic!("a deadline stop never starts a runtime")),
        sink,
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    // Built here, so no private value stands in this file for the gate to find.
    let cases = [
        (
            "home path",
            format!("/{}/{}/models/fixture", "Users", "zqowner"),
        ),
        (
            "bearer token",
            format!("{} {}", "Bearer", "tq8Vw".repeat(6)),
        ),
        (
            "allowlisted key",
            format!("{}-{}", "sk-ant-api03", "Kp9".repeat(10)),
        ),
        (
            "request sentence",
            "Only my sister knows the harbor plan for Friday".to_owned(),
        ),
        ("seat name", "rerank_shadow".to_owned()),
    ];
    let mut kept = Vec::new();
    for (case, task) in &cases {
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
        assert_eq!(
            response.status(),
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "{case}"
        );
        let failed = told_failed(&told, &server).await;
        assert_eq!(
            failed["server"]["exit"]["cause"], "runtime_timeout",
            "{case}: {failed}"
        );
        kept.push(failed["server"]["exit"]["task"].clone());
    }
    manager.shutdown();
    let wanted = [
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
        json!("rerank_shadow"),
    ];
    let wrong: Vec<&str> = cases
        .iter()
        .zip(kept.iter().zip(&wanted))
        .filter(|(_, (kept, wanted))| kept != wanted)
        .map(|((case, _), _)| *case)
        .collect();
    assert_eq!(
        wrong,
        Vec::<&str>::new(),
        "a deadline stop kept as its task something other than a plain seat name: {kept:?}"
    );
    let record =
        fs::read_to_string(data.join("managed-judgment/server-exits.jsonl")).unwrap_or_default();
    let said = serde_json::to_string(&*told.lock().unwrap()).unwrap();
    for (case, task) in &cases[..4] {
        assert!(
            !record.contains(task.as_str()),
            "the {case} reached server-exits.jsonl"
        );
        assert!(
            !said.contains(task.as_str()),
            "the {case} reached the snapshot the card reads"
        );
    }
    assert!(
        record.contains("rerank_shadow") && said.contains("rerank_shadow"),
        "the seat name was not kept with its deadline stop"
    );
}

/// A runtime that answers System One requests as the native fixture does, N
/// being which start of a server it is. With the file `hold-N` it says it is
/// ready only once `go-N` appears. It ends when `end-N` appears: `code C`
/// exits with C, `signal S` sends itself S. It never answers a request whose
/// task is `stall-seat`, as the runtime behind the 19:35 deadline stop did.
/// With the file `leak-N` it writes that file and the state it was asked
/// about to stderr and exits with 1, as a runtime that dies repeating its
/// input does. With the file `fail-N` it exits with 3 before it is ready.
/// With the file `say-N` it writes that file to stderr as it starts. With the
/// file `echo-N` it writes the state each request asks about to stderr: as
/// it answers when the file says `now`, and only once the next request comes
/// in when it says `late`.
const ANSWERING: &str = r#"import json,os,sys,threading,time
root,n=sys.argv[1],sys.argv[2]
def at(name): return os.path.join(root,name+'-'+n)
def watch():
 while not os.path.exists(at('end')): time.sleep(0.005)
 how,_,value=open(at('end')).read().partition(' ')
 if how=='code': os._exit(int(value))
 os.kill(os.getpid(),int(value))
threading.Thread(target=watch,daemon=True).start()
if os.path.exists(at('fail')): os._exit(3)
if os.path.exists(at('say')): sys.stderr.write(open(at('say')).read());sys.stderr.flush()
if os.path.exists(at('hold')):
 while not os.path.exists(at('go')): time.sleep(0.005)
print('{"ready":true}',flush=True)
late=''
for line in sys.stdin:
 r=json.loads(line);a={}
 if late: sys.stderr.write(late);sys.stderr.flush();late=''
 if os.path.exists(at('echo')):
  said='judging: '+str(r.get('state'))+'\n'
  if open(at('echo')).read()=='late': late=said
  else: sys.stderr.write(said);sys.stderr.flush()
 if r.get('task')=='stall-seat':
  while True: time.sleep(60)
 if os.path.exists(at('leak')):
  sys.stderr.write(open(at('leak')).read()+'could not judge: '+str(r.get('state'))+'\n');sys.stderr.flush();os._exit(1)
 for k,q in r['questions'].items():
  if q['type']=='noul': a[k]={'type':'noul','noul':0.9}
  elif q['type']=='choice':
   keys=list(q['criteria']);p={x:(1.0 if i==0 else 0.0) for i,x in enumerate(keys)};a[k]={'type':'choice','choice':keys[0],'confidence':1.0,'probabilities':p}
  else:
   c=len(q['criteria']);p={str(i):(1.0 if i==0 else 0.0) for i in range(c)};a[k]={'type':'score','score':0.0,'confidence':1.0,'probabilities':p,'legend':{str(i):v for i,v in enumerate(q['criteria'])}}
 print(json.dumps({'model':r['model'],'answers':a,'usage':{'input_tokens':1,'output_tokens':0}}),flush=True)
"#;

const SECOND: Duration = Duration::from_secs(1);
const HOUR: Duration = Duration::from_secs(60 * 60);

/// The request deadline the 19:35 reproduction serves under: a stalled
/// request always misses it, and a fixture answer never does.
const STALL: Duration = Duration::from_secs(2);

/// A clock that moves only when the test moves it. It keeps every wait the
/// manager asked of it and runs a restart once its wait has passed, so no
/// test sleeps through a wait.
#[derive(Clone)]
struct Manual(Arc<Mutex<Hands>>);

struct Hands {
    start: Instant,
    moved: Duration,
    due: Vec<(Duration, Box<dyn FnOnce() + Send>)>,
    asked: Vec<Duration>,
}

impl Manual {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Hands {
            start: Instant::now(),
            moved: Duration::ZERO,
            due: Vec::new(),
            asked: Vec::new(),
        })))
    }

    /// Moves the clock on by `by`, running every restart whose wait has
    /// passed.
    fn advance(&self, by: Duration) {
        let due = {
            let mut hands = self.0.lock().unwrap();
            hands.moved += by;
            let now = hands.moved;
            let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut hands.due)
                .into_iter()
                .partition(|(at, _)| *at <= now);
            hands.due = later;
            due
        };
        for (_, run) in due {
            run();
        }
    }

    /// Every wait the manager asked for, in order.
    fn asked(&self) -> Vec<Duration> {
        self.0.lock().unwrap().asked.clone()
    }
}

impl Clock for Manual {
    fn now(&self) -> Instant {
        let hands = self.0.lock().unwrap();
        hands.start + hands.moved
    }

    fn after(
        &self,
        _runtime: &tokio::runtime::Handle,
        wait: Duration,
        due: Box<dyn FnOnce() + Send>,
    ) {
        let mut hands = self.0.lock().unwrap();
        let at = hands.moved + wait;
        hands.asked.push(wait);
        hands.due.push((at, due));
    }
}

/// A manager whose every server start runs [`ANSWERING`] as start N, which
/// waits for its restarts on a [`Manual`] clock, and which knows a second
/// model to change to.
struct Recovering {
    _home: tempfile::TempDir,
    control: tempfile::TempDir,
    settings: PathBuf,
    data: PathBuf,
    manager: Arc<Manager>,
    models: Vec<crate::Model>,
    starts: Arc<AtomicUsize>,
    told: Told,
    clock: Manual,
    client: reqwest::Client,
}

impl Drop for Recovering {
    fn drop(&mut self) {
        self.manager.shutdown();
    }
}

fn recovering() -> Recovering {
    let home = tempfile::tempdir().unwrap();
    let control = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    fs::write(&settings, b"{}").unwrap();
    let data = home.path().join("data");
    let model = crate::tests::fixture_model();
    let mut other = model.clone();
    other.id = "fixture-other".to_owned();
    let root = control.path().to_owned();
    let starts = Arc::new(AtomicUsize::new(0));
    let counted = starts.clone();
    let factory: EngineFactory = Arc::new(move |_| {
        let start = counted.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Engine::from_command(
            Command::new("python3")
                .args(["-I", "-u", "-c", ANSWERING])
                .arg(&root)
                .arg(start.to_string()),
        ))
    });
    let (told, sink) = telling();
    let models = vec![model, other];
    let manager = test_support::installed_manager(&data, &settings, models.clone(), factory, sink);
    let clock = Manual::new();
    test_support::use_clock(&manager, Arc::new(clock.clone()));
    Recovering {
        _home: home,
        control,
        settings,
        data,
        manager,
        models,
        starts,
        told,
        clock,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
    }
}

/// Writes the settings the window writes when a press applies `provider`:
/// the provider, the managed model it binds, and a key nothing else touches.
fn select(settings: &Path, provider: &Provider, binding: &Binding) {
    fs::write(
        settings,
        serde_json::to_vec(&json!({"smart":{
            "judgmentProvider": provider,
            (crate::MANAGED_MODEL_KEY): binding,
            "judgmentKey": "kept as the person wrote it",
        }}))
        .unwrap(),
    )
    .unwrap();
}

/// A System One request to `provider` about `state`, as a seat sends one.
fn question(provider: &Provider, state: &str) -> Value {
    json!({
        "model": provider.model,
        "state": state,
        "questions": {"only": {"type": "noul", "instructions": "fixture"}},
    })
}

fn address(provider: &Provider) -> String {
    provider.base_url.trim_start_matches("http://").to_owned()
}

/// Whether `address` stops taking connections within [`SETTLE`]: the port of
/// a server let go closes once its listener's task has ended.
async fn closes(address: &str) -> bool {
    let began = Instant::now();
    while began.elapsed() < SETTLE {
        if TcpStream::connect(address).is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

impl Recovering {
    fn model(&self) -> crate::Model {
        self.models[0].clone()
    }

    /// How many runtimes the manager has started so far.
    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    /// Waits until the manager has started `count` runtimes.
    async fn started(&self, count: usize) {
        let began = Instant::now();
        while self.starts() < count {
            assert!(
                began.elapsed() < SETTLE,
                "runtime {count} did not start within {SETTLE:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Starts `model` the way the window does before the press applies it.
    async fn start(&self, model: &crate::Model) -> crate::Snapshot {
        self.manager.start(&model.id, &model.revision).unwrap();
        let started = crate::tests::settled_snapshot(&self.manager).await;
        assert_eq!(
            started.server.state,
            "ready",
            "{:?}",
            started
                .operation
                .as_ref()
                .and_then(|operation| operation.error)
        );
        started
    }

    /// The rest of the person's 「실행하고 적용」 for the server that is up:
    /// its check, then the settings write the window makes.
    async fn apply(&self, server: &str) -> Provider {
        self.manager.check(server).unwrap();
        let checked = crate::tests::settled_snapshot(&self.manager).await;
        assert!(checked.server.checked, "{:?}", checked.server.error);
        let expected = checked.provider_identity.unwrap();
        let applied = self
            .manager
            .apply(server, &expected, |provider, _, binding| {
                select(&self.settings, provider, binding);
                Ok(())
            })
            .unwrap();
        assert_eq!(applied.applied_model_id, checked.server.model_id);
        applied.server.provider.unwrap()
    }

    /// The whole press on the first model: start, check, apply.
    async fn press(&self) -> (String, Provider) {
        let server = self.start(&self.model()).await.server.id.unwrap();
        let provider = self.apply(&server).await;
        (server, provider)
    }

    /// Writes the control file `file-N` for the runtime of start `start`.
    fn tell(&self, file: &str, start: usize, what: &str) {
        let pending = self.control.path().join(format!("{file}-{start}.partial"));
        fs::write(&pending, what).unwrap();
        fs::rename(pending, self.control.path().join(format!("{file}-{start}"))).unwrap();
    }

    /// Ends the runtime of start `start` as `how` says.
    fn end(&self, start: usize, how: &str) {
        self.tell("end", start, how);
    }

    /// The first state the manager told by itself for `server` once its
    /// runtime ended — what it made of that end, never a status read of the
    /// test's own.
    async fn ended(&self, server: &str) -> Value {
        let began = Instant::now();
        loop {
            if let Some(found) =
                self.told.lock().unwrap().iter().find(|told| {
                    told["server"]["id"] == server && !told["server"]["exit"].is_null()
                })
            {
                return found.clone();
            }
            assert!(
                began.elapsed() < SETTLE,
                "the manager told nothing of the end of {server} within {SETTLE:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// The server started again in place of `server`, once a status read
    /// shows it ready and its restart finished.
    async fn back(&self, server: &str) -> crate::Snapshot {
        let began = Instant::now();
        loop {
            let snapshot = self.manager.snapshot();
            if snapshot.server.state == "ready"
                && snapshot.server.id.as_deref().is_some_and(|id| id != server)
                && snapshot
                    .operation
                    .as_ref()
                    .is_none_or(|operation| operation.state != "running")
            {
                return snapshot;
            }
            assert!(
                began.elapsed() < SETTLE,
                "the ended server did not come back within {SETTLE:?}: {} {:?}",
                snapshot.server.state,
                snapshot
                    .operation
                    .as_ref()
                    .and_then(|operation| operation.error)
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Every record the store keeps of ended servers and of restart attempts.
    fn records(&self) -> Vec<Value> {
        fs::read_to_string(self.data.join("managed-judgment/server-exits.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The kept records of ended servers.
    fn exits(&self) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|record| record.get("kind").is_none())
            .collect()
    }

    /// What each kept record of an end says about its restart.
    fn recoveries(&self) -> Vec<Value> {
        self.exits()
            .into_iter()
            .map(|record| record["recovery"].clone())
            .collect()
    }

    /// The kept records of restart attempts.
    fn attempts(&self) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|record| record["kind"] == "restart")
            .collect()
    }

    /// The end of `server` wherever the window or the store keeps it, each
    /// place with the exit it holds and the whole of its text: the snapshots
    /// the manager told by itself, the store's server-exits.jsonl, and the
    /// status a read returns, which is what the window reads.
    async fn kept_end(&self, server: &str) -> Vec<(&'static str, Value, String)> {
        let told = self.ended(server).await["server"]["exit"].clone();
        let began = Instant::now();
        let record = loop {
            if let Some(record) = self
                .exits()
                .into_iter()
                .rfind(|record| record["serverId"] == server)
            {
                break record;
            }
            assert!(
                began.elapsed() < SETTLE,
                "the exit event for {server} reached no journal row within {SETTLE:?}"
            );
            tokio::task::yield_now().await;
        };
        let status = serde_json::to_value(self.manager.snapshot()).unwrap();
        vec![
            (
                "server.exit",
                told,
                serde_json::to_string(&*self.told.lock().unwrap()).unwrap(),
            ),
            (
                "server-exits.jsonl",
                record,
                fs::read_to_string(self.data.join("managed-judgment/server-exits.jsonl"))
                    .unwrap_or_default(),
            ),
            (
                "status",
                status["server"]["exit"].clone(),
                status.to_string(),
            ),
        ]
    }

    /// The 503 both routes of `provider`'s port answer: its Retry-After in
    /// whole seconds and its error word.
    async fn refusal(&self, provider: &Provider) -> (Option<u64>, Value) {
        let asked = self
            .client
            .post(provider.request_url())
            .json(&question(provider, "fixture"))
            .send()
            .await
            .unwrap();
        let listed = self
            .client
            .get(provider.api_url("models"))
            .send()
            .await
            .unwrap();
        let mut said = Vec::new();
        for response in [asked, listed] {
            assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
            let after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok());
            let body: Value = response.json().await.unwrap();
            said.push((after, body["error"].clone()));
        }
        assert_eq!(said[0], said[1], "the two routes refused differently");
        said.remove(0)
    }

    /// Sends the request a seat named `stall-seat` sent before the 19:35
    /// stop, which the runtime never answers.
    async fn stall(&self, provider: &Provider) -> reqwest::StatusCode {
        let mut body = question(provider, "fixture");
        body["task"] = json!("stall-seat");
        self.client
            .post(provider.request_url())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_persons_press_brings_an_ended_server_back_on_its_own_port_with_the_settings_untouched()
 {
    let fixture = recovering();
    let model = fixture.model();
    // A selection an earlier run of the app left behind: it names this
    // model, and nothing in this run asked for it.
    let earlier = Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: "http://127.0.0.1:9".to_owned(),
        model: model.id.clone(),
        requires_auth: false,
    };
    select(
        &fixture.settings,
        &earlier,
        &Binding {
            model_id: model.id.clone(),
            revision: model.revision.clone(),
            provider_identity: earlier.identity(),
        },
    );
    fixture.clock.advance(HOUR);
    let idle = fixture.manager.snapshot();
    assert_eq!(idle.configured_model_id, Some(model.id.clone()));
    assert_eq!(
        (
            fixture.starts(),
            idle.server.state,
            fixture.clock.asked().len()
        ),
        (0, "stopped", 0),
        "an old selection alone started a server"
    );

    // Started but never applied: its end stays an end.
    let unapplied = fixture.start(&model).await.server.id.unwrap();
    fixture.end(1, "code 7");
    let ended = fixture.ended(&unapplied).await;
    assert_eq!(ended["server"]["state"], "failed", "{ended}");
    fixture.clock.advance(HOUR);
    assert_eq!(
        (fixture.starts(), fixture.clock.asked().len()),
        (1, 0),
        "a server nobody applied started again"
    );

    // After the press its end is taken up: the same port and provider come
    // back, checked and applied, and the settings stay as the press wrote
    // them, their key and selection with them.
    let (server, provider) = fixture.press().await;
    let settings = fs::read(&fixture.settings).unwrap();
    fixture.end(2, "code 7");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the person's press applied was not taken up: {waiting}"
    );
    assert_eq!(fixture.clock.asked(), [SECOND]);
    fixture.clock.advance(SECOND);
    let back = fixture.back(&server).await;
    assert_eq!(fixture.starts(), 3);
    assert_eq!(
        back.server.provider.as_ref().map(Provider::identity),
        Some(provider.identity()),
        "the restart moved the port or the provider"
    );
    assert!(back.server.checked);
    assert_eq!(back.applied_model_id, Some(model.id.clone()));
    assert_eq!(
        fs::read(&fixture.settings).unwrap(),
        settings,
        "a restart wrote the settings"
    );
    // The card's reason and the store's record of the attempt say the same.
    assert_eq!(fixture.recoveries(), [Value::Null, json!("restart")]);
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(attempts[0]["cause"], waiting["server"]["exit"]["cause"]);
    assert_eq!(attempts[0]["waitMs"], 1000);
    assert_eq!(attempts[0]["result"], "ready");
    assert_eq!(attempts[0]["serverId"], server.as_str());
    assert_eq!(attempts[0]["generation"], fixture.exits()[1]["generation"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_persons_stop_another_models_press_a_new_press_or_app_exit_wins_over_a_planned_restart()
{
    for how in [
        "stop",
        "stop while it loads",
        "another model's press",
        "new press",
        "app exit",
    ] {
        let fixture = recovering();
        let (server, provider) = fixture.press().await;
        fixture.end(1, "code 7");
        let waiting = fixture.ended(&server).await;
        assert_eq!(
            waiting["server"]["state"], "starting",
            "{how}: the end of the server the person's press applied was not taken up: {waiting}"
        );
        match how {
            "stop" => {
                let stopped = fixture
                    .manager
                    .stop(&server)
                    .expect("the person's stop was refused while a restart waited");
                assert_eq!(stopped.server.state, "stopped");
            }
            "stop while it loads" => {
                fixture.tell("hold", 2, "");
                fixture.clock.advance(SECOND);
                fixture.started(2).await;
                let stopped = fixture
                    .manager
                    .stop(&server)
                    .expect("the person's stop was refused while a restart loaded");
                assert_eq!(stopped.server.state, "stopped");
            }
            "another model's press" => {
                fixture.start(&fixture.models[1]).await;
            }
            "new press" => {
                fixture.press().await;
            }
            _ => fixture.manager.shutdown(),
        }
        let starts = fixture.starts();
        fixture.clock.advance(HOUR);
        let after = fixture.manager.snapshot();
        assert_eq!(fixture.starts(), starts, "{how}: a restart ran after it");
        assert!(
            after
                .operation
                .as_ref()
                .is_none_or(|operation| operation.state != "running"),
            "{how}: a restart began after it"
        );
        match how {
            "another model's press" => assert_eq!(
                (after.server.model_id.as_deref(), after.server.state),
                (Some("fixture-other"), "ready")
            ),
            "new press" => assert_eq!(
                (
                    after.server.state,
                    after.server.provider.as_ref().map(Provider::identity)
                ),
                ("ready", Some(provider.identity())),
                "the person's new press lost the port the ended server held"
            ),
            _ => {
                assert_eq!(after.server.state, "stopped", "{how}");
                assert!(
                    closes(&address(&provider)).await,
                    "{how}: the stopped server's port still takes connections"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_planned_under_an_older_generation_does_nothing() {
    let fixture = recovering();
    let (server, provider) = fixture.press().await;
    fixture.end(1, "code 7");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the person's press applied was not taken up: {waiting}"
    );
    // The person presses again while the restart waits: a new generation,
    // on the port the ended server held.
    let (again, again_provider) = fixture.press().await;
    assert_eq!(
        again_provider.identity(),
        provider.identity(),
        "the person's new press lost the port the ended server held"
    );
    fixture.clock.advance(SECOND);
    let after = fixture.manager.snapshot();
    assert_eq!(fixture.starts(), 2, "the older generation's restart ran");
    assert_eq!(
        (after.server.id.as_deref(), after.server.state),
        (Some(again.as_str()), "ready")
    );
    assert!(
        fixture.attempts().is_empty(),
        "the older generation's restart left a record"
    );
    // The new press's own end is taken up, under its own generation.
    fixture.end(2, "code 7");
    let waiting = fixture.ended(&again).await;
    assert_eq!(waiting["server"]["state"], "starting", "{waiting}");
    fixture.clock.advance(SECOND);
    fixture.back(&again).await;
    assert_eq!(fixture.starts(), 3);
    let (exits, attempts) = (fixture.exits(), fixture.attempts());
    assert!(
        exits[0]["generation"].as_u64().unwrap() < exits[1]["generation"].as_u64().unwrap(),
        "{exits:?}"
    );
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(attempts[0]["generation"], exits[1]["generation"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_port_another_process_holds_is_refused_and_its_listener_never_taken_over() {
    let fixture = recovering();
    let (server, provider) = fixture.press().await;
    let settings = fs::read(&fixture.settings).unwrap();
    let address = address(&provider);
    fixture.end(1, "code 7");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the person's press applied was not taken up: {waiting}"
    );
    // The manager loses its hold on the port while the restart waits, and
    // another process takes it.
    test_support::lose_waiting_port(&fixture.manager);
    let began = Instant::now();
    let holder = loop {
        match TcpListener::bind(address.as_str()) {
            Ok(holder) => break holder,
            Err(error) => {
                assert!(began.elapsed() < SETTLE, "the port was not let go: {error}");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    };
    holder.set_nonblocking(true).unwrap();
    fixture.clock.advance(SECOND);
    let refused = fixture.manager.snapshot();
    assert_eq!(fixture.starts(), 1, "a runtime started without its port");
    assert_eq!(refused.server.state, "failed");
    assert!(
        refused.server.provider.is_none() && refused.applied_model_id.is_none(),
        "the holder's port was taken as the server's"
    );
    assert!(
        matches!(holder.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "the manager reached the port's holder"
    );
    let probe = TcpStream::connect(address.as_str()).expect("the holder no longer listens");
    holder.set_nonblocking(false).unwrap();
    holder.accept().expect("the holder no longer accepts");
    drop(probe);
    assert_eq!(
        fs::read(&fixture.settings).unwrap(),
        settings,
        "the refused restart wrote the settings"
    );
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(attempts[0]["cause"], "runtime_exited");
    assert_eq!(attempts[0]["result"], "server_stopped");
    fixture.clock.advance(HOUR);
    assert_eq!(fixture.starts(), 1, "the refused restart was tried again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deadline_stop_like_the_one_at_1935_comes_back_once_and_a_second_stays_stopped_with_its_reason()
 {
    let fixture = recovering();
    test_support::serve_limits(
        &fixture.manager,
        Limits {
            request: STALL,
            ..Limits::DEFAULT
        },
    );
    let (server, provider) = fixture.press().await;
    // A seat's request the runtime never answers, as at 19:35: the request
    // deadline stops the runtime.
    assert_eq!(
        fixture.stall(&provider).await,
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
    let waiting = fixture.ended(&server).await;
    let exit = &waiting["server"]["exit"];
    assert_eq!(
        (&exit["cause"], &exit["task"]),
        (&json!("runtime_timeout"), &json!("stall-seat")),
        "{waiting}"
    );
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the deadline stop left the server stopped until the person presses: {waiting}"
    );
    fixture.clock.advance(SECOND);
    let back = fixture.back(&server).await;
    assert_eq!(
        back.server.provider.as_ref().map(Provider::identity),
        Some(provider.identity())
    );
    let second = back.server.id.unwrap();
    // The same stall under the same press: the guard has had its one restart.
    assert_eq!(
        fixture.stall(&provider).await,
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
    let stopped = fixture.ended(&second).await;
    assert_eq!(
        stopped["server"]["state"], "failed",
        "a second deadline stop under one press was planned again: {stopped}"
    );
    assert_eq!(stopped["server"]["exit"]["cause"], "runtime_timeout");
    assert_eq!(stopped["server"]["exit"]["task"], "stall-seat");
    fixture.clock.advance(HOUR);
    assert_eq!(
        fixture.starts(),
        2,
        "a second deadline stop was started again"
    );
    assert_eq!(fixture.manager.snapshot().server.state, "failed");
    assert_eq!(fixture.recoveries(), [json!("restart"), json!("guard")]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_memory_guards_stop_comes_back_once_per_press_and_never_in_a_loop() {
    let fixture = recovering();
    let (server, _) = fixture.press().await;
    fixture.end(1, "code 75");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the memory guard's stop was not taken up once: {waiting}"
    );
    assert_eq!(waiting["server"]["exit"]["cause"], "runtime_memory_limit");
    fixture.clock.advance(SECOND);
    let second = fixture.back(&server).await.server.id.unwrap();
    // An hour on, the guard stops it again under the same press: a guard's
    // second stop is never restarted, however far apart.
    fixture.clock.advance(HOUR);
    fixture.end(2, "code 75");
    let stopped = fixture.ended(&second).await;
    assert_eq!(
        stopped["server"]["state"], "failed",
        "a second guard stop under one press was planned again: {stopped}"
    );
    assert_eq!(stopped["server"]["exit"]["cause"], "runtime_memory_limit");
    fixture.clock.advance(HOUR);
    assert_eq!(fixture.starts(), 2, "a second guard stop was started again");
    assert_eq!(fixture.recoveries(), [json!("restart"), json!("guard")]);
    // The person's next press starts a new budget.
    let (pressed, _) = fixture.press().await;
    fixture.end(3, "code 75");
    let waiting = fixture.ended(&pressed).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the person's new press did not start a new budget: {waiting}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ended_port_answers_server_restarting_with_retry_after_until_the_attempt_and_while_the_model_loads()
 {
    let fixture = recovering();
    let (server, provider) = fixture.press().await;
    fixture.end(1, "code 7");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the person's press applied was not taken up: {waiting}"
    );
    let restarting = json!("server_restarting");
    assert_eq!(
        fixture.refusal(&provider).await,
        (Some(1), restarting.clone())
    );
    fixture.clock.advance(SECOND);
    let second = fixture.back(&server).await.server.id.unwrap();
    fixture.end(2, "code 7");
    fixture.ended(&second).await;
    // Whole seconds until the attempt, by the manager's clock.
    assert_eq!(fixture.clock.asked(), [SECOND, Duration::from_secs(5)]);
    assert_eq!(
        fixture.refusal(&provider).await,
        (Some(5), restarting.clone())
    );
    fixture.clock.advance(Duration::from_millis(2500));
    assert_eq!(
        fixture.refusal(&provider).await,
        (Some(3), restarting.clone())
    );
    // Once the attempt runs, a short wait while the restarted model loads.
    fixture.tell("hold", 3, "");
    fixture.clock.advance(Duration::from_millis(2500));
    assert_eq!(fixture.refusal(&provider).await, (Some(2), restarting));
    fixture.tell("go", 3, "");
    fixture.back(&second).await;
    // The same client's next request reaches the restarted server, not the
    // ended one it last spoke to.
    let answered = fixture
        .client
        .post(provider.request_url())
        .json(&question(&provider, "fixture"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        answered.status(),
        reqwest::StatusCode::OK,
        "the client's next request stayed on the ended server"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_restart_caps_hold_inside_their_window_and_end_with_the_press() {
    let fixture = recovering();
    let (mut server, _) = fixture.press().await;
    for start in 1..=3 {
        fixture.end(start, "code 7");
        let waiting = fixture.ended(&server).await;
        assert_eq!(
            waiting["server"]["state"], "starting",
            "end {start} under the person's press was not taken up: {waiting}"
        );
        fixture.clock.advance(fixture.clock.asked()[start - 1]);
        server = fixture.back(&server).await.server.id.unwrap();
    }
    assert_eq!(
        fixture.clock.asked(),
        [SECOND, Duration::from_secs(5), Duration::from_secs(25)],
        "the waits before the three restarts"
    );
    fixture.end(4, "code 7");
    let stopped = fixture.ended(&server).await;
    assert_eq!(
        stopped["server"]["state"], "failed",
        "a fourth end inside the window was planned again: {stopped}"
    );
    fixture.clock.advance(HOUR);
    assert_eq!(
        fixture.starts(),
        4,
        "a fourth end inside the window was started again"
    );
    assert_eq!(
        fixture.recoveries(),
        [
            json!("restart"),
            json!("restart"),
            json!("restart"),
            json!("limit")
        ]
    );
    // The limit ended the press, and its count with it: a new press waits
    // the shortest wait again.
    let (pressed, _) = fixture.press().await;
    fixture.end(5, "code 7");
    let waiting = fixture.ended(&pressed).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the person's new press did not start a new count: {waiting}"
    );
    assert_eq!(fixture.clock.asked().last(), Some(&SECOND));
    fixture.clock.advance(SECOND);
    let again = fixture.back(&pressed).await.server.id.unwrap();
    // A restart older than the window no longer counts.
    fixture.clock.advance(Duration::from_secs(10 * 60));
    fixture.end(6, "code 7");
    fixture.ended(&again).await;
    assert_eq!(
        fixture.clock.asked().last(),
        Some(&SECOND),
        "a restart older than ten minutes still counted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_selection_changed_elsewhere_ends_the_restarts() {
    let fixture = recovering();
    let (server, provider) = fixture.press().await;
    let elsewhere = Provider {
        base_url: "http://127.0.0.1:9".to_owned(),
        ..provider.clone()
    };
    let point_elsewhere = || {
        fs::write(
            &fixture.settings,
            serde_json::to_vec(&json!({"smart":{"judgmentProvider":elsewhere}})).unwrap(),
        )
        .unwrap();
    };
    fixture.end(1, "code 7");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the person's press applied was not taken up: {waiting}"
    );
    fixture.clock.advance(SECOND);
    let second = fixture.back(&server).await.server.id.unwrap();
    // The settings now point elsewhere: the next end stays an end.
    point_elsewhere();
    fixture.end(2, "code 7");
    let stopped = fixture.ended(&second).await;
    assert_eq!(
        stopped["server"]["state"], "failed",
        "a server the settings no longer select was planned again: {stopped}"
    );
    fixture.clock.advance(HOUR);
    assert_eq!(
        fixture.starts(),
        2,
        "a server the settings no longer select started again"
    );
    assert_eq!(fixture.recoveries(), [json!("restart"), Value::Null]);
    // A restart already waiting stands down when the settings move meanwhile.
    let (pressed, _) = fixture.press().await;
    fixture.end(3, "code 7");
    let waiting = fixture.ended(&pressed).await;
    assert_eq!(waiting["server"]["state"], "starting", "{waiting}");
    point_elsewhere();
    fixture.clock.advance(SECOND);
    let refused = fixture.manager.snapshot();
    assert_eq!(
        (fixture.starts(), refused.server.state),
        (3, "failed"),
        "a restart ran for settings that moved while it waited"
    );
    assert_eq!(
        fixture
            .attempts()
            .last()
            .map(|attempt| attempt["result"].clone()),
        Some(json!("settings_changed"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_that_comes_due_while_another_operation_runs_stands_down() {
    let fixture = recovering();
    let (server, _) = fixture.press().await;
    fixture.end(1, "code 7");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the person's press applied was not taken up: {waiting}"
    );
    test_support::begin_registration_blocker(&fixture.manager);
    fixture.clock.advance(SECOND);
    let stood_down = fixture.manager.snapshot();
    assert_eq!(
        (fixture.starts(), stood_down.server.state),
        (1, "failed"),
        "a restart ran beside another operation"
    );
    assert_eq!(
        stood_down.server.exit.as_ref().map(|exit| exit.cause),
        Some("runtime_exited"),
        "the stopped server lost its reason"
    );
    assert_eq!(
        fixture
            .attempts()
            .last()
            .map(|attempt| attempt["result"].clone()),
        Some(json!("operation_busy"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_switch_moves_the_press_to_its_model_and_the_server_it_retired_never_comes_back() {
    let fixture = recovering();
    let (server, provider) = fixture.press().await;
    let target = fixture.models[1].clone();
    let instance = fixture.manager.snapshot().instance_id;
    let settings = fixture.settings.clone();
    fixture
        .manager
        .switch_model(
            &target.id,
            &target.revision,
            &server,
            &provider.identity(),
            &instance,
            move |provider, _, binding| {
                select(&settings, provider, binding);
                Ok(())
            },
        )
        .unwrap();
    let switched = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(
        switched.applied_model_id,
        Some(target.id.clone()),
        "{:?}",
        switched
            .operation
            .as_ref()
            .and_then(|operation| operation.error)
    );
    let switched_server = switched.server.id.unwrap();
    fixture.clock.advance(HOUR);
    assert_eq!(
        fixture.starts(),
        2,
        "the server the switch retired came back"
    );
    fixture.end(2, "code 7");
    let waiting = fixture.ended(&switched_server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the switch applied was not taken up: {waiting}"
    );
    assert_eq!(waiting["server"]["modelId"], target.id.as_str());
    fixture.clock.advance(SECOND);
    let back = fixture.back(&switched_server).await;
    assert_eq!(
        (back.server.model_id, back.applied_model_id),
        (Some(target.id.clone()), Some(target.id))
    );
    assert_eq!(fixture.starts(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_that_does_not_come_up_is_not_tried_again_and_lets_its_port_go() {
    let fixture = recovering();
    let (server, provider) = fixture.press().await;
    fixture.tell("fail", 2, "");
    fixture.end(1, "code 7");
    let waiting = fixture.ended(&server).await;
    assert_eq!(
        waiting["server"]["state"], "starting",
        "the end of the server the person's press applied was not taken up: {waiting}"
    );
    fixture.clock.advance(SECOND);
    let stopped = crate::tests::settled_snapshot(&fixture.manager).await;
    assert_eq!(fixture.starts(), 2);
    assert_eq!(stopped.server.state, "failed", "{:?}", stopped.server.error);
    assert_eq!(
        stopped.server.exit.as_ref().map(|exit| exit.cause),
        Some("runtime_exited"),
        "the stopped server lost the reason it ended"
    );
    let attempts = fixture.attempts();
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_ne!(attempts[0]["result"], "ready");
    assert_eq!(
        attempts[0]["result"],
        json!(stopped.server.error),
        "the record and the card disagree on how the restart went"
    );
    assert!(
        closes(&address(&provider)).await,
        "the port of a restart that did not come up still takes connections"
    );
    fixture.clock.advance(HOUR);
    assert_eq!(
        fixture.starts(),
        2,
        "a restart that did not come up was tried again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_that_repeats_the_request_it_was_answering_leaves_that_text_out_of_all_it_keeps()
{
    let fixture = recovering();
    let started = fixture.start(&fixture.model()).await;
    let server = started.server.id.unwrap();
    let provider = started.server.provider.unwrap();
    // Built here, so no private value stands in this file for the gate to find.
    let key = format!("{}={}", "api_key", "Kp9".repeat(8));
    let token = format!("Authorization: {} {}", "Bearer", "tq8Vw".repeat(6));
    let home = format!("/{}/{}/models/fixture", "Users", "zqowner");
    let sentence = "Only my sister knows the harbor plan for Friday";
    fixture.tell("leak", 1, &format!("{key}\n{token}\nloading {home}\n"));
    let response = fixture
        .client
        .post(provider.request_url())
        .json(&question(&provider, sentence))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let ended = fixture.ended(&server).await;
    let said = tail(&ended["server"]["exit"]).join("\n");
    assert!(
        !said.contains(sentence),
        "the text of the request the runtime was answering reached the kept tail: {said}"
    );
    let kept = fs::read_to_string(fixture.data.join("managed-judgment/server-exits.jsonl"))
        .unwrap_or_default();
    let told = serde_json::to_string(&*fixture.told.lock().unwrap()).unwrap();
    for raw in [sentence, "Kp9Kp9", "tq8Vwtq8Vw", "zqowner"] {
        assert!(!said.contains(raw), "{raw} reached the kept tail: {said}");
        assert!(!kept.contains(raw), "{raw} reached server-exits.jsonl");
        assert!(
            !told.contains(raw),
            "{raw} reached the snapshot the card reads"
        );
    }
    assert!(
        !said.contains("could not judge"),
        "a line repeating the request stayed in part: {said}"
    );
    let ends = fixture.kept_end(&server).await;
    assert_eq!(
        reaching(&ends, &["Kp9Kp9", "tq8Vwtq8Vw"]),
        Vec::<&str>::new(),
        "the key or the token written while the runtime answered was kept"
    );
    assert_eq!(
        reaching(&ends, &["zqowner"]),
        Vec::<&str>::new(),
        "the home path written while the runtime answered was kept"
    );
}

/// What a person asks first: a sentence no mask finds.
const ASKED_FIRST: &str = "Only my sister knows the harbor plan for Friday";

/// What they ask next, which shares no 12 characters in a row with
/// `ASKED_FIRST`, so nothing that matches the request in flight finds the
/// first.
const ASKED_NEXT: &str = "Rain gauges overflowed near the quarry wall";

/// Starts the answering runtime, which writes `say` to stderr as it starts
/// and each request's state as `echo` says, has it answer `ASKED_FIRST`, and
/// lets it die while it handles `ASKED_NEXT`, repeating it.
async fn first_then_second(say: Option<&str>, echo: &str) -> (Recovering, String) {
    let next: Vec<char> = ASKED_NEXT.chars().collect();
    assert!(
        next.windows(12)
            .all(|run| !ASKED_FIRST.contains(&run.iter().collect::<String>())),
        "the second request shares 12 characters in a row with the first"
    );
    let fixture = recovering();
    if let Some(say) = say {
        fixture.tell("say", 1, say);
    }
    fixture.tell("echo", 1, echo);
    let started = fixture.start(&fixture.model()).await;
    let server = started.server.id.unwrap();
    let provider = started.server.provider.unwrap();
    let answered = fixture
        .client
        .post(provider.request_url())
        .json(&question(&provider, ASKED_FIRST))
        .send()
        .await
        .unwrap();
    assert_eq!(answered.status(), reqwest::StatusCode::OK);
    fixture.tell("leak", 1, "");
    let died = fixture
        .client
        .post(provider.request_url())
        .json(&question(&provider, ASKED_NEXT))
        .send()
        .await
        .unwrap();
    assert_eq!(
        died.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "the runtime did not end while it handled the second request"
    );
    (fixture, server)
}

/// The places in `kept` whose text holds any of `private`.
fn reaching(kept: &[(&'static str, Value, String)], private: &[&str]) -> Vec<&'static str> {
    kept.iter()
        .filter(|(_, _, text)| private.iter().any(|&word| text.contains(word)))
        .map(|(place, _, _)| *place)
        .collect()
}

/// Asserts that none of `private` reached any place the end is kept, and
/// that each place counts the stderr lines the server withheld.
#[track_caller]
fn withheld_everywhere(kept: &[(&'static str, Value, String)], private: &[&str]) {
    let exits: Value = kept
        .iter()
        .map(|(place, exit, _)| json!({"place": place, "exit": exit}))
        .collect();
    assert_eq!(
        reaching(kept, private),
        Vec::<&str>::new(),
        "an earlier request's text reached what is kept of the end: {exits}"
    );
    assert!(
        kept.iter().all(|(_, exit, _)| exit["stderrWithheld"]
            .as_u64()
            .is_some_and(|count| count > 0)),
        "a place does not count the stderr lines the server withheld: {exits}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_answered_while_stderr_repeats_it_leaves_none_of_it_kept_when_the_runtime_dies_on_the_next()
 {
    let (fixture, server) = first_then_second(None, "now").await;
    withheld_everywhere(&fixture.kept_end(&server).await, &[ASKED_FIRST]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_requests_stderr_that_drains_only_after_the_next_request_is_asked_is_not_kept_either() {
    // The runtime writes what it was first asked only once the second request
    // has reached it.
    let (fixture, server) = first_then_second(None, "late").await;
    withheld_everywhere(&fixture.kept_end(&server).await, &[ASKED_FIRST]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_line_still_open_when_the_runtime_is_first_asked_is_not_kept_in_part() {
    // The runtime begins a line as it starts and ends it with what it was
    // first asked.
    let (fixture, server) = first_then_second(Some("warming up for "), "now").await;
    withheld_everywhere(
        &fixture.kept_end(&server).await,
        &[ASKED_FIRST, "warming up for"],
    );
}

#[tokio::test]
async fn kept_end_waits_for_the_journal_row_after_the_exit_event() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    let fixture = recovering();
    let server = "delayed-exit";
    let exit = json!({"stderr": [], "stderrWithheld": 1});
    fixture.told.lock().unwrap().push(json!({
        "server": {"id": server, "exit": exit},
    }));
    let mut reading = Box::pin(fixture.kept_end(server));
    let first = {
        let mut context = Context::from_waker(Waker::noop());
        reading.as_mut().poll(&mut context)
    };
    let record = json!({"serverId": server, "stderr": [], "stderrWithheld": 1});
    let journal = fixture.data.join("managed-judgment/server-exits.jsonl");
    fs::create_dir_all(journal.parent().unwrap()).unwrap();
    fs::write(&journal, format!("{record}\n")).unwrap();
    let kept = match first {
        Poll::Ready(kept) => kept,
        Poll::Pending => reading.await,
    };
    assert_eq!(
        kept.iter()
            .find(|(place, _, _)| *place == "server-exits.jsonl")
            .unwrap()
            .1,
        record,
        "kept_end returned before the exit event's journal row was written"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runtime_that_ends_before_it_is_asked_anything_keeps_its_masked_tail_everywhere() {
    let fixture = recovering();
    // Built here, so no private value stands in this file for the gate to find.
    let key = format!("{}={}", "api_key", "Kp9".repeat(8));
    let token = format!("Authorization: {} {}", "Bearer", "tq8Vw".repeat(6));
    let home = format!("/{}/{}/models/fixture", "Users", "zqowner");
    fixture.tell(
        "say",
        1,
        &format!("{key}\n{token}\nloading {home}\nready to judge\n"),
    );
    let server = fixture.start(&fixture.model()).await.server.id.unwrap();
    fixture.end(1, "code 7");
    for (place, exit, text) in fixture.kept_end(&server).await {
        assert_eq!(exit["cause"], "runtime_exited", "{place}: {exit}");
        let said = tail(&exit).join("\n");
        assert!(
            said.ends_with("ready to judge"),
            "{place} lost the tail of a runtime never asked anything: {exit}"
        );
        assert!(
            said.matches("[redacted]").count() >= 2,
            "{place} keeps the key and the token unmasked or not at all: {said}"
        );
        assert!(
            said.contains(&format!("/{}/dev/models/fixture", "Users")),
            "{place} keeps the home path unmasked or not at all: {said}"
        );
        for raw in ["Kp9Kp9", "tq8Vwtq8Vw", "zqowner"] {
            assert!(!text.contains(raw), "{raw} reached {place}");
        }
        assert!(
            exit["stderrWithheld"]
                .as_u64()
                .is_none_or(|count| count == 0),
            "{place} counts lines withheld from a runtime never asked anything: {exit}"
        );
    }
}
