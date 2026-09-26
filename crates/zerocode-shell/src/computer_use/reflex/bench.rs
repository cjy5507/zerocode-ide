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
//!
//! A request that names a goal takes the autopilot's road instead (t-10223
//! R9): the goal and the fixture's app go to [`Autopilot::start`] as
//! `reflex-auto` hands them over, and its collects run here. Its plans come
//! from the generator the request names — the window's writer, its key from
//! this process's environment, or [`Stub`], which answers with the runner's
//! `plan.json` and says so on every row — and its questions go down a wire
//! whose key is the environment's and whose settings and ledgers are the
//! bench's own zo home (`ZO_CONFIG_HOME`, the request's `home`): nothing is
//! read from the keychain and nothing is written where the person's zo
//! keeps its own. Each run keeps its receipts in `reflex-<run>.jsonl`, every
//! start and stop the helper answered is a line of `calls.jsonl`, and
//! `ended.json` carries each run's report and the autopilot's account.
#![cfg(target_os = "macos")]

use std::cell::RefCell;
use std::collections::BTreeMap;
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
use zerocode_core::computer_use::{
    COMPUTER_USE_PROTOCOL_VERSION, REFLEX_COLLECT_MS, REFLEX_PLAN_LEDGER,
};
use zerocode_core::computer_use_protocol::reflex::{ReflexPlan, plan_hash};
use zerocode_core::jev::reflex_decide::Wired;
use zerocode_core::jev::{JevMode, REFLEX_DECIDE, Run as Asking};
use zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX;

use super::autopilot::{self, Asked, Autopilot, Keeping, World};
use super::plan::Generator;
use super::{
    Asker, Call, DoorFacts, FileSink, ReceiptSink, Watch, receipts_file, start, steady_ms,
};
use crate::api_routers::{RouterKeys, RouterRefusal};
use crate::computer_use::ComputerUseError;
use crate::computer_use::errand::value::{LiveWriter, Said, ValueWriter as _, endpoint_of};
use crate::computer_use::macos::Session;
use crate::computer_use::session::ProviderSession as _;
use crate::systemone::{self, Spent, Wire};

/// The environment variable that names the run's folder.
const FOLDER: &str = "ZEROCODE_REFLEX_BENCH_DIR";
/// The word that stops the run.
const STOP: &str = "stop";
/// The generator a request names that stands in for a model — and the word
/// every plan it answers with is counted under.
const STUB: &str = "stub";
/// The file every helper start and stop is a line of.
const CALLS: &str = "calls.jsonl";
/// The helper's word for a run it no longer knows.
const MISSING: &str = "missing";

/// The keys this process's environment holds, by the name a key store's
/// item carries after the window's prefix: the bench asks with the key the
/// runner put there and never opens the keychain itself.
struct EnvKeys;

impl RouterKeys for EnvKeys {
    fn read(&self, service: &str) -> Result<Option<String>, RouterRefusal> {
        Ok(service
            .strip_prefix(SERVICE_KEYCHAIN_SERVICE_PREFIX)
            .and_then(|name| std::env::var(name).ok()))
    }

    fn write(&self, _service: &str, _secret: &str) -> Result<(), RouterRefusal> {
        Err(RouterRefusal::from("the bench keeps no key".to_string()))
    }

    fn delete(&self, _service: &str) -> Result<(), RouterRefusal> {
        Err(RouterRefusal::from("the bench keeps no key".to_string()))
    }
}

/// A generator standing in for a model: every request is answered with the
/// plan the runner wrote off the window server's geometry (`plan.json`),
/// whole, once it is there — and every plan it answers is `stub`'s, never a
/// model's.
struct Stub {
    plan: PathBuf,
    poll: Duration,
}

impl Generator for Stub {
    fn unready(&self) -> Option<String> {
        None
    }

    fn model(&self) -> Option<String> {
        None
    }

