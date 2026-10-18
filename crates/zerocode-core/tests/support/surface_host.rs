// An isolated HTTP host for exercising the installed shim without desktop input.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use zerocode_core::agent_shims::SurfaceShim;

pub struct SurfaceHost {
    pub port: u16,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SurfaceHost {
    pub fn start(
        directory: &Path,
        sequence: Option<Vec<serde_json::Value>>,
        shim: SurfaceShim,
    ) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        let program = directory.join(shim.name());
        std::fs::write(
            &program,
            zerocode_core::agent_shims::installed_script(shim, false),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stop);
        let directory = directory.to_path_buf();
        let worker = std::thread::spawn(move || {
            let mut cursor = 0;
            while !done.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => serve(
                        &mut stream,
                        &directory,
                        sequence.as_deref(),
                        &mut cursor,
                        shim,
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture listener failed: {error}"),
                }
            }
        });
        Self {
            port,
            stop,
            worker: Some(worker),
        }
    }

    pub fn environment(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("ZEROCODE_HOOK_ENDPOINT", None),
            ("ZEROCODE_COMPUTER_TOKEN_FILE", None),
            ("ZEROCODE_BROWSER_TOKEN_FILE", None),
            ("ZEROCODE_PANE_KEY", None),
            ("ZEROCODE_SECOND_BRAIN", None),
            ("ZEROCODE_HOOK_PORT", Some(self.port.to_string())),
            ("ZEROCODE_HOOK_TOKEN", Some("fixture-hook".into())),
            ("ZEROCODE_COMPUTER_TOKEN", Some("fixture-computer".into())),
            ("ZEROCODE_BROWSER_TOKEN", Some("fixture-browser".into())),
        ]
    }
}

impl Drop for SurfaceHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            if !std::thread::panicking() {
                worker.join().unwrap();
            }
        }
    }
}

fn serve(
    stream: &mut TcpStream,
    directory: &Path,
    sequence: Option<&[serde_json::Value]>,
    cursor: &mut usize,
    shim: SurfaceShim,
) {
    let (route, capability_header, execution_header) = match shim {
        SurfaceShim::Computer => (
            "POST /computer ",
            "x-zerocode-computer-token: fixture-computer",
            "",
        ),
        SurfaceShim::Browser => (
            "POST /browser ",
            "x-zerocode-browser-token: fixture-browser",
            "x-zerocode-execution: 1\r\n",
        ),
    };
    let request = request(stream);
    assert!(request.0.starts_with(route));
    let headers = request.0.to_ascii_lowercase();
    assert!(headers.contains("x-zerocode-hook-token: fixture-hook"));
    assert!(
        headers.contains(capability_header),
        "the installed shim must send its separate surface capability"
    );
    let words = request.1.split('\u{1f}').collect::<Vec<_>>();
    if matches!(shim, SurfaceShim::Computer) {
        assert!(
            words.windows(2).any(|pair| pair == ["--app", "Fixture"]),
            "every Computer request must retain the selected app"
        );
        assert!(
            words.windows(2).any(|pair| pair == ["--window-id", "41"]),
            "every Computer request must retain the selected window"
        );
    } else {
        assert!(
            words.contains(&"browser-1"),
            "the Browser request must retain its working-tab label"
        );
    }
    let verb = words.into_iter().find(|word| !word.is_empty()).unwrap();
    let mut calls = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("calls"))
        .unwrap();
    writeln!(calls, "{verb}").unwrap();
    let answer = if let Some(sequence) = sequence {
        assert!(
            *cursor < sequence.len(),
            "an unexpected host call followed the fixed trace: {verb}"
        );
        assert_eq!(
            sequence[*cursor]["verb"], verb,
            "the installed shim must follow the fixed host trace"
        );
        let answer = sequence[*cursor]["answer"].clone();
        *cursor += 1;
        answer.to_string()
    } else {
        std::fs::read_to_string(directory.join(format!("reply-{verb}"))).unwrap()
    };
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{execution_header}\r\n{answer}", answer.len()).unwrap();
}

fn request(stream: &mut TcpStream) -> (String, String) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let end = loop {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "a fixture request must contain complete headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|word| word == b"\r\n\r\n") {
            break end + 4;
        }
        assert!(bytes.len() < 65_536, "fixture headers must stay bounded");
    };
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(length <= 256_000, "fixture payload must stay bounded");
    while bytes.len() < end + length {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0, "a fixture request must contain the complete body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    (
        headers,
        String::from_utf8(bytes[end..end + length].to_vec()).unwrap(),
    )
}
