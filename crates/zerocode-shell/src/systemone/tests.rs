//! The wire's honesty: one closed table of failure words, and one road to the
//! socket that carries only the door's bytes. The fake endpoint here is the
//! one every window test that crosses a real socket to System One stands up.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::*;

/// The version a fake endpoint's answers name as the one that answered: what
/// the real endpoint says for a request that asked the alias — a version, not
/// the alias (docs.typesafe.ai/api; every one of the 3,179 zo rows on this
/// machine that names a version names this one, 2026-09-23).
pub(crate) const ANSWERING_VERSION: &str = "jev-1.13.0";

/// A loopback System One that answers every request it is sent with one
/// status and one body, and remembers each request as it arrived.
pub(crate) struct Endpoint {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Endpoint {
    /// Answer `status` with `body`, after `hold_ms` milliseconds each time —
    /// one request at a time, which is what every seat that asks once needs.
    pub(crate) fn serving(status: &'static str, body: String, hold_ms: u64) -> Self {
        Self::listening(
            status,
            Arc::new(move |_: &str| body.clone()),
            hold_ms,
            false,
        )
    }

    /// The same endpoint answering every connection on a thread of its own,
    /// so requests that leave side by side are held side by side — what a
    /// test of a seat that asks in shards needs: on the serial endpoint two
    /// shards held 300 ms each would come back after 600 whether or not the
    /// seat asked them together.
    pub(crate) fn serving_each(status: &'static str, body: String, hold_ms: u64) -> Self {
        Self::listening(status, Arc::new(move |_: &str| body.clone()), hold_ms, true)
    }

    /// An endpoint whose body is chosen by the request it read — head and
    /// body as they arrived — each connection on a thread of its own: what a
    /// walk that asks two different questions of one endpoint needs.
    pub(crate) fn answering_each(
        status: &'static str,
        answer: impl Fn(&str) -> String + Send + Sync + 'static,
        hold_ms: u64,
    ) -> Self {
        Self::listening(status, Arc::new(answer), hold_ms, true)
    }

    fn listening(
        status: &'static str,
        body: Arc<dyn Fn(&str) -> String + Send + Sync>,
        hold_ms: u64,
        each: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback seat");
        let addr = listener.local_addr().expect("its address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&seen);
        thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else {
                    return;
                };
                let heard = Arc::clone(&heard);
                let body = Arc::clone(&body);
                let mut answer = move || {
                    let request = read_request(&mut socket);
                    let body = body(&request);
                    heard.lock().expect("the record").push(request);
                    if hold_ms > 0 {
                        thread::sleep(Duration::from_millis(hold_ms));
                    }
                    let answer = format!(
                        "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(answer.as_bytes());
                    let _ = socket.flush();
                };
                if each {
                    thread::spawn(answer);
                } else {
                    answer();
                }
            }
        });
        Self { addr, seen }
    }

    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Every request heard so far, head and body, in the order they came.
    pub(crate) fn asked(&self) -> Vec<String> {
        self.seen.lock().expect("the record").clone()
    }
}