    fn ask(&mut self, system: &str, user: &str, left: Duration) -> Result<Said, String> {
        let asked = std::time::Instant::now();
        while !self.plan.exists() {
            if asked.elapsed() >= left {
                return Err(systemone::TIMEOUT.to_string());
            }
            std::thread::sleep(self.poll);
        }
        let text = std::fs::read_to_string(&self.plan).map_err(|error| error.to_string())?;
        Ok(Said {
            bytes_out: system.len() + user.len(),
            bytes_in: text.len(),
            tokens: None,
            text,
            // The stand-in is no road the person chose: its answers say so.
            answered: crate::computer_use::errand::value::Answered {
                road: STUB,
                ..Default::default()
            },
        })
    }

    fn source(&self) -> &'static str {
        STUB
    }
}

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
    let calls = folder.join(CALLS);
    let known = RefCell::new(BTreeMap::new());
    // Every start and stop the helper answered, as the host's clock saw it
    // asked and answered: where one plan's hand let go and the next took over.
    // And each run's status as the helper last told it: once a run's receipts
    // are all acknowledged the helper forgets it, but for the last to end.
    let mut call = |method: &str, params: Value| {
        let asked_ns = uptime_ns();
        let answer = session
            .request(method, params.clone())
            .map_err(|failure| failure.into_error());
        if let (Some(run), Ok(told)) = (params.get("run").and_then(Value::as_str), answer.as_ref())
            && told
                .get("state")
                .and_then(Value::as_str)
                .is_some_and(|state| state != MISSING)
        {
            let mut status = told.clone();
            if let Some(fields) = status.as_object_mut() {
                fields.remove("receipts");
            }
            known.borrow_mut().insert(run.to_string(), status);
        }
        if matches!(method, "reflexStart" | "reflexStop") {
            let answered = answer.as_ref().ok();
            append(
                &calls,
                &json!({
                    "method": method,
                    "run": params.get("run").or_else(|| answered.and_then(|answer| answer.get("runId"))),
                    "askedNs": asked_ns,
                    "answeredNs": uptime_ns(),
                    "deadlineNs": answered.and_then(|answer| answer.get("deadlineNs")),
                    "refused": answer.as_ref().err().map(|refusal| refusal.message.clone()),
                }),
            );
        }
        answer
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
    let bench = Bench {
        known: &known,
        folder: &folder,
        request: &request,
        bundle: &bundle,
        display: &display,
        poll,
        stopped: &stopped,
    };
    if request.get("goal").is_some_and(Value::is_string) {
        bench.autopilot(&mut call);
    } else {
        bench.hand(&mut call);
    }
}

/// One run's folder and what the runner asked of it.
struct Bench<'a> {
    /// Each run's status as the helper last told it.
    known: &'a RefCell<BTreeMap<String, Value>>,
    folder: &'a Path,
    request: &'a Value,
    bundle: &'a str,
    display: &'a Value,
    poll: Duration,
    stopped: &'a AtomicBool,
}

