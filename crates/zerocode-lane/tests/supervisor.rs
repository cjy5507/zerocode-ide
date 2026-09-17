//! The supervisor is the only place ZeroCode runs `zo` or trusts a port, so
//! these tests drive two doubles:
//!
//! - a **fake `zo`** that records the arguments and environment it was called
//!   with, proving the spawn path without needing `zo` installed;
//! - a **mock session server** that speaks the real line-delimited JSON-RPC,
//!   so "is this a session server?", "does it take our token?" and "does the
//!   session survive a client leaving?" are answered by the protocol rather
//!   than by a TCP connect succeeding.

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use zerocode_core::{HostError, PtyCwd, PtySpec, RemotePath};
use zerocode_lane::{
    LaneError, LaneSupervisor, LocalPty, PtyHandle, PtySpawner, PtyTransport, PtyTransportError,
    ServeProbe, ServeState, TOKEN_ENV, discover_pane_channel, pane_channel_file,
};
use zerocode_pty::{ZO_EVENTS_ADDR_FILE_ENV, ZO_EXECUTABLE, ZoBinary};

const TOKEN: &str = "supervisor-test-secret";

/// A hand-started interactive Zo publishes the private channel that belongs to
/// its process under the app's runtime directory. The process id is discovery;
/// the two complete lines are the address and the credential needed to use it.
#[test]
fn a_pid_address_file_discovers_one_private_zo_channel() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    let pid = 42_424;
    let path = pane_channel_file(runtime.path(), pid);
    fs::write(&path, format!("127.0.0.1:49152\n{TOKEN}\n")).expect("fake address file");

    let found = discover_pane_channel(runtime.path(), pid)
        .expect("read address file")
        .expect("published channel");

    assert_eq!(
        (found.addr.as_str(), found.token.as_deref()),
        ("127.0.0.1:49152", Some(TOKEN))
    );
}

struct RecordingSpawner {
    seen: Arc<Mutex<Vec<PtySpec>>>,
}

impl PtySpawner for RecordingSpawner {
    fn spawn(self: Box<Self>, spec: &PtySpec) -> Result<PtyHandle, PtyTransportError> {
        self.seen.lock().expect("recording lock").push(spec.clone());
        Err(HostError::Cancelled.into())
    }
}

// ------------------------------------------------------------------- doubles

/// A `zo` that appends `"<args>|<token>"` to `marker` and echoes its arguments.
/// The marker path is baked into the script so parallel tests never share state.
fn fake_zo(dir: &Path, marker: &Path) -> ZoBinary {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(ZO_EXECUTABLE);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$*\" \"${TOKEN_ENV}\" >> '{}'\n\
             if [ -n \"${ZO_EVENTS_ADDR_FILE_ENV}\" ]; then printf '127.0.0.1:49152\\n%s\\n' \"${TOKEN_ENV}\" > \"${ZO_EVENTS_ADDR_FILE_ENV}\"; fi\n\
             printf 'fake-zo ran: %s pane=%s cwd=%s' \"$*\" \"$ZEROCODE_PANE_KEY\" \"$PWD\"\n",
            marker.display()
        ),
    )
    .expect("write fake zo");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    ZoBinary { path }
}

/// A listener that speaks the session protocol. Its history lives on the
/// server, not in any one connection, which is what a session outliving its
/// client actually means.
struct MockServe {
    addr: String,
    received: Arc<Mutex<Vec<String>>>,
}

fn mock_serve(required_token: Option<&str>, history: &[&str]) -> MockServe {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let required = required_token.map(str::to_string);
    let history: Vec<String> = history.iter().map(|entry| (*entry).to_string()).collect();
    let received = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&received);

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let required = required.clone();
            let history = history.clone();
            let seen = Arc::clone(&seen);
            thread::spawn(move || serve_connection(stream, required.as_deref(), &history, &seen));
        }
    });

    MockServe { addr, received }
}

fn serve_connection(
    stream: TcpStream,
    required_token: Option<&str>,
    history: &[String],
    seen: &Arc<Mutex<Vec<String>>>,
) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { break };
        seen.lock().expect("received lock").push(line.clone());

        let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
            break;
        };
        let id = request["id"].as_u64().unwrap_or(0);
        let presented = request.get("token").and_then(serde_json::Value::as_str);
        let reply = if required_token.is_some() && presented != required_token {
            serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32002, "message": "unauthorized" }
            })
        } else {
            match request["method"].as_str().unwrap_or_default() {
                "session.list" => serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "sessions": ["session-7"] }
                }),
                "session.subscribe" => serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "id": "session-7", "history": history, "next_seq": history.len() }
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

