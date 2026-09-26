//! The autopilot against a helper, a teacher and a generator of the test's
//! own, on a clock the test moves: no hand moves, no socket opens, and no
//! ledger but the test's own is written.

use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;

use super::super::plan::tests::{Named, Scripted, answer_for, capture, scope};
use super::super::tests::{answer_naming, open, reading_handshake};
use super::*;
use crate::systemone::Spent;
use crate::systemone::tests::ANSWERING_VERSION;

/// The epoch milliseconds the test's steady clock starts at.
const WALL: i64 = 1_790_000_000_000;

/// One run as the fake helper holds it.
struct Held {
    state: &'static str,
    reason: Option<String>,
    plan_hash: String,
    receipts: Vec<Value>,
    acked: u64,
    policy: Value,
}

/// A helper as an autopilot meets it — a display, the app's window on it,
/// one capture of it — and every run it was started on.
struct Helper {
    calls: Vec<(String, Value)>,
    runs: Vec<String>,
    held: BTreeMap<String, Held>,
    kernel: bool,
    /// What every reading says: a known target, or why none is known; how
    /// old its capture is; which plan the helper says it runs, when not the
    /// one it was started with; and a count that moves when the test says
    /// the run did something.
    unknown: Option<&'static str>,
    age_ns: u64,
    plan_hash: Option<&'static str>,
    moment: u64,
}

