//! The realtime bench's stand-in for the window (t-6767, pre-check (iii)):
//! the installed helper, launched through this build's own session
//! (`macos::Session`, LaunchServices), driven down the window's own reflex
//! road — the door and the start ([`super::start`]) and the receipt watch
//! ([`super::Watch`] into a [`super::FileSink`], the reflex decision off) —
//! on a fixture window of the bench's own. The door's facts are built here:
//! nothing reads or writes the person's `computer_live_reflex`, their ledger
//! or their Jev seat.
//!
//! Not part of the gate. `tools/computer-bench/fixture_reflex.py` runs this
//! test by the absolute path of its binary with the run's folder named in
//! `ZEROCODE_REFLEX_BENCH_DIR` and the installed helper in
//! `ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH`, and the two speak through that
//! folder: the runner's `request.json`, then this test's `geometry.json` (the
//! display and the fixture's window as the window server names them), the
//! runner's `plan.json`, then `flow.md`, `started.json`, `status.jsonl`,
//! `receipts.jsonl` and `ended.json`. The one word the runner ever writes to
//! stdin is `stop`.
#![cfg(target_os = "macos")]

use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::computer_flow::{
    EvidenceLevel, FLOW_FINGERPRINT_APPS, FLOW_FINGERPRINT_PROTOCOL, FLOW_HEADING,
    FLOW_HEADING_CHECKS, FLOW_KEY_EVIDENCE, FLOW_KEY_FINGERPRINT, FLOW_KEY_POLICY, Policy,
};
use zerocode_core::computer_recipe::RECIPE_HEADING_STEPS;
use zerocode_core::computer_use::{COMPUTER_USE_PROTOCOL_VERSION, REFLEX_COLLECT_MS};
use zerocode_core::computer_use_protocol::reflex::{ReflexPlan, plan_hash};
use zerocode_core::jev::JevMode;
use zerocode_core::jev::reflex_decide::Wired;

use super::{Asker, DoorFacts, FileSink, Watch, start};
use crate::computer_use::ComputerUseError;
use crate::computer_use::macos::Session;
use crate::computer_use::session::ProviderSession as _;
use crate::systemone::Spent;

/// The environment variable that names the run's folder.
const FOLDER: &str = "ZEROCODE_REFLEX_BENCH_DIR";
/// The word that stops the run.
const STOP: &str = "stop";