/// One whole request off `socket`: the head, then as many body bytes as its
/// `Content-Length` names — a body larger than one read is still one request.
fn read_request(socket: &mut std::net::TcpStream) -> String {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let Ok(read) = socket.read(&mut chunk) else {
            break;
        };
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&raw);
        let Some(head_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let length = text[..head_end]
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if raw.len() >= head_end + 4 + length {
            break;
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

#[test]
fn a_status_is_refused_by_the_one_table_of_words() {
    assert_eq!(token_for(401), UNAUTHORIZED);
    assert_eq!(token_for(422), INVALID_REQUEST);
    assert_eq!(token_for(429), RATE_LIMITED);
    assert_eq!(token_for(529), OVERLOADED);
    // A status the contract does not name keeps its number in the row.
    assert_eq!(token_for(503), "http_503");
    assert_eq!(token_for(500), "http_500");
}

#[test]
fn an_expired_call_opens_no_socket() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", "{}".to_string(), 0);
    let wire = Wire::at(&endpoint.base(), "test-key", None);
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

    let answer = tauri::async_runtime::block_on(wire.ask_once("test-key", cleared, Instant::now()));
    assert_eq!(answer, Err(TIMEOUT.to_string()));
    assert!(
        endpoint.asked().is_empty(),
        "expiry cannot authorize a POST"
    );
}

/// The window's System One requests carry the door's bytes: the wire's only
/// POST sends a cleared body, and no other product file names the route.
#[test]
fn the_door_is_the_only_road_to_the_wire_in_the_window() {
    let wire = include_str!("../systemone.rs");
    // Up to the test module, not the first test attribute: `at` is test-only
    // and sits above the POST.
    let product = wire
        .split("#[cfg(test)]\npub(crate) mod tests")
        .next()
        .unwrap_or(wire);
    assert_eq!(product.matches(".post(").count(), 1);
    assert!(product.contains(".body(cleared.into_bytes())"));
    assert!(
        !product.contains(".json("),
        "a typed body would skip the door"
    );

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![src];
    let mut naming = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("a source folder").flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && std::fs::read_to_string(&path).is_ok_and(|text| text.contains("SYSTEMONE_PATH"))
            {
                naming.push(path);
            }
        }
    }
    let test_file = |path: &Path| path.file_name().is_some_and(|name| name == "tests.rs");
    assert!(
        naming
            .iter()
            .all(|path| path.ends_with("src/systemone.rs") || test_file(path)),
        "the System One route is named outside the window's wire: {naming:?}"
    );
}

/// What the wire hands back names the version that answered — the answer's
/// own `model`, not the alias the request asked for — and the one writer
/// every seat's row goes through puts it on the row under the key table's
/// spelling, beside the door's two counts (t-6187). An ask nothing answered
/// names no version, and its row carries no such key: a refusal at the door,
/// and a wall the answer never came back inside. The request itself names
/// the person's pin, whatever the body was built with.
#[test]
fn an_answer_names_its_version_on_the_row_and_an_unanswered_ask_names_none() {
    use zerocode_core::jev::STALL;
    use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
    use zerocode_core::jev::summary::MODEL;

    let home = tempfile::tempdir().expect("a zo home");
    let workspace = tempfile::tempdir().expect("a workspace");
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {"jevModel": "jev-1.13.0",
                         "jev": {"workspaces": [door::resolved_path(workspace.path())]}}})
        .to_string(),
    )
    .expect("settings");
    let answer = json!({"model": "jev-1.13.0", "answers": {},
                        "usage": {"input_tokens": 1, "output_tokens": 0}});
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer.to_string(), 0);
    let wire = Wire::at(&endpoint.base(), "test-key", Some(settings.clone()));
    let body = request_body(&json!({"screen": "a quiet pane"}), &json!({}));
    let asked = wire.ask(
        &STALL,
        Some(workspace.path()),
        body.clone(),
        Duration::from_secs(5),
    );
    assert_eq!(asked.spent.model.as_deref(), Some("jev-1.13.0"));
    let mut row = json!({"outcome": "answered"});
    asked.spent.stamp(&mut row);
    assert_eq!(row[MODEL.canonical], json!("jev-1.13.0"));
    assert_eq!(row[REQUESTS_KEY], json!(1));
    assert_eq!(row[REDACTED_LINES_KEY], json!(0));
    let sent = endpoint.asked();
    let sent: Value = serde_json::from_str(sent[0].split("\r\n\r\n").nth(1).expect("a body"))
        .expect("the body is JSON");
    assert_eq!(
        sent["model"],
        json!("jev-1.13.0"),
        "the request asked the pin"
    );

    // Refused at the door: no settings consent to nothing, and nothing
    // answered the ask.
    let refused = Wire::at(&endpoint.base(), "test-key", None).ask(
        &STALL,
        Some(workspace.path()),
        body.clone(),
        Duration::from_secs(5),
    );
    assert!(refused.answer.is_err());
    let mut row = json!({"outcome": "not_consented"});
    refused.spent.stamp(&mut row);
    assert_eq!(refused.spent.model, None);
    assert!(row.get(MODEL.canonical).is_none(), "{row}");

    // Past the wall: an answer that arrives too late is no answer.
    let slow = Endpoint::serving("HTTP/1.1 200 OK", answer.to_string(), 500);
    let late = Wire::at(&slow.base(), "test-key", Some(settings)).ask(
        &STALL,
        Some(workspace.path()),
        body,
        Duration::from_millis(100),
    );
    assert_eq!(late.answer, Err(TIMEOUT.to_string()));
    assert_eq!(late.spent.model, None);
}

