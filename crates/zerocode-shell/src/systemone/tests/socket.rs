//! Which connection a question rides (t-13199): the one the question before
//! it was answered on, and never one that went quiet under a question nobody
//! answered.
//!
//! The stage here keeps its connections open — the endpoint in `tests.rs`
//! closes each one after its answer, so nothing is ever reused on it — and
//! speaks HTTP/1.1 or HTTP/2, answering on some connections and holding the
//! rest silent: the connection stays up and nothing comes back on it, which
//! is what a server that has stopped answering looks like from the client. It
//! records which connection carried each request, counted from zero in the
//! order the connections were accepted.
//!
//! HTTP/2 is spoken with prior knowledge, since a loopback stage has no TLS
//! to negotiate it over. The pool keeps the connection the same way however
//! it was chosen: one shared connection per origin, handed to every request.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

/// How a stage speaks.
#[derive(Debug, Clone, Copy)]
enum Speaks {
    Http1,
    Http2,
}

/// The wall a question on a silent connection runs into: many loopback
/// answers long, and short enough to wait out.
const QUIET: Duration = Duration::from_millis(300);

/// The wall of a question the stage answers — far past any loopback answer.
const UNHURRIED: Duration = Duration::from_secs(5);

/// A loopback System One that keeps its connections open.
struct Stage {
    addr: SocketAddr,
    /// The connection each request arrived on, in the order they arrived.
    rode: Arc<Mutex<Vec<usize>>>,
    /// Connections accepted so far.
    opened: Arc<AtomicUsize>,
}

impl Stage {
    /// A stage speaking `speaks` that answers `status`, with an answer in the
    /// contract's shape, on every connection but those `silent` names by
    /// ordinal — which hear their requests and say nothing.
    fn open(speaks: Speaks, status: u16, silent: impl Fn(usize) -> bool + Send + 'static) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback seat");
        listener
            .set_nonblocking(true)
            .expect("a seat the stage's runtime can own");
        let addr = listener.local_addr().expect("its address");
        let rode = Arc::new(Mutex::new(Vec::new()));
        let opened = Arc::new(AtomicUsize::new(0));
        let (heard, counted) = (Arc::clone(&rode), Arc::clone(&opened));
        let answer = Bytes::from(
            json!({"model": ANSWERING_VERSION, "answers": {},
                   "usage": {"input_tokens": 1, "output_tokens": 0}})
            .to_string(),
        );
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the stage's runtime");
            runtime.block_on(async move {
                let listener = TcpListener::from_std(listener).expect("the loopback seat");
                while let Ok((stream, _)) = listener.accept().await {
                    let ordinal = counted.fetch_add(1, Ordering::SeqCst);
                    let spoken = (!silent(ordinal)).then(|| (status, answer.clone()));
                    let heard = Arc::clone(&heard);
                    match speaks {
                        Speaks::Http1 => tokio::spawn(http1(stream, ordinal, spoken, heard)),
                        Speaks::Http2 => tokio::spawn(http2(stream, ordinal, spoken, heard)),
                    };
                }
            });
        });
        Self { addr, rode, opened }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The connection each request so far arrived on.
    fn rode(&self) -> Vec<usize> {
        self.rode
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn opened(&self) -> usize {
        self.opened.load(Ordering::SeqCst)
    }
}

/// One HTTP/1.1 connection: each whole request recorded, then answered with
/// the connection kept open for the next — or, on a silent connection, heard
/// and left without a word until the client goes.
async fn http1(
    mut stream: TcpStream,
    ordinal: usize,
    spoken: Option<(u16, Bytes)>,
    heard: Arc<Mutex<Vec<usize>>>,
) {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        while let Some(end) = request_end(&raw) {
            raw.drain(..end);
            heard
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(ordinal);
            let Some((status, answer)) = &spoken else {
                continue;
            };
            let head = format!(
                "HTTP/1.1 {status} Scripted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                answer.len()
            );
            if stream.write_all(head.as_bytes()).await.is_err()
                || stream.write_all(answer).await.is_err()
            {
                return;
            }
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
        }
    }
}

/// One HTTP/2 connection: each stream recorded and answered on a task of its
/// own, the connection kept for the next — or, on a silent connection, each
/// stream held without a word until the client resets it.
async fn http2(
    stream: TcpStream,
    ordinal: usize,
    spoken: Option<(u16, Bytes)>,
    heard: Arc<Mutex<Vec<usize>>>,
) {
    let Ok(mut connection) = h2::server::handshake(stream).await else {
        return;
    };
    while let Some(Ok((request, respond))) = connection.accept().await {
        heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ordinal);
        tokio::spawn(answer_stream(request, respond, spoken.clone()));
    }
}

async fn answer_stream(
    request: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
    spoken: Option<(u16, Bytes)>,
) {
    // The body is read whole and its window given back, so a connection that
    // carries a hundred questions never stalls on flow control.
    let mut body = request.into_body();
    while let Some(Ok(chunk)) = body.data().await {
        let _ = body.flow_control().release_capacity(chunk.len());
    }
    let Some((status, answer)) = spoken else {
        let _ = std::future::poll_fn(|context| respond.poll_reset(context)).await;
        return;
    };
    let Ok(head) = http::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(())
    else {
        return;
    };
    if let Ok(mut sending) = respond.send_response(head, false) {
        let _ = sending.send_data(answer, true);
    }
}

