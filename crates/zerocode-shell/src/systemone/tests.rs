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