impl Helper {
    fn new() -> Self {
        Self {
            calls: Vec::new(),
            runs: Vec::new(),
            held: BTreeMap::new(),
            kernel: true,
            unknown: None,
            age_ns: 3_000_000,
            plan_hash: None,
            moment: 0,
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, ComputerUseError> {
        use base64::Engine as _;
        self.calls.push((method.to_string(), params.clone()));
        let run = params["run"].as_str().unwrap_or_default().to_string();
        Ok(match method {
            "displays" => json!({ "displays": [
                { "index": 0, "scale": 1, "bounds": { "x": 0, "y": 0, "width": 100, "height": 60 } },
            ] }),
            "listWindows" => json!({ "windows": [{
                "id": 7, "x": 10, "y": 10, "width": 80, "height": 40,
                "app": { "name": "Fixture", "bundleId": "com.example.Fixture", "pid": 9 },
            }] }),
            "screenshotDesktop" => {
                let (image, _) = capture();
                let png = image.encode().expect("a png");
                json!({ "screenshot": {
                    "data": base64::engine::general_purpose::STANDARD.encode(png),
                    "format": "png", "width": image.width, "height": image.height,
                } })
            }
            "handshake" if self.kernel => reading_handshake(),
            "handshake" => json!({ "supports": { "desktop": { "reflex": { "kernel": false } } } }),
            "reflexStart" => {
                let id = params["runId"].as_str().expect("a run id").to_string();
                let plan: ReflexPlan =
                    serde_json::from_str(params["plan"].as_str().expect("the plan's wire"))
                        .expect("a plan");
                self.runs.push(id.clone());
                self.held.insert(
                    id.clone(),
                    Held {
                        state: "running",
                        reason: None,
                        plan_hash: plan.plan_hash,
                        receipts: Vec::new(),
                        acked: 0,
                        policy: serde_json::from_str(
                            params["runPolicy"].as_str().expect("the run's policy"),
                        )
                        .expect("a policy"),
                    },
                );
                json!({ "runId": id, "state": "running" })
            }
            "reflexReceipts" => {
                let Some(held) = self.held.get(&run) else {
                    return Ok(json!({ "runId": run, "state": "missing" }));
                };
                let after = params["after"].as_u64().unwrap_or(0).max(held.acked);
                let receipts: Vec<&Value> = held
                    .receipts
                    .iter()
                    .filter(|receipt| receipt["seq"].as_u64() > Some(after))
                    .collect();
                json!({
                    "runId": run, "state": held.state, "reason": held.reason,
                    "planHash": self.plan_hash.map_or(held.plan_hash.clone(), str::to_string),
                    "receiptsIssued": held.receipts.len(), "receipts": receipts,
                    "sightings": [{
                        "detector": "ball", "value": if self.unknown.is_some() { Value::Null } else { json!(1) },
                        "unknown": self.unknown, "track": 5, "ageNs": 2_000_000,
                    }],
                    "outcomes": { "done": self.moment },
                    "scene": { "stream": 1, "geometry": 1, "owner": 3, "plan": 1 },
                    "lastCapture": self.moment, "lastCaptureAgeNs": self.age_ns,
                })
            }
            "reflexAck" => {
                if let Some(held) = self.held.get_mut(&run) {
                    held.acked = held.acked.max(params["through"].as_u64().unwrap_or(0));
                }
                json!({})
            }
            "reflexStop" => {
                if let Some(held) = self.held.get_mut(&run)
                    && held.state != "stopped"
                {
                    held.state = "stopped";
                    held.reason = Some("request".into());
                }
                json!({ "runId": run, "state": "stopped" })
            }
            other => json!({ "unexpected": other }),
        })
    }

    /// The runs a stop was sent for, in order.
    fn stops(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter(|(method, _)| method == "reflexStop")
            .map(|(_, params)| params["run"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The hand pressed `pressed` targets on `run` and went for `missed`
    /// more it lost: a click that landed each, a move that found its target
    /// gone each.
    fn press(&mut self, run: &str, pressed: u64, missed: u64) {
        let held = self.held.get_mut(run).expect("a run");
        for (action, outcome, times) in [("click1", "done", pressed), ("move1", "moved", missed)] {
            for _ in 0..times {
                let seq = held.receipts.len() as u64 + 1;
                held.receipts.push(json!({
                    "seq": seq, "ruleId": "follow", "actionId": action, "outcome": outcome,
                }));
            }
        }
        self.moment += 1;
    }

    /// The helper held or ended the run by itself.
    fn ends(&mut self, run: &str, state: &'static str, reason: &str) {
        let held = self.held.get_mut(run).expect("a run");
        held.state = state;
        held.reason = Some(reason.to_string());
    }
}

/// One scripted answer of the teacher's: what the wire came to, the
/// requests it cost, and — when the test holds it — the gate it waits on.
type Line = (Result<String, String>, u32, Option<mpsc::Receiver<()>>);

/// The teacher, answering in the order questions reach it — `continue` once
/// the script runs out — and counting the questions it heard.
#[derive(Clone, Default)]
struct Teacher {
    script: Arc<Mutex<VecDeque<Line>>>,
    heard: Arc<AtomicUsize>,
}

impl Teacher {
    fn says(&self, word: &str) {
        self.line(Ok(answer_naming(word)), 1, None);
    }

    fn fails(&self, token: &str, attempts: u32) {
        self.line(Err(token.to_string()), attempts, None);
    }

    /// `word`, held until the gate handed back is opened.
    fn holds(&self, word: &str) -> mpsc::Sender<()> {
        let (open, gate) = mpsc::channel();
        self.line(Ok(answer_naming(word)), 1, Some(gate));
        open
    }

    fn line(
        &self,
        answer: Result<String, String>,
        attempts: u32,
        gate: Option<mpsc::Receiver<()>>,
    ) {
        self.script
            .lock()
            .expect("the script")
            .push_back((answer, attempts, gate));
    }

    fn asker(&self) -> Asker {
        let teacher = self.clone();
        Arc::new(move |_state: Value| {
            teacher.heard.fetch_add(1, Ordering::SeqCst);
            let line = teacher.script.lock().expect("the script").pop_front();
            let (answer, attempts, gate) =
                line.unwrap_or_else(|| (Ok(answer_naming(CONTINUE)), 1, None));
            if let Some(gate) = gate {
                let _ = gate.recv();
            }
            let answered = answer.is_ok();
            (
                reflex_decide::Wired {
                    answer,
                    attempts,
                    request_bytes: if attempts > 0 { 400 } else { 0 },
                    rtt_ms: 150,
                },
                Spent {
                    requests: attempts,
                    redacted_lines: 0,
                    model: answered.then(|| ANSWERING_VERSION.to_string()),
                },
            )
        })
    }
}

/// A sink that keeps every receipt.
#[derive(Default)]
struct Kept(Vec<u8>);

impl ReceiptSink for Kept {
    fn keep(&mut self, durable: u64, lines: &[u8]) -> std::io::Result<u64> {
        self.0
            .truncate(usize::try_from(durable).unwrap_or(usize::MAX));
        self.0.extend_from_slice(lines);
        Ok(self.0.len() as u64)
    }
}

/// A generator nobody set up.
struct Unset;

impl Generator for Unset {
    fn unready(&self) -> Option<String> {
        Some(crate::computer_use::errand::value::NO_KEY.to_string())
    }

    fn model(&self) -> Option<String> {
        None
    }

    fn ask(
        &mut self,
        _system: &str,
        _user: &str,
        _left: Duration,
    ) -> Result<crate::computer_use::errand::value::Said, String> {
        panic!("a generator nobody set up is never asked")
    }
}

/// The world the test stands in for.
struct Fake {
    helper: Helper,
    teacher: Teacher,
    ask: Asker,
    /// The seat's word and whether it applies.
    standing: (JevMode, bool),
    generator: Scripted,
    decisions: Vec<Value>,
    plans: Vec<Value>,
    now: u64,
    stopped: Option<String>,
}

impl Fake {
    /// A world whose seat stands at `standing` and whose generator writes
    /// `plans`, one per request.
    fn new(standing: (JevMode, bool), plans: Vec<Result<String, String>>) -> Self {
        let teacher = Teacher::default();
        Self {
            helper: Helper::new(),
            ask: teacher.asker(),
            teacher,
            standing,
            generator: Scripted {
                answers: plans.into(),
                asked: Vec::new(),
            },
            decisions: Vec::new(),
            plans: Vec::new(),
            now: 10_000,
            stopped: None,
        }
    }

    fn with<R>(
        &mut self,
        generator: Option<&mut dyn Generator>,
        act: impl FnOnce(&mut World<'_>) -> R,
    ) -> R {
        let Self {
            helper,
            ask,
            standing,
            generator: scripted,
            decisions,
            plans,
            now,
            stopped,
            ..
        } = self;
        let (mode, applies) = *standing;
        let mut call = |method: &str, params: Value| helper.call(method, params);
        let mut read_mode = move || mode;
        let mut read_standing = move || (mode, applies);
        let mut record = |rows: Vec<Value>| decisions.extend(rows);
        let mut write = |rows: Vec<Value>| plans.extend(rows);
        let mut keeper = |_run: &str| {
            (
                None,
                Box::new(Kept::default()) as Box<dyn ReceiptSink + Send>,
            )
        };
        let generator: &mut dyn Generator = match generator {
            Some(generator) => generator,
            None => scripted,
        };
        let mut world = World {
            call: &mut call,
            ask: &*ask,
            mode: &mut read_mode,
            standing: &mut read_standing,
            generator,
            decisions: &mut record,
            plans: &mut write,
            keeper: &mut keeper,
            now_ms: *now,
            wall_ms: WALL + i64::try_from(*now).unwrap_or(0),
            stopped: stopped.clone(),
        };
        act(&mut world)
    }

    fn start(&mut self, asked: Asked) -> Result<(Autopilot, Value), ComputerUseError> {
        self.with(None, |world| Autopilot::start(asked, open(), None, world))
    }

    fn tick(&mut self, autopilot: &mut Autopilot) -> bool {
        self.with(None, |world| autopilot.tick(world))
    }

    /// The rows every settled question left, in order.
    fn asked_rows(&self) -> Vec<&Value> {
        self.decisions
            .iter()
            .filter(|row| {
                row["road"] == json!(reflex_decide::ROAD_JEV) || row["road"] == json!(ROAD_DOOR)
            })
            .collect()
    }

    /// Collect, the clock standing, until one more question settled.
    fn until_settled(&mut self, autopilot: &mut Autopilot) {
        let before = self.asked_rows().len();
        for _ in 0..400 {
            self.tick(autopilot);
            if self.asked_rows().len() > before {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("no question settled");
    }

    /// Collect until the autopilot is done, a second a collect.
    fn until_done(&mut self, autopilot: &mut Autopilot) {
        for _ in 0..400 {
            if self.tick(autopilot) {
                return;
            }
            self.now += REFLEX_COLLECT_MS;
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the autopilot never finished");
    }

    /// The label rows written for the decisions, in order.
    fn labels(&self) -> Vec<&Value> {
        self.decisions
            .iter()
            .filter(|row| row.get(LABEL.canonical).is_some())
            .collect()
    }
}

/// What a test's person asks: press the red dots in the fixture for a
/// minute, the decision forced when `forced` says so.
fn asked(forced: Option<JevMode>) -> Asked {
    Asked {
        goal: "press the red dots, never the blue".into(),
        app: "Fixture".into(),
        display: 0,
        seconds: 60,
        renew: true,
        forced,
    }
}

fn good() -> Result<String, String> {
    Ok(answer_for(&scope()).to_string())
}

/// A pause carried out stops the run, says why, and writes no new plan while
/// the run's detectors still found something: its row says it was carried
/// out, on the grounds it was asked on — the run, its epoch and its plan —
/// and the status counts it on its road. Its label cannot read a stopped
/// hand's screen and says so.
#[test]
fn a_pause_carried_out_stops_the_run_and_says_why() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    fake.teacher.says(PAUSE);
    let (mut autopilot, answer) = fake.start(asked(None)).expect("started");
    let run = answer["runId"].as_str().expect("a run").to_string();
    assert_eq!(answer["epoch"], json!(1));
    assert_eq!(answer["plan"]["requests"], json!(1));
    fake.until_settled(&mut autopilot);
    let row = fake.asked_rows()[0].clone();
    assert_eq!(row["chosen"], json!(PAUSE));
    assert_eq!(row["applied"], json!(true));
    assert!(row.get("why").is_none());
    assert_eq!(row["mode"], json!("auto"));
    assert_eq!(row["provenance"]["epoch"], json!(1));
    assert_eq!(row["provenance"]["planHash"], answer["planHash"]);
    assert_eq!(row["provenance"]["forced"], json!(false));
    assert_eq!(fake.helper.stops(), std::slice::from_ref(&run));
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!(PAUSED))
    );
    assert_eq!(fake.helper.runs.len(), 1, "no new plan");
    assert_eq!(fake.generator.asked.len(), 1);
    fake.until_done(&mut autopilot);
    let label = fake.labels()[0].clone();
    assert_eq!(label["label"], json!(format!("{run}:1")));
    assert_eq!(label["requestAt"], row["at"]);
    assert_eq!(label["notCompared"], json!(reflex_decide::HAND_STOPPED));
    let status = report(&run).expect("the autopilot's account");
    assert_eq!(status["applied"][PAUSE], json!(1));
    assert_eq!(
        status["roads"],
        json!({ "memo": 0, "surrogate": 0, "jev": 1 })
    );
    assert_eq!(status["ended"]["reason"], json!(PAUSED));
    assert_eq!(status["plans"][0]["source"], json!(plan::SOURCE_MODEL));
    assert_eq!(fake.helper.stops(), [run], "one stop");
}

/// A pause carried out after the run found nothing for the collects the
/// table names hands it to a new plan: the old one stopped, the generator
/// asked again with the plan it replaces and how its actions ended, a new
/// run on the next epoch.
#[test]
fn a_pause_after_collects_that_found_nothing_writes_a_new_plan() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good(), good()]);
    fake.helper.unknown = Some("occluded");
    let open = fake.teacher.holds(PAUSE);
    let (mut autopilot, answer) = fake.start(asked(None)).expect("started");
    let first = answer["runId"].as_str().expect("a run").to_string();
    for _ in 0..REFLEX_REPLAN_AFTER_UNKNOWN_PASSES {
        fake.tick(&mut autopilot);
    }
    open.send(()).expect("the gate");
    fake.until_settled(&mut autopilot);
    assert_eq!(fake.helper.stops(), std::slice::from_ref(&first));
    assert_eq!(autopilot.ended(), None, "a new plan, not the end");
    assert_eq!(fake.helper.runs.len(), 2);
    let second = fake.helper.runs[1].clone();
    let (_, asked_again) = &fake.generator.asked[1];
    let asked_again: Value = serde_json::from_str(asked_again).expect("json");
    assert_eq!(asked_again["previous"]["plan"]["scope"], json!(scope()));
    assert!(asked_again["previous"]["outcomes"].is_object());
    let status = report(&second).expect("the account");
    assert_eq!(status["current"]["epoch"], json!(2));
    assert_eq!(status["plans"].as_array().map(Vec::len), Some(2));
    assert_eq!(report(&first), Some(status), "one account for every run");
}

/// A re-plan carried out stops the run and runs the next plan in the same
/// scope, for what is left of the wall; the new run's questions carry the
/// next epoch; and the re-plan is graded across its two runs — the old
/// plan's last seconds against the new plan's first.
#[test]
fn a_replan_carried_out_runs_the_next_plan_and_is_graded_across_both_runs() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good(), good()]);
    fake.teacher.says(REPLAN);
    let (mut autopilot, answer) = fake.start(asked(None)).expect("started");
    let first = answer["runId"].as_str().expect("a run").to_string();
    fake.helper.press(&first, 1, 3);
    fake.now += 20_000;
    fake.until_settled(&mut autopilot);
    assert_eq!(fake.helper.stops(), std::slice::from_ref(&first));
    assert_eq!(fake.helper.runs.len(), 2);
    let second = fake.helper.runs[1].clone();
    let policy = &fake.helper.held[&second].policy;
    let run_ns = policy["run_ns"].as_u64().expect("a wall");
    assert!(
        (39_000_000_000..=40_000_000_000).contains(&run_ns),
        "what is left of the wall: {run_ns}"
    );
    assert_eq!(fake.asked_rows()[0]["applied"], json!(true));
    // The new run asks under its own epoch.
    fake.helper.press(&second, 3, 1);
    fake.now += REFLEX_COLLECT_MS;
    fake.until_settled(&mut autopilot);
    let asked_new = fake.asked_rows().last().copied().cloned().expect("a row");
    assert_eq!(asked_new["run"], json!(second));
    assert_eq!(asked_new["provenance"]["epoch"], json!(2));
    fake.now += REFLEX_REPLAN_COMPARE_MS;
    fake.tick(&mut autopilot);
    let label = fake
        .labels()
        .into_iter()
        .find(|label| label["label"] == json!(format!("{first}:1")))
        .cloned()
        .expect("the re-plan's label");
    assert_eq!(label["agreed"], json!(true), "{label}");
    assert_eq!(label["baselineAgreed"], json!(false));
    assert_eq!(
        label["share"]["before"],
        json!({ "kind": "screen", "pressed": 1, "missed": 3 })
    );
    assert_eq!(
        label["share"]["after"],
        json!({ "kind": "screen", "pressed": 3, "missed": 1 })
    );
    let status = report(&second).expect("the account");
    assert_eq!(status["applied"][REPLAN], json!(1));
    assert_eq!(status["current"]["run"], json!(second));
}