/// The host's uptime clock — the clock the helper's receipts, the fixture's
/// frames and events and the runner's goal are all on.
fn uptime_ns() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` outlives the call, and `clock_gettime` writes one timespec
    // into it and nothing else.
    let read = unsafe { libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &raw mut now) };
    assert_eq!(read, 0, "the uptime clock reads");
    u64::try_from(now.tv_sec).unwrap_or_default() * 1_000_000_000
        + u64::try_from(now.tv_nsec).unwrap_or_default()
}

fn read(path: &Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Written whole, then renamed into place: the runner never reads half a file.
fn write(path: &Path, value: &Value) {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, format!("{value}\n")).expect("the run's folder takes a file");
    std::fs::rename(&partial, path).expect("the run's folder takes a file");
}

fn append(path: &Path, value: &Value) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the run's folder takes a file");
    writeln!(file, "{value}").expect("the run's folder takes a line");
}

/// The Flow document the door reads: the fixture as its one app, a dry
/// policy, and the plan's reflex sections, hashed here by the core.
fn flow(plan: &ReflexPlan, bundle: &str) -> String {
    format!(
        "# reflex bench round\n\n{RECIPE_HEADING_STEPS}\n\n1. `zerocode-computer launch --app {bundle}`\n\n\
         {FLOW_HEADING}\n\n\
         - {FLOW_KEY_POLICY}: {}\n\
         - {FLOW_KEY_EVIDENCE}: {}\n\
         - {FLOW_KEY_FINGERPRINT}: {FLOW_FINGERPRINT_APPS}={bundle} {FLOW_FINGERPRINT_PROTOCOL}={COMPUTER_USE_PROTOCOL_VERSION}\n\n\
         {FLOW_HEADING_CHECKS}\n\n\
         1. `zerocode-computer wait-for --app {bundle} --text round` — event, required\n{}",
        Policy::Dry.as_str(),
        EvidenceLevel::Full.as_str(),
        plan.written_sections()
    )
}

/// The display a window's middle sits on, from `displays`, with its index.
fn display_of(displays: &Value, window: &Value) -> Value {
    let middle = |at: &str, span: &str| {
        window[at].as_f64().unwrap_or_default() + window[span].as_f64().unwrap_or_default() / 2.0
    };
    let (x, y) = (middle("x", "width"), middle("y", "height"));
    displays["displays"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|display| {
            let bounds = &display["bounds"];
            let (left, top) = (bounds["x"].as_f64().unwrap_or_default(), bounds["y"].as_f64().unwrap_or_default());
            (left..left + bounds["width"].as_f64().unwrap_or_default()).contains(&x)
                && (top..top + bounds["height"].as_f64().unwrap_or_default()).contains(&y)
        })
        .map(|display| {
            let bounds = &display["bounds"];
            json!({
                "index": display["index"], "scale": display["scale"],
                "x": bounds["x"], "y": bounds["y"], "width": bounds["width"], "height": bounds["height"],
            })
        })
        .expect("the fixture's window sits on a display")
}

#[test]
#[ignore = "drives the installed helper's hand on the bench's own fixture window: real pointer input; run only by tools/computer-bench/fixture_reflex.py"]
fn a_reflex_run_on_the_benchs_own_fixture() {
    let folder = PathBuf::from(std::env::var(FOLDER).expect("the run's folder"));
    let request = read(&folder.join("request.json"));
    let bundle = request["bundle"]
        .as_str()
        .expect("the fixture's bundle")
        .to_string();
    let poll = Duration::from_millis(request["pollMs"].as_u64().expect("the runner's poll"));
    let stopped = Arc::new(AtomicBool::new(false));
    {
        let stopped = Arc::clone(&stopped);
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                if line.trim() == STOP {
                    stopped.store(true, Ordering::SeqCst);
                }
            }
        });
    }

    let launched_ns = uptime_ns();
    let mut session = Session::start().expect("the installed helper answers this build");
    let connected_ns = uptime_ns();
    let helper_pid = session.helper_pid();
    let mut call = |method: &str, params: Value| {
        session
            .request(method, params)
            .map_err(|failure| failure.into_error())
    };
    let displays = call("displays", json!({})).expect("the displays");
    let windows = call("listWindows", json!({ "app": bundle })).expect("the fixture's windows");
    let window = windows["windows"]
        .as_array()
        .and_then(|windows| windows.first())
        .cloned()
        .expect("the fixture shows a window");
    let display = display_of(&displays, &window);
    write(
        &folder.join("geometry.json"),
        &json!({
            "display": display,
            "window": { "id": window["id"], "x": window["x"], "y": window["y"],
                        "width": window["width"], "height": window["height"] },
            "helperPid": helper_pid,
            "launchedNs": launched_ns,
            "connectedNs": connected_ns,
        }),
    );

    // The runner writes the plan off that geometry; a stop before it starts nothing.
    let plan_path = folder.join("plan.json");
    while !plan_path.exists() {
        if stopped.load(Ordering::SeqCst) {
            write(
                &folder.join("ended.json"),
                &json!({ "stoppedBy": STOP, "startedNothing": true }),
            );
            return;
        }
        std::thread::sleep(poll);
    }
    let mut plan: ReflexPlan =
        serde_json::from_value(read(&plan_path)).expect("a plan in the contract's words");
    plan.plan_hash = plan_hash(&plan);
    let flow_path = folder.join("flow.md");
    std::fs::write(&flow_path, flow(&plan, &bundle)).expect("the run's folder takes the Flow");

    // The door as the window builds it, with the bench's own "on": the
    // person's setting is never read.
    let facts = DoorFacts::now(true);
    let words = json!({
        "flow": flow_path,
        "seconds": request["seconds"],
        "renew": request["renew"],
        "display": display["index"],
    });
    let request_ns = uptime_ns();
    let started = start(
        &words,
        |path| std::fs::read_to_string(path),
        &facts,
        &mut call,
    );
    let answered_ns = uptime_ns();
    let (answer, admitted) = match started {
        Ok(started) => started,
        Err(refusal) => {
            write(
                &folder.join("ended.json"),
                &json!({ "refused": { "code": refusal.code, "message": refusal.message }, "requestNs": request_ns }),
            );
            panic!("the helper refused the run: {refusal:?}");
        }
    };
    let run = answer["runId"].as_str().expect("the run's id").to_string();
    let deadline_ns = answer["deadlineNs"].as_u64().expect("the run's deadline");
    write(
        &folder.join("started.json"),
        &json!({
            "runId": run,
            "requestNs": request_ns,
            "answeredNs": answered_ns,
            "acceptedNs": deadline_ns - admitted.policy.run_ns,
            "deadlineNs": deadline_ns,
            "planHash": plan.plan_hash,
            "answer": answer,
        }),
    );

    // The window's watch, every REFLEX_COLLECT_MS; the run's status every poll.
    let receipts = folder.join("receipts.jsonl");
    let mut watch = Watch::new(&run, Some(receipts.clone()));
    let mut sink = FileSink(Some(receipts));
    let ask: Asker = Arc::new(|_state: Value| -> (Wired, Spent) {
        unreachable!("the bench's reflex decision is off: it asks nothing")
    });
    let status_path = folder.join("status.jsonl");
    let collect = Duration::from_millis(REFLEX_COLLECT_MS);
    let mut last_pass: Option<std::time::Instant> = None;
    let mut stop = None;
    loop {
        if stop.is_none() && stopped.load(Ordering::SeqCst) {
            let asked_ns = uptime_ns();
            let answer = call("reflexStop", json!({ "run": run }));
            stop = Some(json!({
                "askedNs": asked_ns,
                "answeredNs": uptime_ns(),
                "answer": answer.as_ref().ok(),
                "refused": answer.as_ref().err().map(|refusal| refusal.message.clone()),
            }));
        }
        if let Ok(status) = call("reflexStatus", json!({ "run": run })) {
            append(
                &status_path,
                &json!({ "ns": uptime_ns(), "status": status }),
            );
        }
        if last_pass.is_none_or(|at| at.elapsed() >= collect) {
            last_pass = Some(std::time::Instant::now());
            if watch.pass(&mut call, &mut sink, || JevMode::Off, &ask, &mut |_rows| {}) {
                break;
            }
        }
        std::thread::sleep(poll);
    }
    let ended_ns = uptime_ns();
    let status = call("reflexStatus", json!({ "run": run })).unwrap_or(Value::Null);
    // The pointer goes back where it rested at the goal: a point on the
    // fixture's own window, so this input lands there too.
    let restored = request["restore"].as_object().map(|point| {
        call("mouseMove", json!({ "x": point["x"], "y": point["y"] })).map_or_else(
            |refusal: ComputerUseError| json!({ "refused": refusal.message }),
            |_| json!(true),
        )
    });
    write(
        &folder.join("ended.json"),
        &json!({
            "report": watch.report().rendered(),
            "status": status,
            "endedNs": ended_ns,
            "stop": stop,
            "stoppedBy": stopped.load(Ordering::SeqCst).then_some(STOP),
            "restored": restored,
        }),
    );
}
