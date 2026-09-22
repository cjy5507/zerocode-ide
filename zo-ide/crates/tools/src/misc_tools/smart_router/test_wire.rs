//! A scripted System One on a loopback port, for the seats' hermetic tests:
//! one HTTP/1.1 answer per connection and every request body recorded, so a
//! test can say what left the machine. `std::net` on a thread, because the
//! tools crate's tokio has no network driver of its own — the client brings
//! its own. Shared by every seat's tests rather than copied into each.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
pub(super) fn read_request(stream: &mut std::net::TcpStream) -> String {
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