/// A re-plan written for another app is asked again once with the scope's
/// own sentence, and a second one stops the autopilot — a plan is never
/// where the permission to act somewhere else comes from — not as
/// `escalated`, and its row says why.
#[test]
fn a_replan_for_another_scope_is_asked_again_once_then_stops() {
    let other = Scope {
        target: "com.example.Other".into(),
        ..scope()
    };
    let mut fake = Fake::new(
        (JevMode::Auto, true),
        vec![
            good(),
            Ok(answer_for(&other).to_string()),
            Ok(answer_for(&other).to_string()),
        ],
    );
    fake.teacher.says(REPLAN);
    let (mut autopilot, _) = fake.start(asked(None)).expect("started");
    fake.until_settled(&mut autopilot);
    assert_eq!(fake.generator.asked.len(), 3);
    let (_, second_ask) = &fake.generator.asked[2];
    assert!(
        second_ask.contains("com.example.Other"),
        "the scope's sentence"
    );
    assert_eq!(fake.helper.runs.len(), 1, "no plan for another app ran");
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!(plan::SCOPE_REFUSED))
    );
    // The plan's own row says why; the stopped run's plan label follows it.
    assert_eq!(
        fake.plans
            .iter()
            .rev()
            .find_map(|row| row.get("outcome").cloned()),
        Some(json!(plan::SCOPE_REFUSED))
    );
}

