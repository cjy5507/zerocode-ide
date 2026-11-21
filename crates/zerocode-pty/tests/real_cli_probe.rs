//! One real CLI started the way a summons starts it, watched in a real pty
//! (t-124593) — a measurement, run by hand on the build line, never by a gate.
//!
//! What it answers for Devin, the questions the fixes of t-124593 stand on:
//! does its first-run trust menu stay away when the launch says so; when does
//! its composer stand against the moment the window's readiness for it
//! (`ReadyMark::Quiet`: bracketed paste on, then [`zerocode_pty::ready::QUIET`]
//! of silence) says it may type; is a one-line prompt pasted and entered at
//! THAT moment taken, with the Enter pressed once more as the window does;
//! and which hook events reach the window, carrying what. The hooks go
//! to a listener of this test's own: every `ZEROCODE_*` and `CLAUDE*` value of
//! the pane that runs the test is taken out of the child's environment, and
//! the three a hook script needs point here — so nothing the CLI says reaches
//! the window or the person's panes.
//!
//! It does nothing unless `ZEROCODE_REAL_CLI_PROBE_ARGV` names the command
//! (the binary the catalog resolved, then its launch words, space-separated)
//! and `ZEROCODE_REAL_CLI_PROBE_OUT` an existing folder for what it saw. A
//! prompt is pasted only when `ZEROCODE_REAL_CLI_PROBE_PROMPT` gives one: one
//! model call, at most once a run.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use zerocode_pty::PtyLane;
use zerocode_pty::ready::QUIET;

/// How long the start is watched for its composer before the probe gives up.
const START_PATIENCE: Duration = Duration::from_secs(30);
/// How long a taken prompt is given to end its turn.
const TURN_PATIENCE: Duration = Duration::from_secs(120);
/// How long after its Enter a prompt the CLI has not reported taking gets
/// the Enter once more — the window's own second press, in spirit.
const ENTER_AGAIN_AFTER: Duration = Duration::from_secs(4);
/// How long a drawn composer is given to fall quiet before the prompt goes in
/// anyway — so a start that never settles still spends its one model call on
/// a question the facts can answer, and says it did.
const QUIET_FALLBACK: Duration = Duration::from_secs(10);
/// The moments of the start whose screens are kept, from the spawn.
const START_SHOTS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// A moment as the facts keep it: whole milliseconds.
fn ms(moment: Duration) -> u64 {
    u64::try_from(moment.as_millis()).unwrap_or(u64::MAX)
}

/// One hook post, as this test's listener heard it.
struct Heard {
    at: Duration,
    event: String,
    payload: serde_json::Value,
}

/// `application/x-www-form-urlencoded`, decoded: `+` is a space, `%XY` a byte.
fn form_decoded(text: &str) -> BTreeMap<String, String> {
    fn unescape(raw: &str) -> String {
        let bytes = raw.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'+' => out.push(b' '),
                b'%' if at + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[at + 1..at + 3])
                        .ok()
                        .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                    match hex {
                        Some(byte) => {
                            out.push(byte);
                            at += 2;
                        }
                        None => out.push(b'%'),
                    }
                }
                byte => out.push(byte),
            }
            at += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    text.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (unescape(name), unescape(value)))
        .collect()
}

/// A listener on a loopback port of its own, answering every post `200` and
/// handing each one over as it arrives.
fn listen(began: Instant) -> (u16, Receiver<Heard>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("its address").port();
    let (said, heard) = channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let said = said.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let mut stream = stream;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                let form = form_decoded(&String::from_utf8_lossy(&body));
                let payload = form
                    .get("payload")
                    .and_then(|text| serde_json::from_str(text).ok())
                    .unwrap_or(serde_json::Value::Null);
                let event = form
                    .get("hook_event_name")
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .or_else(|| payload["hook_event_name"].as_str().map(str::to_string))
                    .unwrap_or_default();
                let _ = said.send(Heard {
                    at: began.elapsed(),
                    event,
                    payload,
                });
            });
        }
    });
    (port, heard)
}

