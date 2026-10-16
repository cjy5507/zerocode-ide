//! How the owned server admits requests to its runtime, against a fake
//! runtime the test releases one answer at a time: requests that arrive
//! together, a full queue, a caller that leaves while it waits, a wait that
//! runs out, a runtime that never answers, a truncated answer, a cut head and
//! shutdown.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::engine::{Engine, Limits, Running};

/// A runtime that writes down the questions of every request it reads, in
/// the order it reads them, and answers request `n` only once the file
/// `release-n` exists; `truncated` in that file makes it report a truncated
/// context, and `cut` a cut head for every question.
const GATED_RUNTIME: &str = r#"import json,os,sys,time
root=sys.argv[1]
for n,line in enumerate(sys.stdin):
 r=json.loads(line);names=sorted(r['questions'])
 with open(os.path.join(root,'arrived'),'a') as f: f.write(' '.join(names)+'\n')
 gate=os.path.join(root,'release-%d'%n)
 while not os.path.exists(gate): time.sleep(0.002)
 usage={'input_tokens':1,'output_tokens':0}
 mode=open(gate).read().strip()
 if mode=='truncated': usage['truncated']=True
 if mode=='cut': usage['truncated_questions']=names
 print(json.dumps({'model':r['model'],'answers':{k:{'type':'noul','noul':0.5} for k in names},'usage':usage}),flush=True)
"#;

/// A caller that waits as long as any test here runs.
const PATIENT: Duration = Duration::from_secs(20);

/// How long a step the server takes by itself may take on a slow machine.
const SETTLE: Duration = Duration::from_secs(5);

struct Gated {
    root: tempfile::TempDir,
    engine: Arc<Engine>,
    running: Running,
    failed: Arc<AtomicBool>,
    client: reqwest::Client,
    url: String,
    model: String,
}

async fn gated(limits: Limits) -> Gated {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::from_command(
        std::process::Command::new("python3")
            .args(["-I", "-u", "-c", GATED_RUNTIME])
            .arg(root.path()),
    );
    let failed = Arc::new(AtomicBool::new(false));
    let seen = failed.clone();
    let model = crate::tests::fixture_model();
    let running = crate::engine::serve_limited(
        &model,
        engine.clone(),
        Arc::new(move || seen.store(true, Ordering::Release)),
        limits,
    )
    .await
    .unwrap();
    let url = running.provider.request_url();
    Gated {
        root,
        engine,
        running,
        failed,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        url,
        model: model.id,
    }
}

impl Gated {
    fn ask(&self, names: &[&str], wait: Duration) -> reqwest::RequestBuilder {
        let questions: Map<String, Value> = names
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    json!({"type":"noul","instructions":"fixture"}),
                )
            })
            .collect();
        self.client
            .post(&self.url)
            .timeout(wait)
            .json(&json!({"model":self.model,"state":"fixture","questions":questions}))
    }

    fn arrived(&self) -> Vec<String> {
        arrived(self.root.path())
    }

    fn release(&self, request: usize, mode: &str) {
        let pending = self.root.path().join(format!("pending-{request}"));
        std::fs::write(&pending, mode).unwrap();
        std::fs::rename(pending, self.root.path().join(format!("release-{request}"))).unwrap();
    }
}