/// Every request `endpoint` has heard once it has heard `many` of them, or
/// once three seconds have passed: a warm-up's own request is sent from a
/// task of its own, so it lands a moment after the call that asked for it.
fn heard(endpoint: &Endpoint, many: usize) -> Vec<String> {
    let began = std::time::Instant::now();
    while endpoint.asked().len() < many && began.elapsed() < Duration::from_secs(3) {
        thread::sleep(Duration::from_millis(20));
    }
    endpoint.asked()
}

/// A warm-up is one bare GET of the base — no key, no words — whose only
/// product is the pooled socket the first question rides; and a wire with
/// no key sends none, as it would ask nothing (the same "no key, no socket"
/// the question keeps).
#[test]
fn a_warm_up_sends_one_bare_get_and_a_keyless_wire_sends_none() {
    let endpoint = Endpoint::serving("HTTP/1.1 404 Not Found", String::new(), 0);
    Wire::at(&endpoint.base(), "", None).warm();
    thread::sleep(Duration::from_millis(300));
    assert!(endpoint.asked().is_empty(), "no key, no socket");

    Wire::at(&endpoint.base(), "test-key", None).warm();
    let asked = heard(&endpoint, 1);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].starts_with("GET / HTTP/1.1"), "{}", asked[0]);
    assert!(
        !asked[0].contains("Authorization") && !asked[0].contains("test-key"),
        "a warm-up carries no key: {}",
        asked[0]
    );
    assert!(
        asked[0].ends_with("\r\n\r\n"),
        "a warm-up carries no body: {}",
        asked[0]
    );
}

/// The doors' warm-up (t-5535) is that same warm-up, reached from a window
/// that is only booting or a pane that is only opening: with a key it opens
/// one socket, and without one it opens none — a door cannot send what a
/// question could not.
#[test]
fn the_doors_warm_up_opens_one_socket_and_none_without_a_key() {
    let endpoint = Endpoint::serving("HTTP/1.1 404 Not Found", String::new(), 0);
    warm_off_thread(Wire::at(&endpoint.base(), "", None));
    thread::sleep(Duration::from_millis(300));
    assert!(endpoint.asked().is_empty(), "no key, no socket");

    warm_off_thread(Wire::at(&endpoint.base(), "test-key", None));
    let asked = heard(&endpoint, 1);
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].starts_with("GET / HTTP/1.1"), "{}", asked[0]);
}

/// The second door in a minute sends nothing: the first door's socket is
/// still in the pool, and a GET per door is the cost this saves. The window
/// where that is true is the pool's own ([`POOL_IDLE`]), so the skip and the
/// socket cannot disagree.
#[test]
fn a_second_warm_up_inside_the_pools_idle_window_opens_no_socket() {
    let endpoint = Endpoint::serving("HTTP/1.1 404 Not Found", String::new(), 0);
    Wire::at(&endpoint.base(), "test-key", None).warm();
    assert_eq!(
        heard(&endpoint, 1).len(),
        1,
        "the first door opens a socket"
    );

    // A second door, with a wire of its own — what is remembered is the
    // origin's socket, not one wire's idea of it.
    Wire::at(&endpoint.base(), "test-key", None).warm();
    thread::sleep(Duration::from_millis(300));
    let asked = endpoint.asked();
    assert_eq!(asked.len(), 1, "a live socket was warmed twice: {asked:?}");
}