impl Bench<'_> {
    /// The run's end, as the runner reads it; the pointer back where it
    /// rested at the goal — a point on the fixture's own window, so this
    /// input lands there too.
    fn end(&self, call: Call<'_>, mut ended: Value, stop: Option<Value>) {
        let restored = self.request["restore"].as_object().map(|point| {
            call("mouseMove", json!({ "x": point["x"], "y": point["y"] })).map_or_else(
                |refusal: ComputerUseError| json!({ "refused": refusal.message }),
                |_| json!(true),
            )
        });
        ended["stop"] = json!(stop);
        ended["stoppedBy"] = json!(self.stopped.load(Ordering::SeqCst).then_some(STOP));
        ended["restored"] = json!(restored);
        write(&self.folder.join("ended.json"), &ended);
    }

    /// A person's plan: the runner's `plan.json` through the window's door
    /// and start, and the window's watch with the reflex decision off.
    fn hand(&self, call: Call<'_>) {
        let folder = self.folder;
        // The runner writes the plan off that geometry; a stop before it starts nothing.
        let plan_path = folder.join("plan.json");
        while !plan_path.exists() {
            if self.stopped.load(Ordering::SeqCst) {
                write(
                    &folder.join("ended.json"),
                    &json!({ "stoppedBy": STOP, "startedNothing": true }),
                );
                return;
            }
            std::thread::sleep(self.poll);
        }
        let mut plan: ReflexPlan =
            serde_json::from_value(read(&plan_path)).expect("a plan in the contract's words");
        plan.plan_hash = plan_hash(&plan);
        let flow_path = folder.join("flow.md");
        std::fs::write(&flow_path, flow(&plan, self.bundle))
            .expect("the run's folder takes the Flow");

        // The door as the window builds it, with the bench's own "on": the
        // person's setting is never read.
        let facts = DoorFacts::now(true);
        let words = json!({
            "flow": flow_path,
            "seconds": self.request["seconds"],
            "renew": self.request["renew"],
            "display": self.display["index"],
        });
        let request_ns = uptime_ns();
        let started = start(
            &words,
            |path| std::fs::read_to_string(path),
            &facts,
            &mut *call,
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
            if stop.is_none() && self.stopped.load(Ordering::SeqCst) {
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
                if watch.pass(
                    &mut *call,
                    &mut sink,
                    || JevMode::Off,
                    &ask,
                    &mut |_rows| {},
                ) {
                    break;
                }
            }
            std::thread::sleep(self.poll);
        }
        let ended_ns = uptime_ns();
        let status = call("reflexStatus", json!({ "run": run })).unwrap_or(Value::Null);
        self.end(
            call,
            json!({ "report": watch.report().rendered(), "status": status, "endedNs": ended_ns }),
            stop,
        );
    }

    /// A goal: the autopilot writes its plans with the generator the request
    /// names, carries its reflex decision out as the request forces it, and
    /// keeps every ledger in the bench's own zo home.
    fn autopilot(&self, call: Call<'_>) {
        let folder = self.folder;
        let home = PathBuf::from(
            self.request["home"]
                .as_str()
                .expect("the bench's own zo home"),
        );
        let wire = Wire::new(&EnvKeys);
        assert_eq!(
            wire.config_home(),
            Some(home.as_path()),
            "the bench's ledgers live in its own zo home, never the person's: ZO_CONFIG_HOME names request.home"
        );
        let decisions = systemone::ledger_of(&wire, &REFLEX_DECIDE);
        let plans = decisions
            .as_deref()
            .and_then(Path::parent)
            .map(|dir| dir.join(REFLEX_PLAN_LEDGER));
        let ask = super::asker(wire.clone(), Some(folder.to_path_buf()));
        let mut generator: Box<dyn Generator> = if self.request["generator"] == json!(STUB) {
            Box::new(Stub {
                plan: folder.join("plan.json"),
                poll: self.poll,
            })
        } else {
            // The bench's key road, as before t-10372: the row the API-key
            // road asks, its key read off the runner's environment.
            let writer = LiveWriter::at(
                &LiveWriter::window(crate::computer_use::errand::value::Setup::new(
                    zerocode_core::type_value::GeneratorRoad::ApiKey,
                    PathBuf::new(),
                    PathBuf::new(),
                ))
                .row()
                .and_then(endpoint_of)
                .unwrap_or_default(),
                Box::new(EnvKeys),
            );
            Box::new(writer)
        };
        let asked = Asked::of(&json!({
            "goal": self.request["goal"],
            "app": self.bundle,
            "display": self.display["index"],
            "seconds": self.request["seconds"],
            "renew": self.request["renew"],
            "l1": self.request["l1"],
        }));
        let (mode_wire, standing_wire) = (wire.clone(), wire.clone());
        let mut mode = move || REFLEX_DECIDE.mode_in_run(&mode_wire.settings_root(), Asking::Fresh);
        let mut standing =
            move || systemone::standing_in(&standing_wire, &REFLEX_DECIDE, Asking::Fresh);
        let mut record = |rows: Vec<Value>| super::record_decisions(decisions.as_deref(), rows);
        let mut write_plans = |rows: Vec<Value>| {
            if let Some(ledger) = &plans {
                systemone::append_rows(ledger, &rows);
            }
        };
        let mut keeper = |run: &str| -> Keeping {
            let receipts = receipts_file(folder, run);
            (
                Some(receipts.clone()),
                Box::new(FileSink(Some(receipts))) as Box<dyn ReceiptSink + Send>,
            )
        };
        macro_rules! world {
            () => {
                World {
                    call: &mut *call,
                    ask: &ask,
                    mode: &mut mode,
                    standing: &mut standing,
                    generator: generator.as_mut(),
                    decisions: &mut record,
                    plans: &mut write_plans,
                    keeper: &mut keeper,
                    now_ms: steady_ms(),
                    wall_ms: crate::project_runtime::now_epoch_ms(),
                    stopped: None,
                }
            };
        }

        let request_ns = uptime_ns();
        let started = Autopilot::start(
            asked,
            DoorFacts::now(true),
            Some(folder.to_path_buf()),
            &mut world!(),
        );
        let answered_ns = uptime_ns();
        let (mut pilot, answer) = match started {
            Ok(started) => started,
            Err(refusal) => {
                write(
                    &folder.join("ended.json"),
                    &json!({ "refused": { "code": refusal.code, "message": refusal.message }, "requestNs": request_ns }),
                );
                panic!("the autopilot did not start: {refusal:?}");
            }
        };
        let first = answer["runId"].as_str().expect("the run's id").to_string();
        let deadline_ns = std::fs::read_to_string(folder.join(CALLS))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|row| row["method"] == json!("reflexStart") && row["run"] == json!(first))
            .and_then(|row| row["deadlineNs"].as_u64())
            .expect("the first run's deadline");
        let run_ns = answer["runNs"].as_u64().expect("the run's length");
        write(
            &folder.join("started.json"),
            &json!({
                "runId": first,
                "requestNs": request_ns,
                "answeredNs": answered_ns,
                "acceptedNs": deadline_ns - run_ns,
                "deadlineNs": deadline_ns,
                "planHash": answer["planHash"],
                "answer": answer,
            }),
        );

        // The autopilot's collects, every REFLEX_COLLECT_MS; the status of
        // the run it stands on every poll.
        let status_path = folder.join("status.jsonl");
        let collect = Duration::from_millis(REFLEX_COLLECT_MS);
        let mut last_pass: Option<std::time::Instant> = None;
        let mut stop = None;
        loop {
            if stop.is_none() && self.stopped.load(Ordering::SeqCst) {
                stop = Some(json!({
                    "askedNs": uptime_ns(),
                    "current": autopilot::stop_owner(&first).flatten(),
                }));
            }
            let current = pilot.rendered()["current"]["run"].clone();
            if let Some(run) = current.as_str()
                && let Ok(status) = call("reflexStatus", json!({ "run": run }))
            {
                append(
                    &status_path,
                    &json!({ "ns": uptime_ns(), "run": run, "status": status }),
                );
            }
            if last_pass.is_none_or(|at| at.elapsed() >= collect) {
                last_pass = Some(std::time::Instant::now());
                if pilot.tick(&mut world!()) {
                    break;
                }
            }
            std::thread::sleep(self.poll);
        }
        let ended_ns = uptime_ns();
        let account = pilot.rendered();
        let runs: Vec<Value> = account["plans"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|plan| plan["run"].as_str())
            .map(|run| {
                // Asked once more; a run the helper forgot is what it last said.
                let _ = call("reflexStatus", json!({ "run": run }));
                let status = self.known.borrow().get(run).cloned().unwrap_or(Value::Null);
                json!({
                    "runId": run,
                    "status": status,
                    "report": super::watch_report(run),
                    "receipts": receipts_file(Path::new(""), run),
                })
            })
            .collect();
        let last = runs.last().cloned().unwrap_or(Value::Null);
        let beside = |ledger: Option<&Path>| {
            ledger.and_then(|path| path.strip_prefix(folder).ok().map(Path::to_path_buf))
        };
        self.end(
            call,
            json!({
                "road": autopilot::AUTOPILOT,
                "report": last["report"],
                "status": last["status"],
                "runs": runs,
                "autopilot": account,
                "ledgers": { "decisions": beside(decisions.as_deref()), "plans": beside(plans.as_deref()) },
                "endedNs": ended_ns,
            }),
            stop,
        );
    }
}