/// Under `shadow`, and with the decision off, nothing the teacher says
/// reaches the hand: the rows say `applied: false` and why — and whether the
/// answer would have been fit to carry out — and the helper hears no stop
/// and no second start. Off asks nothing.
#[test]
fn shadow_and_off_change_no_input() {
    let mut fake = Fake::new((JevMode::Shadow, false), vec![good()]);
    for word in [PAUSE, REPLAN] {
        fake.teacher.says(word);
    }
    let (mut autopilot, _) = fake.start(asked(None)).expect("started");
    fake.until_settled(&mut autopilot);
    fake.helper.moment += 1;
    fake.until_settled(&mut autopilot);
    for row in fake.asked_rows() {
        assert_eq!(row["applied"], json!(false));
        assert_eq!(row["why"], json!(Why::NotAuto.word()));
        assert_eq!(row["mode"], json!("shadow"));
    }
    assert!(fake.helper.stops().is_empty());
    assert_eq!(fake.helper.runs.len(), 1);
    assert_eq!(autopilot.ended(), None);
    // Off: nothing is asked at all.
    let mut off = Fake::new((JevMode::Shadow, false), vec![good()]);
    let (mut autopilot, _) = off.start(asked(Some(JevMode::Off))).expect("started");
    for _ in 0..5 {
        off.helper.moment += 1;
        off.tick(&mut autopilot);
    }
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(off.teacher.heard.load(Ordering::SeqCst), 0);
    assert!(off.helper.stops().is_empty());
}

