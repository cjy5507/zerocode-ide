use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub struct OwnProviders {
    previous: Vec<(&'static str, Option<OsString>)>,
    _home: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
}

impl OwnProviders {
    pub fn new() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = [api::CUSTOM_PROVIDERS_ENV, api::MODEL_CONTEXT_WINDOWS_ENV,
            "ZO_DISABLE_MODEL_DISCOVERY", "ZO_DISABLE_KEYCHAIN", "HOME", "ZO_HOME", "ZO_CONFIG_HOME"]
            .into_iter().map(|key| (key, std::env::var_os(key))).collect();
        let home = tempfile::tempdir().unwrap();
        for key in ["HOME", "ZO_HOME", "ZO_CONFIG_HOME"] {
            std::env::set_var(key, home.path());
        }
        std::env::remove_var(api::CUSTOM_PROVIDERS_ENV);
        std::env::remove_var(api::MODEL_CONTEXT_WINDOWS_ENV);
        std::env::remove_var("ZO_DISABLE_MODEL_DISCOVERY");
        std::env::set_var("ZO_DISABLE_KEYCHAIN", "1");
        api::refresh_custom_providers_from_json("[]").unwrap();
        Self { previous, _home: home, _lock: lock }
    }

    pub fn install(rows: &Value) {
        api::refresh_custom_providers_from_json(&rows.to_string()).unwrap();
    }
}

impl Drop for OwnProviders {
    fn drop(&mut self) {
        // Clear while the isolated home is still active; restoring the ambient
        // provider store could otherwise consult the person's saved keys.
        api::refresh_custom_providers_from_json("[]").unwrap();
        for (key, previous) in &self.previous {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

pub struct Server {
    pub base: String,
    pub requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    pub fn new(status: &str, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback fixture");
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let status = status.to_string();
        let body = body.to_string();
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 { break; }
                    bytes.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                        let length = head.lines().find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                        }).unwrap_or(0);
                        if rest.len() >= length { break; }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                let (status, body) = if request.starts_with("POST ") {
                    ("200 OK", r#"{"id":"fixture","model":"vendor/model","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
                } else { (status.as_str(), body.as_str()) };
                captured.lock().unwrap().push(request);
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { base, requests, stop, worker: Some(worker) }
    }

    pub fn provider(&self, name: &str, models: Value) -> Value {
        let mut provider = json!({"name": name, "base_url": self.base, "requires_auth": false});
        provider["models"] = models;
        provider
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

pub fn read(server: &Server) -> Result<Vec<String>, api::ApiError> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(
        api::discover_models_with_bearer(&server.base, "fixture-key")
    )
}

/// The fixture's own checks, over bare sockets: it answers a request only once
/// it has read it whole. They touch neither `provider` nor `read`, so a test
/// file that shares only the provider guard still leaves those unused.
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::net::{Shutdown, TcpStream};

    fn connect(server: &Server) -> TcpStream {
        let address = server.base.trim_start_matches("http://").trim_end_matches("/v1");
        let stream = TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream
    }

    /// Whatever comes back until the fixture closes, or the error that ended it.
    fn answer(stream: &mut TcpStream) -> String {
        let mut text = String::new();
        match stream.read_to_string(&mut text) {
            Ok(_) => text,
            Err(error) => format!("{text}<{error}>"),
        }
    }

    #[test]
    fn a_request_sent_a_moment_after_connecting_is_read_before_it_is_answered() {
        let server = Server::new("200 OK", r#"{"data":[]}"#);
        let mut client = connect(&server);
        client.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut early = [0; 512];
        let answered_early = match client.read(&mut early) {
            Ok(read) => Some(String::from_utf8_lossy(&early[..read]).into_owned()),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => None,
            Err(error) => Some(format!("<{error}>")),
        };
        assert_eq!(answered_early, None, "the fixture answered before the request came");
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let body = r#"{"model":"vendor/model","messages":[]}"#;
        let request = format!("POST /v1/chat/completions HTTP/1.1\r\nHost: fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let reply = match client.write_all(request.as_bytes()) {
            Ok(()) => answer(&mut client),
            Err(error) => format!("<{error}>"),
        };
        assert!(reply.starts_with("HTTP/1.1 200 OK") && reply.contains(r#""id":"fixture""#), "not the answer to the POST: {reply}");
        assert_eq!(*server.requests.lock().unwrap(), [request]);
    }

    #[test]
    fn a_connection_that_ends_without_a_request_is_closed_unanswered_and_unrecorded() {
        let server = Server::new("200 OK", r#"{"data":[]}"#);
        let mut silent = connect(&server);
        silent.shutdown(Shutdown::Write).unwrap();
        assert_eq!(answer(&mut silent), "", "the fixture answered a connection that sent nothing");
        let request = "GET /v1/models HTTP/1.1\r\nHost: fixture\r\n\r\n";
        let mut client = connect(&server);
        let reply = match client.write_all(request.as_bytes()) {
            Ok(()) => answer(&mut client),
            Err(error) => format!("<{error}>"),
        };
        assert!(reply.starts_with("HTTP/1.1 200 OK") && reply.ends_with(r#"{"data":[]}"#), "the next connection: {reply}");
        assert_eq!(*server.requests.lock().unwrap(), [request]);
    }
}