/// A listener that is *not* a session server: it accepts, records whatever it
/// is told, and never answers.
fn foreign_listener() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let received = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&received);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let seen = Arc::clone(&seen);
            thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    seen.lock().expect("received lock").push(line);
                }
            });
        }
    });
    (addr, received)
}

/// An address nothing is listening on: bind to get a free port, then release it.
fn free_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    addr.to_string()
}

fn pump_until(lane: &mut PtyHandle, predicate: impl Fn(&PtyHandle) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        lane.pump();
        if predicate(lane) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn marker_lines(marker: &Path) -> Vec<String> {
    fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The marker once `want` lines have arrived, or whatever is there when the wait
/// runs out.
///
/// The lines are written by a spawned SHELL, so "the supervisor returned" and
/// "the child has run" are two different moments. Reading once made the spawn
/// assertion a race that lost under load — this suite failed intermittently only
/// when the whole workspace ran at once, which is the shape of flake that
/// teaches people to re-run instead of to look. Waiting for the fact keeps the
/// assertion about the spawn rather than about the scheduler; a genuine
/// no-spawn still fails, one wait later.
fn marker_lines_awaiting(marker: &Path, want: usize) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let lines = marker_lines(marker);
        if lines.len() >= want || std::time::Instant::now() >= deadline {
            return lines;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn supervisor_for(zo: ZoBinary, addr: &str) -> LaneSupervisor {
    LaneSupervisor::new(zo, addr, Some(TOKEN.to_string())).expect("loopback supervisor")
}

// ------------------------------------------------------------------- identity

/// A server the user already started — in their own terminal, or by a previous
/// run — must be reused. A second one would split the session pool in two and
/// quietly break "the same session in both places".
#[test]
fn an_already_running_session_server_is_reused_rather_than_restarted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);
    let server = mock_serve(Some(TOKEN), &[]);

    let supervisor = supervisor_for(zo, &server.addr);
    assert_eq!(supervisor.probe(), ServeProbe::Ready);
    assert_eq!(
        supervisor
            .ensure_serve(Duration::from_secs(2))
            .expect("ensure"),
        ServeState::AlreadyRunning
    );
    assert!(
        marker_lines(&marker).is_empty(),
        "nothing may be spawned when a usable server is already listening"
    );

    // The same ordering rule, seen from the server: identify first, then
    // authenticate. If these two were swapped the secret would go to whatever
    // held the port.
    let seen = server.received.lock().expect("received lock").clone();
    assert!(seen.len() >= 2, "expected two exchanges, saw {seen:?}");
    assert!(
        !seen[0].contains(TOKEN),
        "the identifying exchange must carry no token: {}",
        seen[0]
    );
    assert!(
        seen[1].contains(TOKEN),
        "the second exchange must present the token: {}",
        seen[1]
    );
}

/// The security case: any process can hold a loopback port. Answering a TCP
/// connect is not identity, and attaching to whatever squatted there would
/// point `zo attach` — which sends the shared secret as its first request — at
/// a stranger.
#[test]
fn a_plain_listener_is_never_mistaken_for_a_session_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);
    let (addr, _received) = foreign_listener();

    let supervisor = supervisor_for(zo, &addr);
    assert_eq!(supervisor.probe(), ServeProbe::NotSessionServer);
    assert!(!supervisor.serve_is_up());

    assert!(matches!(
        supervisor.ensure_serve(Duration::from_millis(500)),
        Err(LaneError::PortOccupied { .. })
    ));
    assert!(matches!(
        supervisor.open_lane(Box::new(LocalPty), Some("session-7"), None, &[], 10, 40),
        Err(LaneError::PortOccupied { .. })
    ));
    assert!(
        marker_lines(&marker).is_empty(),
        "no `zo` may be spawned against an unidentified port"
    );
}

