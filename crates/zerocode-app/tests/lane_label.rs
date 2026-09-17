//! The `zerocode` binary, driven end to end against a mock session server.
//!
//! Everything else in this workspace is tested one crate at a time, which is
//! exactly why the wiring bug this file pins was invisible: every unit passed
//! while the app handed the wrong credential to the one call that names a
//! session. So this drives the real binary, with a real `PATH`, and reads what
//! a person would actually see on their screen.

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// The question the mock session was opened with, and therefore the name the
/// screen must show instead of the id.
const QUESTION: &str = "드레인 게이트 봉인 리팩터링";
const SESSION_ID: &str = "session-1785722030389-0";

/// A session server that answers the two-step probe and one subscribe.
///
/// It accepts **any** non-empty token on purpose: the app mints its own when
/// the environment has none, so the test cannot know the secret in advance —
/// and "the app presented some token" is precisely what is under test.
fn mock_serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            thread::spawn(move || serve_connection(stream));
        }
    });
    addr
}

fn serve_connection(stream: TcpStream) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
            break;
        };
        let id = request["id"].as_u64().unwrap_or(0);
        let presented = request
            .get("token")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.is_empty());

        // The identifying exchange carries no token and must be refused with
        // -32002 — that is how `probe()` learns this is a session server at all.
        let reply = if presented.is_none() {
            serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32002, "message": "unauthorized" }
            })
        } else {
            match request["method"].as_str().unwrap_or_default() {
                "session.list" => serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "sessions": [SESSION_ID] }
                }),
                // The shape the real server sends: `{role, text}` per entry,
                // opening with the system prompt.
                "session.subscribe" => serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {
                        "id": SESSION_ID,
                        "history": [
                            { "role": "system", "text": "you are zo" },
                            { "role": "user", "text": QUESTION },
                            { "role": "assistant", "text": "on it" },
                        ],
                        "next_seq": 3,
                        "helm": null,
                    }
                }),
                other => serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": format!("unknown: {other}") }
                }),
            }
        };
        let mut out = reply.to_string();
        out.push('\n');
        if writer.write_all(out.as_bytes()).is_err() {
            break;
        }
        let _ = writer.flush();
    }
}

/// A `zo` that stays alive long enough for the lane to open and the label line
/// to be written. A binary that exits at once would end the process through the
/// `pumped.ended` path before there is anything to read.
fn fake_zo(dir: &Path) -> String {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin dir");
    let path = bin.join("zo");
    fs::write(
        &path,
        "#!/bin/sh\nprintf 'fake zo: %s\\n' \"$*\"\nsleep 5\n",
    )
    .expect("write fake zo");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    bin.to_string_lossy().into_owned()
}

/// The defect this pins: `zerocode lane` mints its own token when the
/// environment has none, and the code that names the session used to re-read
/// only the environment. It therefore subscribed with no credential at all, the
/// server refused it, and the name fell back to the id — on the **default**
/// path, the one every user without an exported token takes.
#[test]
fn a_lane_is_named_after_its_first_question_even_when_the_token_was_minted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addr = mock_serve();
    let bin_dir = fake_zo(dir.path());

    let mut child = Command::new(env!("CARGO_BIN_EXE_zerocode"))
        .args(["lane", SESSION_ID, "--bind", &addr])
        .env("PATH", &bin_dir)
        // Deliberately absent: this is the path where the app mints its own.
        .env_remove("ZO_SERVE_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn zerocode");

    let mut stderr = child.stderr.take().expect("stderr");
    let collected = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = std::sync::Arc::clone(&collected);
    let reader = thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        *sink.lock().expect("lock") = buffer;
    });

    // Closed stdin takes the detach path, so this exits on its own; the
    // deadline is only so a regression cannot hang the suite.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    reader.join().expect("join stderr reader");

    let stderr = collected.lock().expect("lock").clone();
    assert!(
        stderr.contains(QUESTION),
        "the screen must name the session after what was asked, not its id.\n{stderr}"
    );
    assert!(
        stderr.contains(SESSION_ID),
        "the id has to stay addressable alongside the name.\n{stderr}"
    );
}
