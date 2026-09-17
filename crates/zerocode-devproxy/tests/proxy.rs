//! What the label proxy does with a real socket on both sides.
//!
//! The label host is never resolved: a test connects to the proxy's own
//! loopback port and carries the label in the `Host` header, which is exactly
//! how a browser reaches it in production too — `*.zerocode.localhost` is a
//! name the browser resolves to the loopback, not a name this process serves
//! through DNS.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use zerocode_core::localhost_label::{LabelInput, RouteSpec};
use zerocode_devproxy::{AllowedPorts, ProxyState, RouteError, TargetGate, serve};

/// A gate that says yes, for the tests that are about something else.
struct OpenGate;
impl TargetGate for OpenGate {
    fn allows(&self, _target: &Url) -> bool {
        true
    }
}

/// A gate that can be told to stop trusting a port mid-session.
struct MoodyGate {
    trusting: std::sync::atomic::AtomicBool,
}
impl TargetGate for MoodyGate {
    fn allows(&self, _target: &Url) -> bool {
        self.trusting.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// An upstream that records the exact request head it was handed and answers
/// with a canned response.
struct Upstream {
    port: u16,
    seen: Arc<tokio::sync::Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn capturing_upstream(answer: &'static str) -> Upstream {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                continue;
            };
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                recorded
                    .lock()
                    .await
                    .push(String::from_utf8_lossy(&buffer[..read]).to_string());
                let _ = stream.write_all(answer.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });
    Upstream { port, seen, task }
}

/// A port nobody is listening on: bound, read, dropped.
async fn dead_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    listener.local_addr().unwrap().port()
}

/// An upstream answer carrying one hop-by-hop field of its own, so a leak of
/// the dev server's connection policy into the browser's socket is visible.
const ONE_LINE: &str = "HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: keep-alive\r\n\
     keep-alive: timeout=99\r\nx-frame: deny\r\n\r\nhello";

async fn proxy(gate: Arc<dyn TargetGate>) -> (ProxyState, u16, tokio::task::JoinHandle<()>) {
    let state = ProxyState::new(gate);
    let (addr, task) = serve(state.clone(), 0).await.unwrap();
    assert!(
        addr.ip().is_loopback(),
        "the proxy must never leave this machine"
    );
    (state, addr.port(), task)
}

fn spec<'a>(owner: &'a str, target: &'a str) -> RouteSpec<'a> {
    RouteSpec {
        identity: LabelInput {
            project_name: "Snap Studio",
            worktree_name: "ui-auth",
            worktree_path: None,
            repo_id: None,
            worktree_id: Some("wt-a"),
        },
        owner,
        target_url: target,
    }
}

/// Speak plainly to the proxy: our own client, so what is asserted is the
/// server's behaviour rather than a library's.
async fn ask(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer)).await;
    String::from_utf8_lossy(&answer).to_string()
}

fn get(label: &str, proxy_port: u16, path: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {label}.zerocode.localhost:{proxy_port}\r\nconnection: close\r\n\r\n"
    )
}