/// The secret must not be the thing that *discovers* identity. The first
/// exchange carries no token, so a squatter learns nothing it could replay.
#[test]
fn the_token_is_never_sent_to_a_listener_that_has_not_identified_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);
    let (addr, received) = foreign_listener();

    let supervisor = supervisor_for(zo, &addr);
    assert_eq!(supervisor.probe(), ServeProbe::NotSessionServer);

    let lines = received.lock().expect("received lock").clone();
    assert!(!lines.is_empty(), "the probe should have said something");
    for line in &lines {
        assert!(
            !line.contains(TOKEN),
            "the token leaked to an unidentified listener: {line}"
        );
    }
}

/// A real session server that does not share our secret is a *different*
/// failure from a squatter, and must not be attached to either.
#[test]
fn a_session_server_with_a_different_token_is_refused_not_attached() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);
    let server = mock_serve(Some("someone-elses-secret"), &[]);

    let supervisor = supervisor_for(zo, &server.addr);
    assert_eq!(supervisor.probe(), ServeProbe::Unauthorized);
    assert!(matches!(
        supervisor.ensure_serve(Duration::from_millis(500)),
        Err(LaneError::TokenMismatch { .. })
    ));
    assert!(matches!(
        supervisor.open_lane(Box::new(LocalPty), Some("session-7"), None, &[], 10, 40),
        Err(LaneError::TokenMismatch { .. })
    ));
    assert!(marker_lines(&marker).is_empty(), "nothing may be spawned");
}

/// A session server can run any command in the project, so it does not get to
/// listen anywhere but loopback — and there is deliberately no override.
#[test]
fn a_non_loopback_bind_address_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);

    let error = LaneSupervisor::new(zo, "93.184.216.34:8787", Some(TOKEN.to_string()))
        .expect_err("a routable address must be refused");
    assert!(
        matches!(error, LaneError::NotLoopback { .. }),
        "unexpected error: {error}"
    );
}

// --------------------------------------------------------------------- spawn

/// With nothing listening, the supervisor really does spawn `zo serve` — with
/// the bind address *and* the secret passed explicitly rather than inherited by
/// luck. The fake exits without binding, so the honest outcome is a timeout.
#[test]
fn an_absent_server_is_spawned_with_the_serve_arguments_and_the_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);
    let addr = free_addr();

    let supervisor = supervisor_for(zo, &addr);
    assert_eq!(supervisor.probe(), ServeProbe::NotListening);

    let error = supervisor
        .ensure_serve(Duration::from_millis(700))
        .expect_err("a server that never answers must not be reported as ready");
    assert!(
        matches!(error, LaneError::ServeNotReady { .. }),
        "unexpected error: {error}"
    );

    // Awaited rather than read: the only evidence of the spawn is a line the
    // spawned shell appends, and "ensure_serve gave up" happens before "the
    // child got scheduled". The other marker assertions in this file are safe
    // as plain reads — they either follow output the child printed AFTER the
    // append, or assert the marker is empty on a path that spawns nothing.
    let calls = marker_lines_awaiting(&marker, 1);
    assert_eq!(calls.len(), 1, "exactly one spawn: {calls:?}");
    assert_eq!(calls[0], format!("serve --bind {addr}|{TOKEN}"));
}

/// A lane is `zo attach <session>` in a pty — the real client, carrying the
/// secret, the pane key and the worktree.
#[test]
fn opening_a_lane_hosts_zo_attach_with_the_token_and_the_worktree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);
    let worktree = dir.path().join("wt-refactor");
    fs::create_dir_all(&worktree).expect("worktree");
    let server = mock_serve(Some(TOKEN), &[]);

    let supervisor = supervisor_for(zo, &server.addr);
    let mut lane = supervisor
        .open_lane(
            Box::new(LocalPty),
            Some("session-7"),
            Some(PtyCwd::Local(worktree.clone())),
            &[("ZEROCODE_PANE_KEY".to_string(), "tab-1/leaf-2".to_string())],
            10,
            80,
        )
        .expect("open lane");

    assert!(
        pump_until(&mut lane, |lane| {
            lane.terminal().grid().visible_text().contains("pane=")
        }),
        "the lane never reported itself; saw {:?}",
        lane.terminal().grid().visible_text()
    );
    let screen = lane.terminal().grid().visible_text();
    assert!(screen.contains("session-7"), "screen was {screen:?}");
    assert!(
        screen.contains("pane=tab-1/leaf-2"),
        "screen was {screen:?}"
    );
    assert!(screen.contains("wt-refactor"), "screen was {screen:?}");

    assert_eq!(
        marker_lines(&marker),
        vec![format!(
            "attach session-7 --bind {}|{TOKEN}",
            supervisor.bind_addr()
        )],
        "the attach client must receive the address and the secret"
    );
}