/// An answer about a reading older than the table allows, or about another
/// plan than the one the helper runs, is never carried out — and says which;
/// nor is an answer about a run that has ended by the time it lands.
#[test]
fn a_stale_or_mismatched_or_late_answer_is_never_carried_out() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    fake.helper.age_ns = 2_500_000_000;
    fake.teacher.says(PAUSE);
    let (mut autopilot, _) = fake.start(asked(None)).expect("started");
    fake.until_settled(&mut autopilot);
    assert_eq!(fake.asked_rows()[0]["why"], json!(Why::Stale.word()));
    fake.helper.age_ns = 3_000_000;
    fake.helper.plan_hash = Some("another");
    fake.helper.moment += 1;
    fake.teacher.says(PAUSE);
    fake.tick(&mut autopilot);
    fake.until_settled(&mut autopilot);
    assert_eq!(fake.asked_rows()[1]["why"], json!(Why::PlanMismatch.word()));
    assert!(fake.helper.stops().is_empty(), "neither stopped the run");
    // A question in flight when a person stops the autopilot lands on a run
    // that is no longer the hand's.
    fake.helper.plan_hash = None;
    fake.helper.moment += 1;
    let open = fake.teacher.holds(REPLAN);
    fake.tick(&mut autopilot);
    fake.tick(&mut autopilot);
    let run = fake.helper.runs[0].clone();
    assert_eq!(stop_owner(&run), Some(Some(run.clone())));
    fake.tick(&mut autopilot);
    open.send(()).expect("the gate");
    fake.until_settled(&mut autopilot);
    let late = fake.asked_rows().last().copied().cloned().expect("a row");
    assert_eq!(late["why"], json!(Why::EpochMismatch.word()));
    assert_eq!(late["applied"], json!(false));
    assert_eq!(
        fake.helper.runs.len(),
        1,
        "nothing re-planned after the stop"
    );
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!(STOPPED))
    );
    let status = report(&run).expect("the account");
    assert_eq!(status["invalid"]["stale"], json!(1));
    assert_eq!(status["invalid"]["plan_mismatch"], json!(1));
    assert_eq!(status["invalid"]["epoch_mismatch"], json!(1));
}