/// The probe's own folder names and the person's home, out of what is kept.
fn masked(text: &str, cwd: &Path) -> String {
    let mut text = text.replace(&cwd.display().to_string(), "<probe-dir>");
    if let Some(home) = std::env::var_os("HOME") {
        text = text.replace(&home.to_string_lossy().into_owned(), "~");
    }
    text
}

/// The shape of a JSON value: its keys and the kinds of their values, so a
/// payload's fields are kept without its words.
fn shape(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.clone(), shape(value)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        serde_json::Value::Array(items) => {
            serde_json::json!(items.first().map(shape).into_iter().collect::<Vec<_>>())
        }
        serde_json::Value::String(_) => "string".into(),
        serde_json::Value::Number(_) => "number".into(),
        serde_json::Value::Bool(_) => "bool".into(),
        serde_json::Value::Null => "null".into(),
    }
}

#[test]
#[ignore = "starts a real CLI of this machine; run by hand with its variables"]
fn a_real_cli_starts_takes_a_line_and_reports_through_its_hooks() {
    let (Ok(argv), Ok(out)) = (
        std::env::var("ZEROCODE_REAL_CLI_PROBE_ARGV"),
        std::env::var("ZEROCODE_REAL_CLI_PROBE_OUT"),
    ) else {
        return;
    };
    let out = Path::new(&out);
    assert!(out.is_dir(), "{} is not a folder", out.display());
    let prompt = std::env::var("ZEROCODE_REAL_CLI_PROBE_PROMPT")
        .ok()
        .filter(|line| !line.trim().is_empty() && !line.contains('\n'));
    let words: Vec<String> = argv.split_whitespace().map(str::to_string).collect();
    let (program, args) = words.split_first().expect("a program to start");

    // A fresh checkout nobody has trusted, as a summons' worktree is.
    let dir = tempfile::tempdir().expect("a scratch checkout");
    let cwd = dir.path().canonicalize().expect("its real path");
    let git = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&cwd)
        .status()
        .expect("git");
    assert!(git.success(), "git init failed");

    let began = Instant::now();
    let (port, heard) = listen(began);
    // An empty value takes the name out of the child's environment.
    let mut env: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| name.starts_with("ZEROCODE_") || name.starts_with("CLAUDE"))
        .map(|name| (name, String::new()))
        .collect();
    env.extend([
        ("ZEROCODE_HOOK_PORT".to_string(), port.to_string()),
        ("ZEROCODE_HOOK_TOKEN".to_string(), "probe-token".to_string()),
        ("ZEROCODE_PANE_KEY".to_string(), "probe-pane".to_string()),
    ]);
    let mut lane =
        PtyLane::spawn(program, args, Some(&cwd), &env, 40, 120).expect("the CLI starts");

    let mut facts = serde_json::Map::new();
    let mut screens: Vec<(String, String)> = Vec::new();
    let mut events: Vec<Heard> = Vec::new();
    let mut shots = START_SHOTS.iter().peekable();
    let first = |facts: &mut serde_json::Map<String, serde_json::Value>, name: &str| {
        facts
            .entry(name.to_string())
            .or_insert_with(|| serde_json::json!(ms(began.elapsed())));
    };
    // The moment the window's readiness for a `Quiet` row says it may type,
    // and whether the composer was drawn by then.
    let mut ready_at = None;
    let mut composer_at = None;
    let mut pasted = false;
    while began.elapsed() < START_PATIENCE {
        lane.pump();
        events.extend(heard.try_iter());
        let screen = lane.terminal().grid().visible_text();
        let handshake = lane.terminal().grid().bracketed_paste();
        if handshake {
            first(&mut facts, "bracketedPasteOnMs");
        }
        if screen.contains("Do you trust") {
            first(&mut facts, "trustMenuMs");
        }
        if screen.contains("Ask Devin") && !screen.contains("Do you trust") {
            first(&mut facts, "composerMs");
            composer_at.get_or_insert_with(Instant::now);
        }
        if handshake
            && ready_at.is_none()
            && lane
                .last_output_at()
                .is_some_and(|at| at.elapsed() >= QUIET)
        {
            first(&mut facts, "quietReadyMs");
            facts.insert(
                "composerDrawnAtQuietReady".into(),
                serde_json::json!(composer_at.is_some()),
            );
            screens.push(("at-quiet-ready".into(), screen.clone()));
            ready_at = Some(Instant::now());
        }
        // The prompt goes in at the window's own moment — or, when that
        // never comes, once the composer has stood a while, and says so.
        let fallback = prompt.is_some()
            && !pasted
            && ready_at.is_none()
            && composer_at.is_some_and(|at: Instant| at.elapsed() >= QUIET_FALLBACK);
        if fallback {
            facts.insert("pastedWithoutQuietReady".into(), serde_json::json!(true));
        }
        if let (Some(line), true, false) =
            (prompt.as_deref(), ready_at.is_some() || fallback, pasted)
        {
            let words = format!("\x1b[200~{line}\x1b[201~");
            lane.write_input(words.as_bytes()).expect("the paste");
            std::thread::sleep(Duration::from_millis(150));
            lane.write_input(b"\r").expect("the Enter");
            first(&mut facts, "enteredMs");
            pasted = true;
            break;
        }
        while let Some(shot) = shots.next_if(|shot| began.elapsed() >= **shot) {
            screens.push((format!("start-{}s", shot.as_secs()), screen.clone()));
        }
        if prompt.is_none() && ready_at.is_some() && shots.peek().is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if pasted {
        let entered = Instant::now();
        let mut pressed_again = false;
        while entered.elapsed() < TURN_PATIENCE {
            lane.pump();
            events.extend(heard.try_iter());
            let taken = events.iter().any(|one| one.event == "UserPromptSubmit");
            if !taken && !pressed_again && entered.elapsed() >= ENTER_AGAIN_AFTER {
                lane.write_input(b"\r").expect("the Enter again");
                first(&mut facts, "enteredAgainMs");
                pressed_again = true;
            }
            if events.iter().any(|one| one.event == "Stop") {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        lane.pump();
        screens.push(("after-turn".into(), lane.terminal().grid().visible_text()));
    }

    // Out the way a person leaves it, then the lane's own end.
    for _ in 0..2 {
        let _ = lane.write_input(b"\x03");
        std::thread::sleep(Duration::from_millis(600));
        lane.pump();
    }
    events.extend(heard.try_iter());
    screens.push(("at-exit".into(), lane.terminal().grid().visible_text()));
    let _ = lane.kill();

    facts.insert("argv".into(), serde_json::json!(masked(&argv, &cwd)));
    facts.insert("promptPasted".into(), serde_json::json!(pasted));
    let transcript = events
        .iter()
        .find_map(|one| one.payload["transcript_path"].as_str().map(str::to_string));
    facts.insert(
        "events".into(),
        serde_json::json!(
            events
                .iter()
                .map(|one| serde_json::json!({
                    "atMs": ms(one.at),
                    "event": one.event,
                    // The few short values a reader of this probe needs, by
                    // value; the rest by shape only.
                    "payloadEvent": one.payload["hook_event_name"],
                    "sessionId": one.payload["session_id"],
                    "transcriptPath": one.payload["transcript_path"]
                        .as_str()
                        .map(|path| masked(path, &cwd)),
                    "toolName": one.payload["tool_name"],
                    "payloadShape": shape(&one.payload),
                }))
                .collect::<Vec<_>>()
        ),
    );
    facts.insert(
        "transcriptPath".into(),
        serde_json::json!(transcript.as_deref().map(|path| masked(path, &cwd))),
    );
    std::fs::write(
        out.join("probe-facts.json"),
        serde_json::to_string_pretty(&serde_json::Value::Object(facts)).expect("json"),
    )
    .expect("the facts are written");
    for (name, screen) in &screens {
        std::fs::write(
            out.join(format!("probe-screen-{name}.txt")),
            masked(screen, &cwd),
        )
        .expect("a screen is written");
    }
    // The probe's own conversation, for the shape of the store the window
    // reads a pane's conversation from — never anybody else's.
    if let Some(path) = transcript.filter(|path| Path::new(path).is_file()) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(out.join("probe-transcript.txt"), masked(&text, &cwd))
            .expect("the transcript is written");
    }
}