/// A socket of the test's own, building its clients the way the window's
/// does — the same builder, told to speak HTTP/2 when the stage does.
fn socket_for(speaks: Speaks) -> &'static Socket<reqwest::Client> {
    fn speaking_http2() -> Option<reqwest::Client> {
        builder().http2_prior_knowledge().build().ok()
    }
    let build: fn() -> Option<reqwest::Client> = match speaks {
        Speaks::Http1 => built,
        Speaks::Http2 => speaking_http2,
    };
    Box::leak(Box::new(Socket::new(build)))
}

/// One question down `wire`, bounded by `wall`: the door's bytes for a key
/// check — no one's words — posted the way every seat's question is.
fn ask(wire: &Wire, wall: Duration) -> Result<String, String> {
    let settings = JevSettings::from_root(&Value::Null);
    let cleared = door::may_check_key(
        &door::Asking {
            key: true,
            settings: &settings,
            workspace: None,
            sent_today: 0,
        },
        &json!({}),
    )
    .expect("a key check carries no workspace words");
    tauri::async_runtime::block_on(wire.ask_once("test-key", cleared, Instant::now() + wall))
}

/// Before t-13199 a question that ran out of time left its client standing,
/// and over HTTP/2 the next question rode the very connection that had gone
/// quiet — in R11 q8 one run heard 0 of 58 while new connections to the same
/// service answered two or three times in five. The question after an
/// unanswered one opens a connection of its own, and is answered there.
fn the_question_after_an_unanswered_one_opens_a_connection_of_its_own(speaks: Speaks) {
    // The first connection hears and says nothing; every later one answers.
    let stage = Stage::open(speaks, 200, |ordinal| ordinal == 0);
    let wire = Wire::at(&stage.base(), "test-key", None).on(socket_for(speaks));
    assert_eq!(
        ask(&wire, QUIET),
        Err(TIMEOUT.to_string()),
        "the first connection says nothing"
    );
    let next = ask(&wire, UNHURRIED);
    assert_eq!(
        stage.rode(),
        [0, 1],
        "over {speaks:?} the question after an unanswered one rode the connection that had gone quiet"
    );
    assert!(
        next.is_ok(),
        "a connection of its own answered it: {next:?}"
    );
}

#[test]
fn over_http2_the_question_after_an_unanswered_one_opens_a_connection_of_its_own() {
    the_question_after_an_unanswered_one_opens_a_connection_of_its_own(Speaks::Http2);
}

/// HTTP/1.1 closes a connection whose request was dropped, so this held
/// before t-13199 as well — the fact the HTTP/2 case is measured against.
#[test]
fn over_http1_the_question_after_an_unanswered_one_opens_a_connection_of_its_own() {
    the_question_after_an_unanswered_one_opens_a_connection_of_its_own(Speaks::Http1);
}

/// A question the server answered leaves its connection for the next: two
/// answered questions ride one connection, the reuse the socket is kept for
/// (§2 of docs/design/jev-seats-accuracy-wave-20260921.md).
fn an_answered_question_leaves_its_connection_for_the_next(speaks: Speaks) {
    let stage = Stage::open(speaks, 200, |_| false);
    let wire = Wire::at(&stage.base(), "test-key", None).on(socket_for(speaks));
    for _ in 0..2 {
        let answer = ask(&wire, UNHURRIED);
        assert!(answer.is_ok(), "{answer:?}");
    }
    assert_eq!(
        stage.rode(),
        [0, 0],
        "over {speaks:?} an answered question's connection was not ridden again"
    );
}

#[test]
fn over_http2_an_answered_question_leaves_its_connection_for_the_next() {
    an_answered_question_leaves_its_connection_for_the_next(Speaks::Http2);
}

#[test]
fn over_http1_an_answered_question_leaves_its_connection_for_the_next() {
    an_answered_question_leaves_its_connection_for_the_next(Speaks::Http1);
}

/// A status is a response, and a response proves the socket carries: a
/// question refused with one leaves its connection for the next. Asked over
/// HTTP/2, where the pool keeps a connection whatever becomes of one stream,
/// so only the wire letting its client go could move the next question.
#[test]
fn a_question_refused_with_a_status_leaves_its_connection_for_the_next() {
    let stage = Stage::open(Speaks::Http2, 503, |_| false);
    let wire = Wire::at(&stage.base(), "test-key", None).on(socket_for(Speaks::Http2));
    for _ in 0..2 {
        assert_eq!(ask(&wire, UNHURRIED), Err(token_for(503)));
    }
    assert_eq!(
        stage.rode(),
        [0, 0],
        "a question refused with a status let its connection go"
    );
}