/// Three questions running that come back unanswered or unfit to carry out,
/// while the decision is carried out, end the run `escalated`. A door that
/// refused and a wire that failed are the person's settings and the network,
/// counted apart and never escalated; and nothing escalates under `shadow`.
#[test]
fn three_unusable_answers_escalate_and_a_door_or_a_wire_never_does() {
    let mut fake = Fake::new((JevMode::Shadow, false), vec![good()]);
    let (mut autopilot, answer) = fake.start(asked(Some(JevMode::Auto))).expect("started");
    let run = answer["runId"].as_str().expect("a run").to_string();
    for _ in 0..4 {
        fake.teacher.fails("not_consented", 0);
        fake.helper.moment += 1;
        fake.until_settled(&mut autopilot);
    }
    for _ in 0..4 {
        fake.teacher.fails("http_503", 1);
        fake.helper.moment += 1;
        fake.until_settled(&mut autopilot);
    }
    assert_eq!(autopilot.ended(), None);
    let status = report(&run).expect("the account");
    assert_eq!(status["door"], json!({ "not_consented": 4 }));
    assert_eq!(status["wire"], json!({ "http_503": 4 }));
    for _ in 0..REFLEX_ESCALATE_AFTER {
        fake.teacher.fails(systemone::TIMEOUT, 1);
        fake.helper.moment += 1;
        fake.until_settled(&mut autopilot);
    }
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!(ESCALATED))
    );
    assert_eq!(fake.helper.stops(), std::slice::from_ref(&run));
    assert_eq!(report(&run).expect("the account")["unanswered"], json!(3));
    // The same three under a forced `shadow`: recorded, never escalated.
    let mut shadow = Fake::new((JevMode::Auto, true), vec![good()]);
    let (mut autopilot, _) = shadow.start(asked(Some(JevMode::Shadow))).expect("started");
    for _ in 0..=REFLEX_ESCALATE_AFTER {
        shadow.teacher.fails(systemone::TIMEOUT, 1);
        shadow.helper.moment += 1;
        shadow.until_settled(&mut autopilot);
    }
    assert_eq!(autopilot.ended(), None);
    assert!(shadow.helper.stops().is_empty());
}

/// A person's input — the helper holds the run — the operator's stop and a
/// `reflex-stop` each end the autopilot with their own reason, and nothing
/// re-plans after any of them, whatever the teacher said.
#[test]
fn a_persons_hand_or_a_stop_ends_it_and_nothing_replans() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good(), good()]);
    let (mut autopilot, answer) = fake.start(asked(None)).expect("started");
    let run = answer["runId"].as_str().expect("a run").to_string();
    fake.helper.ends(&run, "paused", "external_input");
    fake.teacher.says(REPLAN);
    fake.tick(&mut autopilot);
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!("external_input"))
    );
    assert_eq!(fake.helper.stops(), [run], "a held run is stopped");
    assert_eq!(fake.helper.runs.len(), 1);
    let mut stopped = Fake::new((JevMode::Auto, true), vec![good(), good()]);
    let (mut autopilot, _) = stopped.start(asked(None)).expect("started");
    stopped.stopped = Some("hotkey".into());
    stopped.tick(&mut autopilot);
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!("hotkey"))
    );
    assert_eq!(stopped.helper.runs.len(), 1);
}

/// No generator set up, no autopilot: the refusal names the word and the
/// road a person's own plan takes, and nothing — the helper, the screen, a
/// generator — was asked. A door that is closed refuses before anything is
/// asked either.
#[test]
fn no_generator_or_a_closed_door_starts_nothing() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    let mut unset = Unset;
    let refused = fake
        .with(Some(&mut unset), |world| {
            Autopilot::start(asked(None), open(), None, world)
        })
        .err()
        .expect("refused");
    assert_eq!(refused.code, error_code::UNSUPPORTED_CAPABILITY);
    assert!(
        refused.message.starts_with(plan::NO_GENERATOR),
        "{}",
        refused.message
    );
    assert!(refused.message.contains("reflex-start"));
    assert!(fake.helper.calls.is_empty());
    let closed = DoorFacts {
        enabled: false,
        ..open()
    };
    let refused = fake
        .with(None, |world| {
            Autopilot::start(asked(None), closed, None, world)
        })
        .err()
        .expect("refused");
    assert_eq!(refused.code, error_code::UNSUPPORTED_CAPABILITY);
    assert!(fake.helper.calls.is_empty() && fake.generator.asked.is_empty());
}