/// A lane with no session must run `zo attach` with **no id**. That is the only
/// thing that makes the server create a session; a placeholder id names one
/// instead, and the lane then renders a screen that belongs to nothing — which
/// is how `session.list` stays empty while everything looks fine.
#[test]
fn a_lane_with_no_session_asks_the_server_to_create_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let zo = fake_zo(dir.path(), &marker);
    let server = mock_serve(Some(TOKEN), &[]);

    let supervisor = supervisor_for(zo, &server.addr);
    let mut lane = supervisor
        .open_lane(Box::new(LocalPty), None, None, &[], 10, 40)
        .expect("open lane");
    assert!(pump_until(&mut lane, |lane| {
        !lane.terminal().grid().visible_text().is_empty()
    }));

    assert_eq!(
        marker_lines(&marker),
        vec![format!("attach --bind {}|{TOKEN}", supervisor.bind_addr())],
        "the id must be absent, not invented"
    );
}

/// A local IDE pane is not an attach client. It receives an address-file path
/// and the token explicitly, reports its kernel-selected port there, and the
/// supervisor returns that address without probing or starting `zo serve`.
#[test]
fn opening_a_pane_uses_its_private_events_channel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let addr_file = dir.path().join("pane.addr");
    let worktree = dir.path().join("wt-events");
    fs::create_dir_all(&worktree).expect("worktree");
    let supervisor = supervisor_for(fake_zo(dir.path(), &marker), &free_addr());

    let opened = supervisor
        .open_pane(
            Box::new(LocalPty),
            &worktree,
            Some("session-42"),
            &addr_file,
            &[("ZEROCODE_PANE_KEY".to_string(), "tab-1/leaf-3".to_string())],
            &[
                "--permission-mode".to_string(),
                "danger-full-access".to_string(),
                "--resume".to_string(),
                "stale-session".to_string(),
                "--events-bind=127.0.0.1:9".to_string(),
                "--cwd".to_string(),
                "--model".to_string(),
                "fast".to_string(),
            ],
            10,
            80,
            // A ceiling, not a measurement: the fake zo is a shell script,
            // and under a release gate's load its spawn has taken longer than
            // two seconds (lane a8e6b0d1, 2026-09-17, load 9). The exit test
            // below gives the same ten.
            Duration::from_secs(10),
        )
        .expect("open pane");

    assert_eq!(opened.addr, "127.0.0.1:49152");
    assert!(!addr_file.exists(), "the one-shot address file leaked");
    assert_eq!(
        marker_lines(&marker),
        vec![format!(
            "--events-bind 127.0.0.1:0 --cwd {} --resume session-42 --permission-mode danger-full-access --model fast|{TOKEN}",
            worktree.display()
        )]
    );
}

/// A pane whose process ends before it publishes is a refusal NOW, in the
/// process's own words — not the whole patience of silence and then a timeout
/// with no reason in it. 2026-09-13: a `zo --resume` refused by the session's
/// writer lease exited within a second, the window waited 20 s, and the
/// refusal it logged said only that no channel had appeared.
#[test]
fn a_pane_that_exits_before_publishing_is_refused_at_once_with_its_last_words() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addr_file = dir.path().join("pane.addr");
    let worktree = dir.path().join("wt-refused");
    fs::create_dir_all(&worktree).expect("worktree");
    let supervisor = supervisor_for(refusing_zo(dir.path()), &free_addr());

    let started = Instant::now();
    let error = supervisor
        .open_pane(
            Box::new(LocalPty),
            &worktree,
            Some("session-held"),
            &addr_file,
            &[],
            &[],
            10,
            80,
            Duration::from_secs(10),
        )
        .err()
        .expect("a pane whose process ended cannot be open");
    let waited = started.elapsed();

    match error {
        LaneError::PaneExited { code, said } => {
            assert_eq!(code, 3);
            assert!(
                said.contains("resume failed: fixture lease held by zo pid 1"),
                "the refusal must carry the process's last line: {said:?}"
            );
        }
        other => panic!("expected PaneExited, got {other}"),
    }
    assert!(
        waited < Duration::from_secs(5),
        "waited {waited:?} on a process that had already ended"
    );
    assert!(!addr_file.exists(), "the one-shot address file leaked");
}