/// A client let go takes its pool with it, and the warm-up record of its
/// origin with the pool: the next door warms the origin again rather than
/// skip it for a socket that is gone. A late failure on a client already let
/// go forgets nothing — the record by then is the replacement's.
#[test]
fn a_client_let_go_lets_the_next_door_warm_its_origin_again() {
    // A documentation address, never dialled: only the record is read.
    let origin = "http://192.0.2.1:9";
    let socket = socket_for(Speaks::Http1);
    let lent = socket.lend().expect("a client");
    assert!(warm_due(origin), "the first door warms");
    assert!(!warm_due(origin), "the next skips: the socket is pooled");
    let_go(socket, &lent, origin);
    assert!(
        warm_due(origin),
        "after its client was let go, the next door skipped the origin"
    );
    let_go(socket, &lent, origin);
    assert!(
        !warm_due(origin),
        "a late failure on a client already let go forgot the replacement's warm-up"
    );
}

/// A fair coin for one connection of one trial, the same on every run: the
/// standard library's fixed-key hasher over the pair, its low bit.
fn coin(trial: u64, ordinal: usize) -> bool {
    let mut hasher = DefaultHasher::new();
    (trial, ordinal).hash(&mut hasher);
    hasher.finish() & 1 == 1
}

/// A span in whole microseconds, and in whole milliseconds — what a printed
/// measure keeps.
fn micros(span: Duration) -> u64 {
    u64::try_from(span.as_micros()).unwrap_or(u64::MAX)
}

fn millis(span: Duration) -> u64 {
    u64::try_from(span.as_millis()).unwrap_or(u64::MAX)
}

/// What the rule buys and what it costs, printed one line a measure: on
/// stages that hold half their connections silent, how many of a hundred
/// questions are answered; on a stage that answers every connection, each
/// question's latency and the connections opened; on a stage where every
/// connection is silent, how long a hundred questions take and how many
/// connections they open; and what building one client costs. Run on the
/// commit before the rule and on the rule's own (t-13199).
#[test]
#[ignore = "measurement, not a rule: run with --ignored --nocapture"]
fn measure_the_rule_on_stages_that_answer_all_half_and_none() {
    use zerocode_core::jev::summary::percentile;

    const QUESTIONS: usize = 100;
    const TRIALS: u64 = 10;
    const WALL: Duration = Duration::from_millis(150);
    let asked = |stage: &Stage, speaks: Speaks| {
        let wire = Wire::at(&stage.base(), "test-key", None).on(socket_for(speaks));
        let began = Instant::now();
        let mut spans = Vec::with_capacity(QUESTIONS);
        let mut answered = 0;
        for _ in 0..QUESTIONS {
            let one = Instant::now();
            if ask(&wire, WALL).is_ok() {
                answered += 1;
            }
            spans.push(micros(one.elapsed()));
        }
        spans.sort_unstable();
        (answered, spans, millis(began.elapsed()))
    };
    for speaks in [Speaks::Http1, Speaks::Http2] {
        let mut trials = Vec::new();
        for trial in 0..TRIALS {
            let stage = Stage::open(speaks, 200, move |ordinal| coin(trial, ordinal));
            let (answered, _, _) = asked(&stage, speaks);
            trials.push(json!({"trial": trial, "firstSilent": coin(trial, 0),
                               "answered": answered, "opened": stage.opened()}));
        }
        let total: u64 = trials
            .iter()
            .filter_map(|row| row["answered"].as_u64())
            .sum();
        println!(
            "jev_socket {}",
            json!({"speaks": format!("{speaks:?}"), "stage": "half_silent",
                   "questionsPerTrial": QUESTIONS, "wallMs": millis(WALL),
                   "answered": total, "of": QUESTIONS * trials.len(), "trials": trials})
        );

        let stage = Stage::open(speaks, 200, |_| false);
        let (answered, spans, _) = asked(&stage, speaks);
        println!(
            "jev_socket {}",
            json!({"speaks": format!("{speaks:?}"), "stage": "healthy",
                   "answered": answered, "of": QUESTIONS, "opened": stage.opened(),
                   "p50Us": percentile(&spans, 0.5), "p95Us": percentile(&spans, 0.95),
                   "maxUs": spans.last()})
        );

        let stage = Stage::open(speaks, 200, |_| true);
        let (answered, _, total_ms) = asked(&stage, speaks);
        println!(
            "jev_socket {}",
            json!({"speaks": format!("{speaks:?}"), "stage": "all_silent",
                   "wallMs": millis(WALL), "answered": answered, "of": QUESTIONS,
                   "totalMs": total_ms, "opened": stage.opened()})
        );
    }
    let mut builds: Vec<u64> = (0..200)
        .map(|_| {
            let began = Instant::now();
            let client = built();
            let spent = micros(began.elapsed());
            assert!(client.is_some(), "a client is built");
            spent
        })
        .collect();
    builds.sort_unstable();
    println!(
        "jev_socket {}",
        json!({"measure": "build_client", "n": builds.len(),
               "p50Us": percentile(&builds, 0.5), "p95Us": percentile(&builds, 0.95),
               "maxUs": builds.last()})
    );
}