/// A reflex autopilot starts on a plan the person's real login wrote, on this
/// machine, with no API key (t-10372 §6 (a)): the plan's row says which road
/// wrote it and what it cost, and the helper was handed a run. Printed.
#[test]
#[ignore = "spends the person's own login on one plan; evidence for the report"]
fn an_autopilot_on_this_machine_starts_on_the_plan_its_login_wrote() {
    use crate::computer_use::errand::value::LiveWriter;
    use crate::computer_use::errand::value::tests::probe_setup;
    use zerocode_core::type_value::GeneratorRoad;

    let road = std::env::var("ZEROCODE_PROBE_ROAD")
        .ok()
        .and_then(|word| serde_json::from_value(json!(word)).ok())
        .unwrap_or(GeneratorRoad::Auto);
    let mut writer = LiveWriter::window(probe_setup(road));
    let mut fake = Fake::new((JevMode::Off, false), Vec::new());
    let started = fake.with(Some(&mut writer), |world| {
        Autopilot::start(asked(None), open(), None, world)
    });
    println!(
        "{}",
        json!({
            "started": started.as_ref().map(|(_, answer)| answer.clone()).map_err(|error| json!({ "code": error.code, "message": error.message })),
            "plans": fake.plans,
            "runs": fake.helper.runs.len(),
        })
    );
    assert!(started.is_ok(), "the autopilot did not start");
    assert_eq!(fake.helper.runs.len(), 1, "the helper was handed no run");
}

/// Every plan passes the helper's handshake before a start: a helper that
/// does not read the contract is never sent one, and the plan's row says
/// what became of it.
#[test]
fn every_plan_passes_the_door_and_the_handshake() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    fake.helper.kernel = false;
    let refused = fake.start(asked(None)).err().expect("refused");
    assert_eq!(refused.code, error_code::UNSUPPORTED_CAPABILITY);
    assert!(fake.helper.runs.is_empty());
    assert_eq!(
        fake.plans.last().map(|row| row["outcome"].clone()),
        Some(json!(error_code::UNSUPPORTED_CAPABILITY))
    );
    // A plan that never passed the contract starts nothing, after the retries.
    let mut refusing = Fake::new(
        (JevMode::Auto, true),
        vec![Ok("no".into()), Ok("no".into()), Ok("no".into())],
    );
    let refused = refusing.start(asked(None)).err().expect("refused");
    assert_eq!(refused.code, plan::PLAN_REFUSED);
    assert!(refusing.helper.runs.is_empty());
    assert_eq!(refusing.plans[0]["requests"], json!(3));
}

/// Every answer is graded on what the hand did in the window after it, the
/// label naming its request by run, decision and time; a written plan is
/// graded on its run's first seconds in the plan ledger; a forced decision's
/// rows say so and take the same road; and the status's roads add up to the
/// decisions carried out.
#[test]
fn labels_grade_every_answer_and_the_roads_add_up_to_what_was_carried_out() {
    let mut fake = Fake::new((JevMode::Shadow, false), vec![good()]);
    fake.teacher.says(CONTINUE);
    let (mut autopilot, answer) = fake.start(asked(Some(JevMode::Auto))).expect("started");
    let run = answer["runId"].as_str().expect("a run").to_string();
    fake.until_settled(&mut autopilot);
    let row = fake.asked_rows()[0].clone();
    assert_eq!(row["provenance"]["forced"], json!(true));
    assert_eq!(row["road"], json!(ROAD_JEV));
    assert_eq!(row["applied"], json!(true));
    fake.now += REFLEX_COLLECT_MS;
    fake.helper.press(&run, 2, 0);
    fake.tick(&mut autopilot);
    fake.now += REFLEX_LABEL_WINDOW_MS;
    fake.tick(&mut autopilot);
    let label = fake
        .labels()
        .into_iter()
        .find(|label| label["label"] == json!(format!("{run}:1")))
        .cloned()
        .expect("the label");
    assert_eq!(label["requestAt"], row["at"]);
    assert_eq!(label["kind"], json!(LABEL_KIND));
    assert_eq!(label["agreed"], json!(true));
    assert_eq!(label["baselineAgreed"], json!(true));
    assert_eq!(
        label["share"],
        json!({ "kind": "screen", "pressed": 2, "missed": 0 })
    );
    fake.now += REFLEX_PLAN_LABEL_MS;
    fake.tick(&mut autopilot);
    let written = fake
        .plans
        .iter()
        .find(|row| row.get(LABEL.canonical) == Some(&json!(run)))
        .cloned()
        .expect("the plan's label");
    assert_eq!(written["kind"], json!(LABEL_KIND));
    assert_eq!(written["cut"], json!(false));
    assert_eq!(written["share"]["pressed"], json!(2));
    let status = report(&run).expect("the account");
    let sum = |key: &str| {
        status[key].as_object().map_or(0, |counts| {
            counts.values().filter_map(Value::as_u64).sum::<u64>()
        })
    };
    assert!(sum("applied") >= 1);
    assert_eq!(sum("roads"), sum("applied"));
    assert_eq!(status["l1"]["forced"], json!("auto"));
}