/// The whole road, end to end: a seat left on `auto` records, is judged by
/// its own writer at its own cadence, writes the rise into the ledger it was
/// decided on, and is read back as acting — the claim `auto` makes, held
/// here because until 2026-09-22 no ledger on this machine carried a single
/// transition row in 1,147 requests (t-5875).
///
/// The placement seat, because it is the one whose rows say what the road is
/// for: 38 real requests, one timeout, and marks that are hindsight — the
/// pane left where it was put — which the seat waits a window of before it
/// may rise (t-6155 F1): its own rows alone never carry it up.
#[test]
fn a_seat_on_auto_rises_on_its_own_rows_and_is_read_back_as_acting() {
    use serde_json::json;
    use zerocode_core::jev::promote::{
        ROSE, Stand, marks_that_can_clear, stand_from, window_wanted_for,
    };
    use zerocode_core::jev::{JevMode, PLACEMENT};

    let home = tempfile::tempdir().expect("a zo home");
    let ledger = home.path().join(PLACEMENT.ledger);
    let wanted = window_wanted_for(&PLACEMENT).expect("placement rises");
    let answered = |at: i64| json!({"at": at, "outcome": "answered", "elapsedMs": 300, "requests": 1, "applied": true});

    // One short of the window: nothing is judged, because nothing could be.
    for at in 0..wanted as i64 - 1 {
        record_rows(&PLACEMENT, &ledger, &[answered(at)], at);
    }
    // The marks the seat's own later facts wrote, dated inside the window:
    // the three the label said no to (t-6342), enough agreeing ones to bound
    // above its budget with those inside, and today's room beside each.
    let misses = PLACEMENT.negatives_wanted.expect("placement rises");
    let marks = marks_that_can_clear(&PLACEMENT).expect("a width the budget can be cleared on");
    let labels: Vec<serde_json::Value> = (0..marks)
        .map(|n| json!({"at": 1, "label": format!("placement-{n}"), "agreed": n >= misses, "baselineAgreed": n % 2 == 0}))
        .collect();
    record_rows(&PLACEMENT, &ledger, &labels, 1);
    assert_eq!(
        stand_from(&read_rows(&ledger)),
        Stand::Recording,
        "marks alone do not fill the window"
    );
    assert_eq!(stand_from(&read_rows(&ledger)), Stand::Recording);
    assert!(
        !read_rows(&ledger)
            .iter()
            .any(|row| row["transition"] == json!(ROSE))
    );

    // The row that fills it: the writer judges, and the rise is written
    // beside the rows it was decided on.
    record_rows(
        &PLACEMENT,
        &ledger,
        &[answered(wanted as i64)],
        wanted as i64,
    );
    let rows = read_rows(&ledger);
    let rose = rows
        .iter()
        .find(|row| row["transition"] == json!(ROSE))
        .expect("the seat rose on its own rows");
    assert_eq!(rose["rows"], json!(wanted));
    assert_eq!(rose["answered"], json!(wanted));
    assert_eq!(stand_from(&rows), Stand::Applying);
    assert!(JevMode::Auto.applies_with(true), "and `auto` acts on it");
    assert!(
        !JevMode::Shadow.applies_with(true),
        "a person's word outranks it"
    );

    // One miss inside the window is forgiven; three failures running are not,
    // and an acting seat does not wait for the next cadence to stop.
    record_rows(
        &PLACEMENT,
        &ledger,
        &[json!({"at": 1, "outcome": "timeout", "requests": 1})],
        1,
    );
    assert_eq!(stand_from(&read_rows(&ledger)), Stand::Applying);
    for at in 2..4 {
        record_rows(
            &PLACEMENT,
            &ledger,
            &[json!({"at": at, "outcome": "timeout", "requests": 1})],
            at,
        );
    }
    let rows = read_rows(&ledger);
    let fell = rows.last().expect("a row");
    assert_eq!(fell["transition"], json!(zerocode_core::jev::promote::FELL));
    assert_eq!(fell["line"], json!("fallbacks"));
    assert_eq!(stand_from(&rows), Stand::Recording);
}