/// A `zo` that refuses the way a lease-held `--resume` does: one line on its
/// terminal, a non-zero exit, and no address file.
fn refusing_zo(dir: &Path) -> ZoBinary {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(ZO_EXECUTABLE);
    fs::write(
        &path,
        "#!/bin/sh\nprintf 'resume failed: fixture lease held by zo pid 1\\n' >&2\nexit 3\n",
    )
    .expect("write refusing zo");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    ZoBinary { path }
}

/// The supervisor builds the attach request, but the injected backend is the
/// only object allowed to decide how that request becomes a live terminal.
#[test]
fn spawn_attach_hands_the_exact_remote_request_to_the_selected_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("calls.log");
    let supervisor = supervisor_for(fake_zo(dir.path(), &marker), &free_addr());
    let cwd = RemotePath::parse("/srv/work/zerocode").expect("validated remote cwd");
    let seen = Arc::new(Mutex::new(Vec::new()));

    let failed = supervisor.spawn_attach(
        Box::new(RecordingSpawner {
            seen: Arc::clone(&seen),
        }),
        Some("session-remote"),
        Some(PtyCwd::Remote(cwd.clone())),
        &[
            ("ZEROCODE_PANE_KEY".to_string(), "tab-4/leaf-2".to_string()),
            (TOKEN_ENV.to_string(), "stale-token".to_string()),
        ],
        24,
        96,
    );
    assert!(
        matches!(
            failed,
            Err(LaneError::Pty(PtyTransportError::Host(
                HostError::Cancelled
            )))
        ),
        "the recording backend did not return its sentinel failure"
    );

    let requests = seen.lock().expect("recording lock");
    let [request] = requests.as_slice() else {
        panic!("the selected backend saw {} requests", requests.len());
    };
    assert_eq!(request.program, supervisor.zo_path().to_string_lossy());
    assert_eq!(
        request.args,
        ["attach", "session-remote", "--bind", supervisor.bind_addr()]
    );
    assert_eq!(request.cwd, Some(PtyCwd::Remote(cwd)));
    assert_eq!(
        request.env,
        [
            ("ZEROCODE_PANE_KEY".to_string(), "tab-4/leaf-2".to_string()),
            (TOKEN_ENV.to_string(), TOKEN.to_string()),
        ]
    );
    assert_eq!((request.rows, request.cols), (24, 96));
}

// ---------------------------------------------------------------- continuity

/// The continuity claim, checked against the protocol rather than against our
/// own argument strings: a client subscribes and sees the session's history,
/// **disconnects entirely**, and a second client subscribing afterwards sees
/// the same history. The session lives on the server, so closing a lane is a
/// detach and not a loss.
#[test]
fn a_session_outlives_the_client_that_was_watching_it() {
    let server = mock_serve(Some(TOKEN), &["user: fix the drain gate", "zo: on it"]);

    let first = subscribe_once(&server.addr, "session-7");
    assert_eq!(
        first,
        vec!["user: fix the drain gate", "zo: on it"],
        "the first client must see the session history"
    );

    // The window closes: the connection goes away completely.
    let second = subscribe_once(&server.addr, "session-7");
    assert_eq!(
        second, first,
        "reattaching must find the same session, not a fresh one"
    );
}

/// One subscribe over a throwaway connection, returning the history the server
/// hands back. Deliberately plain `std::net` — this is asserting the server's
/// behaviour, not our client's.
fn subscribe_once(addr: &str, session_id: &str) -> Vec<String> {
    let stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let mut writer = &stream;
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "session.subscribe",
        "params": { "id": session_id, "boundary": true },
        "token": TOKEN,
    });
    let mut line = request.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes()).expect("write");
    writer.flush().expect("flush");

    let mut reply = String::new();
    BufReader::new(&stream)
        .read_line(&mut reply)
        .expect("read reply");
    let value: serde_json::Value = serde_json::from_str(reply.trim()).expect("json");
    value["result"]["history"]
        .as_array()
        .expect("history")
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect()
}
