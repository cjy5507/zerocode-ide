//! The fake System One every zo seat's tests ask — one scripted HTTP answer on
//! a loopback port that records each request body it saw — and the machine
//! such a test stands on: a config home with one consented workspace, a key,
//! and the mock's origin in the environment for the length of the test.
//!
//! One copy, shared: the recall seat wrote the first, the agent tool seat
//! needed the second, and two fakes of one wire would be two contracts.
//! `std::net` on a thread, because the tools crate's tokio has no network
//! driver of its own — the client brings its own.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zerocode_core::jev::JevUse;

/// One scripted HTTP answer on a loopback port, recording each request
/// body it saw. `std::net` on a thread, because the tools crate's tokio
/// has no network driver of its own — the client brings its own.
pub(super) struct Mock {
    pub(super) base_url: String,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    /// A port that accepts and never answers — a judgment that misses any
    /// wall put in front of it.
    pub(super) fn silent() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
        let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        std::thread::spawn(move || {
            let held: Vec<std::net::TcpStream> = listener.incoming().flatten().collect();
            drop(held);
        });
        Self { base_url, bodies }
    }

    /// A port whose first answer is `delay` late and whose later answers
    /// come at once — one judgment slow on the wire, its second copy not.
    ///
    /// Each connection is answered on its own thread, because a hedge
    /// holds two of them open at the same moment and a server that
    /// answered them in turn would be measuring itself.
    pub(super) fn slow_first(delay: Duration, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
        let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&bodies);
        std::thread::spawn(move || {
            for (nth, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { break };
                let recorder = Arc::clone(&recorder);
                let body = body.clone();
                std::thread::spawn(move || {
                    let request = read_request(&mut stream);
                    if let Ok(mut seen) = recorder.lock() {
                        seen.push(request);
                    }
                    if nth == 0 {
                        std::thread::sleep(delay);
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                });
            }
        });
        Self { base_url, bodies }
    }

    /// A port that reads each request and answers it with whatever `answer`
    /// makes of the request body — a status and a body — so a test whose
    /// question is cut into several requests can answer each with the ids it
    /// asked under. Each connection is answered on its own thread, because
    /// the shards leave together.
    pub(super) fn answering(answer: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
        let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&bodies);
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let recorder = Arc::clone(&recorder);
                let answer = Arc::clone(&answer);
                std::thread::spawn(move || {
                    let request = read_request(&mut stream);
                    let (status, body) = answer(&request);
                    if let Ok(mut seen) = recorder.lock() {
                        seen.push(request);
                    }
                    let response = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                });
            }
        });
        Self { base_url, bodies }
    }

    pub(super) fn serving(status: u16, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
        let base_url = format!("http://{}", listener.local_addr().expect("mock address"));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&bodies);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let request = read_request(&mut stream);
                if let Ok(mut seen) = recorder.lock() {
                    seen.push(request);
                }
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { base_url, bodies }
    }

    pub(super) fn requests(&self) -> Vec<String> {
        self.bodies.lock().map(|seen| seen.clone()).unwrap_or_default()
    }
}

/// The body of one HTTP/1.1 request: headers up to the blank line, then
/// exactly `Content-Length` bytes.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            return String::from_utf8_lossy(&buffer).into_owned();
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).unwrap_or(0);
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8_lossy(&buffer[header_end..]).into_owned()
}


/// The whole of what a zo seat reads: a config home holding one consented
/// workspace with `seat`'s switch set to `mode`, a key, and a mock origin —
/// the environment held for the length of `body`.
pub(super) fn machine<T>(seat: &JevUse, mode: &str, base_url: &str, body: impl FnOnce(&Path) -> T) -> T {
    machine_at(seat, mode, Some(base_url), body)
}

/// The same machine on the REAL wire: the config home and the consented
/// workspace are the test's, the key and the origin are the environment's
/// own — what an `#[ignore]` measurement stands on.
pub(super) fn machine_live<T>(seat: &JevUse, mode: &str, body: impl FnOnce(&Path) -> T) -> T {
    machine_at(seat, mode, None, body)
}

fn machine_at<T>(seat: &JevUse, mode: &str, wire: Option<&str>, body: impl FnOnce(&Path) -> T) -> T {
    let home = tempfile::tempdir().expect("a config home");
    let work = tempfile::tempdir().expect("a workspace");
    // As the filesystem spells it, which is how the door spells a cwd.
    let cwd = std::fs::canonicalize(work.path()).expect("the workspace resolved");
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({
            zerocode_core::jev::SMART_SETTINGS_KEY: {
                seat.setting: mode,
                "jev": {"enabled": true, "workspaces": [cwd.to_string_lossy()]},
            }
        })
        .to_string(),
    )
    .expect("a settings file");
    let mut env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", &home.path().to_string_lossy())
        .set_also("ZO_HOME", home.path())
        .set_also(core_types::paths::ZO_STATE_DIR_ENV, home.path());
    if let Some(base_url) = wire {
        env = env
            .set_also("HOME", home.path())
            .set_also(api::SYSTEMONE_API_KEY_ENV, "test-key")
            .set_also(api::SYSTEMONE_BASE_URL_ENV, base_url);
    }
    let _env = env;
    body(&cwd)
}