/// The whole point, end to end: a labeled name reaches the dev server that
/// owns it, and the answer comes back untouched.
#[tokio::test(flavor = "multi_thread")]
async fn a_labeled_name_reaches_its_own_dev_server_and_the_answer_comes_back_whole() {
    let upstream = capturing_upstream(ONE_LINE).await;
    let (state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{}/", upstream.port);
    let route = state.register(&spec("/w/a", &target)).unwrap();
    assert_eq!(route.label, "ui-auth");
    assert_eq!(
        route.url,
        format!("http://ui-auth.zerocode.localhost:{port}/")
    );

    let answered = ask(port, &get("ui-auth", port, "/app?q=1")).await;
    assert!(answered.starts_with("HTTP/1.1 200 OK"), "{answered}");
    assert!(answered.contains("x-frame: deny"), "{answered}");
    assert!(answered.ends_with("hello"), "{answered}");

    let asked = upstream
        .seen
        .lock()
        .await
        .first()
        .cloned()
        .unwrap_or_default();
    assert!(asked.starts_with("GET /app?q=1 HTTP/1.1"), "{asked}");
    // The dev server is told the name it knows itself by, not the label.
    assert!(
        asked
            .to_lowercase()
            .contains(&format!("host: 127.0.0.1:{}", upstream.port)),
        "{asked}"
    );
    assert!(
        !asked.to_lowercase().contains("zerocode.localhost"),
        "{asked}"
    );
}

/// The connection's own fields stop at the hop, in both directions, and a
/// forwarding claim a tab typed never reaches the dev server.
#[tokio::test(flavor = "multi_thread")]
async fn the_hop_keeps_the_connections_own_headers_to_itself() {
    let upstream = capturing_upstream(ONE_LINE).await;
    let (state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{}/", upstream.port);
    state.register(&spec("/w/a", &target)).unwrap();

    let asked_with = format!(
        "GET / HTTP/1.1\r\nHost: ui-auth.zerocode.localhost:{port}\r\nconnection: close, x-secret\r\n\
         x-secret: do not forward\r\nx-forwarded-for: 10.0.0.9\r\nproxy-authorization: Basic hunter2\r\n\
         x-kept: ordinary\r\n\r\n"
    );
    let answered = ask(port, &asked_with).await;
    assert!(answered.starts_with("HTTP/1.1 200 OK"), "{answered}");
    // The dev server's own socket policy is about a connection the browser
    // cannot see, so it stops here.
    assert!(
        !answered.to_lowercase().contains("timeout=99"),
        "{answered}"
    );

    let asked = upstream
        .seen
        .lock()
        .await
        .first()
        .cloned()
        .unwrap_or_default();
    let lowered = asked.to_lowercase();
    for withheld in [
        "x-secret",
        "x-forwarded-for",
        "proxy-authorization",
        "connection:",
    ] {
        assert!(!lowered.contains(withheld), "{withheld} crossed: {asked}");
    }
    assert!(lowered.contains("x-kept: ordinary"), "{asked}");
}

/// A name this window never minted learns only that it is unknown.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_this_window_never_minted_is_a_404_that_reflects_nothing() {
    let (_state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let answered = ask(port, &get("stranger", port, "/")).await;
    assert!(answered.starts_with("HTTP/1.1 404"), "{answered}");
    assert!(!answered.contains("stranger"), "{answered}");

    // A host outside the label zone is not a label at all.
    let outside = format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nconnection: close\r\n\r\n");
    assert!(ask(port, &outside).await.starts_with("HTTP/1.1 404"));
}

/// A tunnel to anywhere is not what a label serves, and the refusal happens
/// before any route is looked up.
#[tokio::test(flavor = "multi_thread")]
async fn a_connect_request_is_refused_before_any_route_is_looked_up() {
    let upstream = capturing_upstream(ONE_LINE).await;
    let (state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{}/", upstream.port);
    state.register(&spec("/w/a", &target)).unwrap();

    let answered = ask(
        port,
        &format!("CONNECT evil.example:443 HTTP/1.1\r\nHost: ui-auth.zerocode.localhost:{port}\r\nconnection: close\r\n\r\n"),
    )
    .await;
    assert!(answered.starts_with("HTTP/1.1 405"), "{answered}");
    assert!(
        upstream.seen.lock().await.is_empty(),
        "upstream was reached"
    );
}

/// The authority of the hop belongs to the route: an absolute-form target
/// keeps its path and loses its host, and a walk upwards is refused outright.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_cannot_choose_its_own_upstream_or_walk_out_of_the_document_root() {
    let upstream = capturing_upstream(ONE_LINE).await;
    let (state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{}/", upstream.port);
    state.register(&spec("/w/a", &target)).unwrap();

    let absolute = format!(
        "GET http://evil.example/y?q=1 HTTP/1.1\r\nHost: ui-auth.zerocode.localhost:{port}\r\nconnection: close\r\n\r\n"
    );
    assert!(ask(port, &absolute).await.starts_with("HTTP/1.1 200"));
    let asked = upstream
        .seen
        .lock()
        .await
        .first()
        .cloned()
        .unwrap_or_default();
    assert!(asked.starts_with("GET /y?q=1 HTTP/1.1"), "{asked}");
    assert!(!asked.contains("evil.example"), "{asked}");

    let walked = ask(port, &get("ui-auth", port, "/%2e%2e/secret")).await;
    assert!(walked.starts_with("HTTP/1.1 400"), "{walked}");
    assert_eq!(
        upstream.seen.lock().await.len(),
        1,
        "the walk reached upstream"
    );
}

/// Two `Host` headers are two different requests; the hop refuses to pick.
#[tokio::test(flavor = "multi_thread")]
async fn two_host_headers_are_refused_rather_than_resolved() {
    let (_state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let asked = format!(
        "GET / HTTP/1.1\r\nHost: ui-auth.zerocode.localhost:{port}\r\nHost: other.zerocode.localhost:{port}\r\nconnection: close\r\n\r\n"
    );
    let answered = ask(port, &asked).await;
    assert!(answered.starts_with("HTTP/1.1 400"), "{answered}");
}

/// A gate that stops trusting a port stops the proxying, with no byte
/// forwarded — the original asks only once, at registration.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_the_gate_stops_trusting_is_answered_without_forwarding_a_byte() {
    let upstream = capturing_upstream(ONE_LINE).await;
    let gate = Arc::new(MoodyGate {
        trusting: std::sync::atomic::AtomicBool::new(true),
    });
    let (state, port, _task) = proxy(gate.clone()).await;
    let target = format!("http://127.0.0.1:{}/", upstream.port);
    state.register(&spec("/w/a", &target)).unwrap();
    assert!(
        ask(port, &get("ui-auth", port, "/"))
            .await
            .starts_with("HTTP/1.1 200")
    );

    gate.trusting
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let answered = ask(port, &get("ui-auth", port, "/")).await;
    assert!(answered.starts_with("HTTP/1.1 502"), "{answered}");
    assert_eq!(
        upstream.seen.lock().await.len(),
        1,
        "a refused request was forwarded"
    );
}

/// A port the gate does not know never takes a label — the check the original
/// exempts every loopback address from.
#[tokio::test(flavor = "multi_thread")]
async fn a_loopback_target_the_gate_does_not_know_is_refused_at_registration() {
    let (state, _port, _task) = proxy(Arc::new(AllowedPorts::default())).await;
    let refused = state.register(&spec("/w/a", "http://127.0.0.1:2375/"));
    assert!(
        matches!(refused, Err(RouteError::TargetRefused)),
        "{refused:?}"
    );

    // And the same gate, told about the port, admits it.
    let gate = AllowedPorts::new([("127.0.0.1".to_string(), 2375)]);
    let state = ProxyState::new(Arc::new(gate));
    let (addr, task) = serve(state.clone(), 0).await.unwrap();
    let _ = addr;
    assert!(
        state
            .register(&spec("/w/a", "http://127.0.0.1:2375/"))
            .is_ok()
    );
    task.abort();
}

/// A dev server that is not there is a plain sentence, not a stack trace, and
/// it names neither the label nor the operating system's word for the failure.
#[tokio::test(flavor = "multi_thread")]
async fn an_upstream_that_is_not_there_is_a_plain_text_502_that_names_nothing() {
    let (state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{}/", dead_port().await);
    state.register(&spec("/w/a", &target)).unwrap();
    let answered = ask(port, &get("ui-auth", port, "/")).await;
    assert!(answered.starts_with("HTTP/1.1 502"), "{answered}");
    assert!(answered.contains("text/plain; charset=utf-8"), "{answered}");
    assert!(!answered.contains("ui-auth"), "{answered}");
    assert!(
        !answered.to_lowercase().contains("connection refused"),
        "{answered}"
    );
}

/// A retired workspace's name stops answering, and is never handed to
/// somebody else while this process lives.
#[tokio::test(flavor = "multi_thread")]
async fn a_retired_label_stops_answering_and_is_never_handed_to_a_new_owner() {
    let upstream = capturing_upstream(ONE_LINE).await;
    let (state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{}/", upstream.port);
    state.register(&spec("/w/a", &target)).unwrap();
    assert!(
        ask(port, &get("ui-auth", port, "/"))
            .await
            .starts_with("HTTP/1.1 200")
    );

    assert_eq!(state.retire_owner("/w/a"), 1);
    assert!(
        ask(port, &get("ui-auth", port, "/"))
            .await
            .starts_with("HTTP/1.1 404")
    );

    // A different workspace with the same name gets a different label.
    let mut newcomer = spec("/w/b", &target);
    newcomer.identity.worktree_id = Some("wt-b");
    let taken = state.register(&newcomer).unwrap();
    assert_ne!(taken.label, "ui-auth", "a retired name was re-minted");
}

/// The dev server agrees to the upgrade, and only then are the two sockets
/// joined — the original bridges whatever answers.
#[tokio::test(flavor = "multi_thread")]
async fn a_websocket_upgrade_is_bridged_only_after_the_dev_server_agrees() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let upstream_port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        let asked = String::from_utf8_lossy(&buffer[..read]).to_lowercase();
        assert!(asked.contains("upgrade: websocket"), "{asked}");
        assert!(asked.contains("sec-websocket-key:"), "{asked}");
        let _ = stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
                  sec-websocket-accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n",
            )
            .await;
        // Echo one frame's worth of bytes so the bridge can be observed.
        let mut relayed = vec![0u8; 16];
        let read = stream.read(&mut relayed).await.unwrap_or(0);
        let _ = stream.write_all(&relayed[..read]).await;
    });

    let (state, port, _proxy_task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{upstream_port}/");
    state.register(&spec("/w/a", &target)).unwrap();

    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client
        .write_all(
            format!(
                "GET /socket HTTP/1.1\r\nHost: ui-auth.zerocode.localhost:{port}\r\n\
                 upgrade: websocket\r\nconnection: Upgrade\r\n\
                 sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut head = vec![0u8; 256];
    let read = client.read(&mut head).await.unwrap();
    let answered = String::from_utf8_lossy(&head[..read]).to_string();
    assert!(answered.starts_with("HTTP/1.1 101"), "{answered}");
    assert!(
        answered
            .to_lowercase()
            .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "{answered}"
    );

    client.write_all(b"ping").await.unwrap();
    let mut echoed = vec![0u8; 4];
    let read = tokio::time::timeout(Duration::from_secs(5), client.read(&mut echoed))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&echoed[..read], b"ping");
    task.abort();
}

/// An upstream that answers something other than 101 is relayed as an
/// ordinary response, and no byte relay begins.
#[tokio::test(flavor = "multi_thread")]
async fn an_upstream_that_answers_200_to_an_upgrade_is_never_bridged() {
    let upstream =
        capturing_upstream("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nno")
            .await;
    let (state, port, _task) = proxy(Arc::new(OpenGate)).await;
    let target = format!("http://127.0.0.1:{}/", upstream.port);
    state.register(&spec("/w/a", &target)).unwrap();

    let answered = ask(
        port,
        &format!(
            "GET /socket HTTP/1.1\r\nHost: ui-auth.zerocode.localhost:{port}\r\n\
             upgrade: websocket\r\nconnection: Upgrade, close\r\n\
             sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n"
        ),
    )
    .await;
    assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
    assert!(answered.ends_with("no"), "{answered}");
}
