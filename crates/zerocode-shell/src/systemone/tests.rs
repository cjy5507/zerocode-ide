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

/// A loopback System One that answers every request it is sent with one
/// status and one body, and remembers each request as it arrived.
pub(crate) struct Endpoint {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Endpoint {
    /// Answer `status` with `body`, after `hold_ms` milliseconds each time.
    pub(crate) fn serving(status: &'static str, body: String, hold_ms: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback seat");
        let addr = listener.local_addr().expect("its address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&seen);
        thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else {
                    return;
                };
                let request = read_request(&mut socket);
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
