use super::*;
use std::future::Future;
use std::task::{Context, Poll, Waker};

const CLOSED_INPUT: &str = r#"
import os,sys,time
root=sys.argv[1]
os.close(0)
with open(os.path.join(root,'ready'),'w') as marker:
 marker.write('ready')
while True:
 time.sleep(1)
"#;

const UNUSED_INPUT: &str = r#"
import os,sys,time
root=sys.argv[1]
sys.stdin.readline()
with open(os.path.join(root,'received'),'w') as marker:
 marker.write('received')
while True:
 time.sleep(1)
"#;

async fn fixture(program: &str, root: &Path) -> (Running, HttpState) {
    let engine = Engine::from_command(
        Command::new("python3")
            .args(["-I", "-S", "-B", "-u", "-c", program])
            .arg(root),
    );
    let failed: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    let running = serve_limited(
        &crate::tests::fixture_model(),
        engine.clone(),
        failed.clone(),
        Limits {
            request: Duration::from_secs(5),
            queue_wait: Duration::from_secs(5),
            waiting: 1,
        },
    )
    .await
    .unwrap();
    let state = HttpState {
        engine,
        provider: running.provider.clone(),
        admission: running.admission.clone(),
        failed,
    };
    (running, state)
}

async fn until(what: &str, done: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(what);
}

#[tokio::test]
async fn an_io_failure_closes_admission_before_its_supervisor_is_polled_again() {
    let root = tempfile::tempdir().unwrap();
    let (_running, state) = fixture(CLOSED_INPUT, root.path()).await;
    until("the fake runtime closes its input", || {
        root.path().join("ready").exists()
    })
    .await;
    let ticket = state.admission.enter().unwrap();
    let turn = state.admission.turn.clone().acquire_owned().await.unwrap();
    let mut exchange = Box::pin(exchange_in_turn(
        state.clone(),
        turn,
        ticket,
        b"{}".to_vec(),
    ));
    {
        let _io = lock(&state.engine.io);
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            exchange.as_mut().poll(&mut context),
            Poll::Pending
        ));
    }
    until(
        "the blocking exchange either closes or releases its turn",
        || state.admission.turn.is_closed() || state.admission.turn.available_permits() > 0,
    )
    .await;
    assert!(
        state.admission.turn.is_closed(),
        "a queued request can acquire the failed runtime before its supervisor runs"
    );
    assert_eq!(exchange.await, Err(Error::RuntimeFailed));
    assert!(!state.engine.alive());
}

#[tokio::test]
async fn a_prepared_frame_at_the_pipe_limit_is_refused_without_stopping_the_runtime() {
    let root = tempfile::tempdir().unwrap();
    let (running, state) = fixture(UNUSED_INPUT, root.path()).await;
    let mut request = json!({
        "model": state.provider.model,
        "state": "",
        "questions": {"only": {"type": "noul", "instructions": "fixture"}},
    });
    let padding = MAX_FRAME - serde_json::to_vec(&request).unwrap().len();
    request["state"] = json!("x".repeat(padding));
    let body = serde_json::to_vec(&request).unwrap();
    assert_eq!(body.len(), MAX_FRAME);
    request["model"] = json!(state.provider.identity());
    let prepared = state
        .provider
        .prepare(serde_json::to_vec(&request).unwrap())
        .unwrap();
    assert_eq!(prepared.body.len(), MAX_FRAME);
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::HOST,
        state
            .provider
            .base_url
            .trim_start_matches("http://")
            .parse()
            .unwrap(),
    );
    let response = predict(State(state.clone()), headers, Bytes::from(body)).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(state.engine.alive());
    assert_eq!(running.admitted(), 0);
    assert!(!root.path().join("received").exists());
}