/// A status of any of the autopilot's runs carries its account, and a
/// `reflex-stop` of one ends the autopilot at its next collect.
#[test]
fn a_status_carries_the_account_and_a_stop_ends_the_autopilot() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    let (mut autopilot, answer) = fake.start(asked(None)).expect("started");
    let run = answer["runId"].as_str().expect("a run").to_string();
    let helper = &mut fake.helper;
    let status = super::super::status(&json!({ "run": run }), &open(), &mut |method, params| {
        helper.call(method, params)
    })
    .expect("a status");
    assert_eq!(status[AUTOPILOT]["current"]["run"], json!(run));
    assert_eq!(status[AUTOPILOT]["ended"], Value::Null);
    let stopped = super::super::stop(&json!({ "run": run }), &mut |method, params| {
        helper.call(method, params)
    })
    .expect("stopped");
    assert_eq!(stopped["state"], json!("stopped"));
    fake.tick(&mut autopilot);
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!(STOPPED))
    );
    assert_eq!(fake.helper.runs.len(), 1);
}

/// The words a screen's share is counted in are the helper's own receipt
/// outcomes (`ReflexReceipt.Outcome`): a word the helper stopped writing
/// would leave every share empty with nobody told.
#[test]
fn the_share_is_counted_in_the_helpers_own_outcome_words() {
    let scheduler = include_str!(
        "../../../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/ReflexScheduler.swift"
    );
    let outcomes = &scheduler[scheduler
        .find("public enum Outcome: String")
        .expect("the receipt's outcome")..];
    let outcomes = &outcomes[..outcomes.find("\n    }").expect("its end")];
    for word in std::iter::once(REFLEX_PRESSED_OUTCOME).chain(REFLEX_MISSED_OUTCOMES) {
        assert!(
            outcomes.contains(&format!("case {word}\n")),
            "`{word}` is not a receipt outcome"
        );
    }
}

/// The autopilot's account names every plan by the generator that wrote it:
/// a stand-in's plans are never said to be a model's.
#[test]
fn the_account_names_the_generator_each_plan_came_from() {
    let mut fake = Fake::new((JevMode::Auto, true), Vec::new());
    let mut stub = Named {
        scripted: Scripted {
            answers: vec![good()].into(),
            asked: Vec::new(),
        },
        source: "stub",
    };
    let (_autopilot, answer) = fake
        .with(Some(&mut stub), |world| {
            Autopilot::start(asked(Some(JevMode::Auto)), open(), None, world)
        })
        .expect("started");
    let run = answer["runId"].as_str().expect("a run");
    let status = report(run).expect("the autopilot's account");
    assert_eq!(status["plans"][0]["source"], json!("stub"));
    assert_eq!(fake.plans[0]["source"], json!("stub"));
}

/// A pause about a hand with nothing to stop — its reading found nothing to
/// press, and the hand pressed nothing over the collect before — is not
/// carried out: right after a start, before any target shows, the hand
/// already waits, and the run goes on. Its row says `idle`, the account
/// counts it, and nothing counts it toward an escalation. A pause about a
/// hand that was pressing still stops the run.
#[test]
fn a_pause_about_a_hand_with_nothing_to_stop_leaves_the_run_standing() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    fake.helper.unknown = Some("occluded");
    let open = fake.teacher.holds(PAUSE);
    let (mut autopilot, answer) = fake.start(asked(None)).expect("started");
    let run = answer["runId"].as_str().expect("a run").to_string();
    fake.tick(&mut autopilot);
    open.send(()).expect("the gate");
    std::thread::sleep(Duration::from_millis(50));
    fake.until_settled(&mut autopilot);
    let row = fake.asked_rows()[0].clone();
    assert_eq!(row["chosen"], json!(PAUSE));
    assert_eq!(row["applied"], json!(false));
    assert_eq!(row["why"], json!(Why::Idle.word()));
    assert!(fake.helper.stops().is_empty(), "the hand was never stopped");
    assert_eq!(autopilot.ended(), None, "the run goes on");
    let status = report(&run).expect("the account");
    assert_eq!(status["invalid"][Why::Idle.word()], json!(1));
    assert_eq!(status["applied"][PAUSE], json!(0));

    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    fake.helper.unknown = Some("occluded");
    let open = fake.teacher.holds(PAUSE);
    let (mut autopilot, answer) = fake.start(asked(None)).expect("started");
    let run = answer["runId"].as_str().expect("a run").to_string();
    fake.tick(&mut autopilot);
    // The question went on an empty reading; the hand then pressed a
    // target it saw.
    fake.helper.press(&run, 1, 0);
    fake.helper.unknown = None;
    fake.tick(&mut autopilot);
    open.send(()).expect("the gate");
    std::thread::sleep(Duration::from_millis(50));
    fake.until_settled(&mut autopilot);
    let row = fake.asked_rows()[0].clone();
    assert_eq!(row["applied"], json!(true), "a pressing hand is stopped");
    assert_eq!(fake.helper.stops(), [run]);
    assert_eq!(
        autopilot.ended().map(|ended| ended["reason"].clone()),
        Some(json!(PAUSED))
    );
}
