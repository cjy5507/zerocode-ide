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
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zerocode_core::jev::JevUse;

/// Changes the exact temporary settings file a test bound before its HTTP
/// reply. No background callback resolves a config home after the test ends.
#[derive(Clone)]
pub(super) struct SettingsChange {
    path: Arc<Mutex<Option<PathBuf>>>,
    setting: &'static str,
    change: &'static str,
}

impl SettingsChange {
    pub(super) fn new(seat: &JevUse, change: &'static str) -> Self {
        Self { path: Arc::new(Mutex::new(None)), setting: seat.setting, change }
    }

    pub(super) fn bind(&self, home: &Path) {
        *self.path.lock().unwrap() = Some(home.join("settings.json"));
    }

    pub(super) fn apply(&self) {
        if self.change == "none" { return; }
        let path = self.path.lock().unwrap().clone().expect("bind the temporary settings before asking");
        let mut root: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match self.change {
            "global_off" => root["smart"]["jev"]["enabled"] = serde_json::json!(false),
            "consent" => root["smart"]["jev"]["workspaces"] = serde_json::json!([]),
            "model" => root["smart"]["jevModel"] = serde_json::json!("jev-new-pin"),
            word => root["smart"][self.setting] = serde_json::json!(word),
        }
        std::fs::write(path, root.to_string()).unwrap();
    }
}

pub(super) const APPLICATION_CHANGES: [&str; 6] = ["none", "off", "shadow", "global_off", "consent", "model"];

/// One scripted HTTP answer on a loopback port, recording each request
/// body it saw. `std::net` on a thread, because the tools crate's tokio
/// has no network driver of its own — the client brings its own.
///
/// Dropping the mock closes its port: the accept thread is woken, sees the stop
/// flag and ends, so the listener does not outlive the test that asked for it.
/// Without that, every mock a lib run builds holds one descriptor until the
/// process exits, and a few hundred of them exhaust the macOS default limit of
/// 256 (t-20571).
pub(super) struct Mock {
    pub(super) base_url: String,
    bodies: Arc<Mutex<Vec<String>>>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the parked `accept` so the thread observes the flag. Not joined:
        // a client that connected and never wrote would park the thread in
        // `read_request`, and the test must not wait on it.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
    }
}

/// A mock's loopback port as first bound: the listener for its accept thread
/// and the parts [`Mock`] keeps.
struct Bound {
    listener: TcpListener,
    base_url: String,
    addr: SocketAddr,
    bodies: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Mock {
    fn bind() -> Bound {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the mock");
        let addr = listener.local_addr().expect("mock address");
        Bound {
            listener,
            base_url: format!("http://{addr}"),
            addr,
            bodies: Arc::new(Mutex::new(Vec::new())),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A port that accepts and never answers — a judgment that misses any
    /// wall put in front of it.
    pub(super) fn silent() -> Self {
        let Bound { listener, base_url, addr, bodies, stop } = Self::bind();
        let watching = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut held: Vec<TcpStream> = Vec::new();
            for stream in listener.incoming().flatten() {
                if watching.load(Ordering::SeqCst) {
                    break;
                }
                held.push(stream);
            }
            drop(held);
        });
        Self { base_url, bodies, addr, stop }
    }

    /// A port whose first answer is `delay` late and whose later answers
    /// come at once — one judgment slow on the wire, its second copy not.
    ///
    /// Each connection is answered on its own thread, because a hedge
    /// holds two of them open at the same moment and a server that
    /// answered them in turn would be measuring itself.
    pub(super) fn slow_first(delay: Duration, body: String) -> Self {
        let Bound { listener, base_url, addr, bodies, stop } = Self::bind();
        let recorder = Arc::clone(&bodies);
        let watching = Arc::clone(&stop);
        std::thread::spawn(move || {
            for (nth, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { break };
                if watching.load(Ordering::SeqCst) {
                    break;
                }
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
        Self { base_url, bodies, addr, stop }
    }

    /// A port that reads each request and answers it with whatever `answer`
    /// makes of the request body — a status and a body — so a test whose
    /// question is cut into several requests can answer each with the ids it
    /// asked under. Each connection is answered on its own thread, because
    /// the shards leave together.
    pub(super) fn answering(answer: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Self {
        let Bound { listener, base_url, addr, bodies, stop } = Self::bind();
        let recorder = Arc::clone(&bodies);
        let watching = Arc::clone(&stop);
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                if watching.load(Ordering::SeqCst) {
                    break;
                }
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
        Self { base_url, bodies, addr, stop }
    }

    pub(super) fn serving(status: u16, body: String) -> Self {
        let Bound { listener, base_url, addr, bodies, stop } = Self::bind();
        let recorder = Arc::clone(&bodies);
        let watching = Arc::clone(&stop);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                if watching.load(Ordering::SeqCst) {
                    break;
                }
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
        Self { base_url, bodies, addr, stop }
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
    machine_with(&[(seat.setting, mode)], wire, body)
}

/// [`machine`] with the seat words its settings file holds given whole —
/// none, one or several, as `(setting, word)` — for a test of how two
/// seats' words read together (t-6877: the skill suggestion keeps the
/// search's word until it has one of its own).
pub(super) fn machine_words<T>(words: &[(&str, &str)], base_url: &str, body: impl FnOnce(&Path) -> T) -> T {
    machine_with(words, Some(base_url), body)
}

fn machine_with<T>(words: &[(&str, &str)], wire: Option<&str>, body: impl FnOnce(&Path) -> T) -> T {
    let home = tempfile::tempdir().expect("a config home");
    let work = tempfile::tempdir().expect("a workspace");
    // As the filesystem spells it, which is how the door spells a cwd.
    let cwd = std::fs::canonicalize(work.path()).expect("the workspace resolved");
    let mut smart: serde_json::Map<String, serde_json::Value> =
        words.iter().map(|(setting, word)| ((*setting).to_string(), serde_json::Value::from(*word))).collect();
    smart.insert("jev".to_string(), serde_json::json!({"enabled": true, "workspaces": [cwd.to_string_lossy()]}));
    std::fs::write(
        home.path().join("settings.json"),
        serde_json::json!({ zerocode_core::jev::SMART_SETTINGS_KEY: smart }).to_string(),
    )
    .expect("a settings file");
    let mut env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", &home.path().to_string_lossy())
        .set_also("ZO_HOME", home.path())
        .set_also("ZO_PROVIDER_SKILLS", "0")
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

#[cfg(test)]
mod tests {
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use super::Mock;

    /// Whether `base_url`'s port still takes a connection.
    fn still_listening(base_url: &str) -> bool {
        let addr = base_url.trim_start_matches("http://").parse().expect("a socket address");
        TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
    }

    /// A mock that has been dropped must have given its port back. Each one
    /// that stayed bound held a descriptor until the process exited, and the
    /// 109 the tools lib builds exhausted the macOS default limit of 256 for
    /// the tests that spawn a shell (t-20571).
    #[test]
    fn a_dropped_mock_closes_its_port() {
        let ports = [
            Mock::silent().base_url.clone(),
            Mock::slow_first(Duration::ZERO, "{}".to_string()).base_url.clone(),
            Mock::answering(|_| (200, "{}".to_string())).base_url.clone(),
            Mock::serving(200, "{}".to_string()).base_url.clone(),
        ];
        let deadline = Instant::now() + Duration::from_secs(5);
        for base_url in &ports {
            while still_listening(base_url) {
                assert!(Instant::now() < deadline, "{base_url} still takes connections after its mock was dropped");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