fn arrived(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("arrived"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < within, "{what} within {within:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The question names an answer carries, in order.
async fn answered(response: reqwest::Response) -> Vec<String> {
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    let mut names: Vec<String> = body["answers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// The word a 503 names.
async fn refused(response: reqwest::Response) -> String {
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json().await.unwrap();
    body["error"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_that_arrive_together_wait_their_turn_and_each_gets_its_own_answers() {
    let limits = Limits {
        request: PATIENT,
        queue_wait: PATIENT,
        waiting: 4,
    };
    let gated = gated(limits).await;
    let first = tokio::spawn(gated.ask(&["a0", "a1"], PATIENT).send());
    until("the first request reaches the runtime", SETTLE, || {
        gated.arrived().len() == 1
    })
    .await;
    let names = [["b0", "b1"], ["c0", "c1"], ["d0", "d1"], ["e0", "e1"]];
    let waiting: Vec<_> = names
        .iter()
        .map(|names| tokio::spawn(gated.ask(names, PATIENT).send()))
        .collect();
    until("four requests wait behind the first", SETTLE, || {
        gated.running.admitted() == 1 + limits.waiting
    })
    .await;
    // A full queue refuses at once and holds nothing more.
    for extra in 0..8 {
        let name = format!("x{extra}");
        let began = Instant::now();
        let response = gated.ask(&[name.as_str()], PATIENT).send().await.unwrap();
        assert_eq!(refused(response).await, "operation_busy");
        assert!(began.elapsed() < SETTLE);
        assert_eq!(gated.running.admitted(), 1 + limits.waiting);
    }
    assert_eq!(gated.arrived().len(), 1, "nothing ran beside the first");
    for request in 0..=limits.waiting {
        gated.release(request, "answer");
    }
    assert_eq!(answered(first.await.unwrap().unwrap()).await, ["a0", "a1"]);
    for (task, names) in waiting.into_iter().zip(names) {
        assert_eq!(answered(task.await.unwrap().unwrap()).await, names);
    }
    let arrived = gated.arrived();
    assert_eq!(arrived.len(), 1 + limits.waiting, "{arrived:?}");
    assert_eq!(arrived[0], "a0 a1");
    let mut behind = arrived[1..].to_vec();
    behind.sort();
    assert_eq!(behind, ["b0 b1", "c0 c1", "d0 d1", "e0 e1"]);
    until("the queue empties", SETTLE, || {
        gated.running.admitted() == 0
    })
    .await;
    assert!(gated.engine.alive());
    assert!(!gated.failed.load(Ordering::Acquire));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_request_whose_caller_left_never_reaches_the_runtime() {
    // The wait for a turn outlasts the test: only the caller leaving can
    // take the request out of the queue.
    let limits = Limits {
        request: PATIENT,
        queue_wait: PATIENT,
        waiting: 4,
    };
    let gated = gated(limits).await;
    let first = tokio::spawn(gated.ask(&["a"], PATIENT).send());
    until("the first request reaches the runtime", SETTLE, || {
        gated.arrived().len() == 1
    })
    .await;
    let left = gated.ask(&["b"], Duration::from_millis(50)).send().await;
    assert!(
        left.as_ref().is_err_and(reqwest::Error::is_timeout),
        "the caller gave up while it waited: {left:?}"
    );
    until("the abandoned request leaves the queue", SETTLE, || {
        gated.running.admitted() == 1
    })
    .await;
    gated.release(0, "answer");
    assert_eq!(answered(first.await.unwrap().unwrap()).await, ["a"]);
    let next = tokio::spawn(gated.ask(&["c"], PATIENT).send());
    until("the next request reaches the runtime", SETTLE, || {
        gated.arrived().len() == 2
    })
    .await;
    gated.release(1, "answer");
    assert_eq!(answered(next.await.unwrap().unwrap()).await, ["c"]);
    assert_eq!(gated.arrived(), ["a", "c"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_that_outwaits_the_queue_is_refused_and_the_runtime_keeps_running() {
    let limits = Limits {
        request: PATIENT,
        queue_wait: Duration::from_millis(200),
        waiting: 4,
    };
    let gated = gated(limits).await;
    let first = tokio::spawn(gated.ask(&["a"], PATIENT).send());
    until("the first request reaches the runtime", SETTLE, || {
        gated.arrived().len() == 1
    })
    .await;
    let began = Instant::now();
    let response = gated.ask(&["b"], PATIENT).send().await.unwrap();
    assert_eq!(refused(response).await, "operation_busy");
    assert!(
        began.elapsed() >= limits.queue_wait,
        "it waited its turn first: {:?}",
        began.elapsed()
    );
    assert!(
        gated.engine.alive(),
        "a wait that ran out is not a runtime failure"
    );
    assert!(!gated.failed.load(Ordering::Acquire));
    assert_eq!(gated.running.admitted(), 1);
    gated.release(0, "answer");
    assert_eq!(answered(first.await.unwrap().unwrap()).await, ["a"]);
    assert_eq!(gated.arrived(), ["a"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_runtime_deadline_holds_after_its_caller_leaves() {
    let limits = Limits {
        request: Duration::from_millis(300),
        queue_wait: PATIENT,
        waiting: 4,
    };
    let gated = gated(limits).await;
    let left = gated.ask(&["a"], Duration::from_millis(50)).send().await;
    assert!(
        left.as_ref().is_err_and(reqwest::Error::is_timeout),
        "the caller gave up first: {left:?}"
    );
    until(
        "the runtime that never answered is stopped",
        limits.request + SETTLE,
        || !gated.engine.alive(),
    )
    .await;
    until("the failure is reported", SETTLE, || {
        gated.failed.load(Ordering::Acquire)
    })
    .await;
    let response = gated.ask(&["b"], PATIENT).send().await.unwrap();
    assert_eq!(refused(response).await, "server_stopped");
    assert_eq!(gated.arrived(), ["a"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_refuses_waiting_requests_without_running_them() {
    let limits = Limits {
        request: PATIENT,
        queue_wait: PATIENT,
        waiting: 4,
    };
    let gated = gated(limits).await;
    let first = tokio::spawn(gated.ask(&["a"], PATIENT).send());
    until("the first request reaches the runtime", SETTLE, || {
        gated.arrived().len() == 1
    })
    .await;
    let queued = tokio::spawn(gated.ask(&["b"], PATIENT).send());
    until("one request waits behind the first", SETTLE, || {
        gated.running.admitted() == 2
    })
    .await;
    let Gated {
        root,
        engine,
        running,
        ..
    } = gated;
    running.retire().await;
    assert!(!engine.alive());
    // A connection shutdown closed is a refusal too; an answer never is.
    if let Ok(response) = tokio::time::timeout(SETTLE, queued).await.unwrap().unwrap() {
        assert_eq!(refused(response).await, "server_stopped");
    }
    if let Ok(response) = tokio::time::timeout(SETTLE, first).await.unwrap().unwrap() {
        assert_ne!(response.status(), reqwest::StatusCode::OK);
    }
    assert_eq!(arrived(root.path()), ["a"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_truncated_native_answer_names_its_reason_and_the_runtime_keeps_running() {
    let limits = Limits {
        request: PATIENT,
        queue_wait: PATIENT,
        waiting: 4,
    };
    let gated = gated(limits).await;
    let pending = tokio::spawn(gated.ask(&["a"], PATIENT).send());
    until("the request reaches the runtime", SETTLE, || {
        gated.arrived().len() == 1
    })
    .await;
    gated.release(0, "truncated");
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "native_protocol_failed");
    assert_eq!(body["refusal"], "provider_context_truncated");
    assert!(gated.engine.alive());
    assert!(!gated.failed.load(Ordering::Acquire));
    until("the queue empties", SETTLE, || {
        gated.running.admitted() == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_head_cut_native_answer_names_its_reason_and_the_runtime_keeps_running() {
    let limits = Limits {
        request: PATIENT,
        queue_wait: PATIENT,
        waiting: 4,
    };
    let gated = gated(limits).await;
    let pending = tokio::spawn(gated.ask(&["a", "b"], PATIENT).send());
    until("the request reaches the runtime", SETTLE, || {
        gated.arrived().len() == 1
    })
    .await;
    gated.release(0, "cut");
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "native_protocol_failed");
    assert_eq!(body["refusal"], "provider_context_truncated");
    assert!(gated.engine.alive());
    assert!(!gated.failed.load(Ordering::Acquire));
    until("the queue empties", SETTLE, || {
        gated.running.admitted() == 0
    })
    .await;
}
