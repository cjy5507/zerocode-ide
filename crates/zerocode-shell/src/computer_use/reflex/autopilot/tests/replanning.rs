//! The autopilot while a re-plan is written (t-21494): the model that writes
//! the next plan is held by the test, and the valid run standing meanwhile
//! must keep being read and judged; only an answer whose premises still
//! stand replaces that run, under the same door every plan passes.

use super::*;

/// A model the test holds: its first request — the autopilot's first plan —
/// answers at once; its second, the re-plan, tells the test and the
/// collector that it was asked and waits on the test's gate. Every
/// request's budget is remembered, and each copy says when it is dropped.
#[derive(Clone)]
struct Delayed {
    calls: Arc<AtomicUsize>,
    entered: mpsc::Sender<()>,
    collecting: mpsc::Sender<()>,
    finished: mpsc::Sender<()>,
    gate: Arc<Mutex<mpsc::Receiver<()>>>,
    answer: String,
    budgets: Arc<Mutex<Vec<Duration>>>,
}

impl Drop for Delayed {
    fn drop(&mut self) {
        let _ = self.finished.send(());
    }
}

impl Generator for Delayed {
    fn unready(&self) -> Option<String> {
        None
    }

    fn model(&self) -> Option<String> {
        Some("held-plan-test".into())
    }

    fn ask(
        &mut self,
        system: &str,
        user: &str,
        left: Duration,
    ) -> Result<crate::computer_use::errand::value::Said, String> {
        let at = self.calls.fetch_add(1, Ordering::SeqCst);
        self.budgets.lock().expect("budgets").push(left);
        if at == 1 {
            self.entered.send(()).expect("the test hears the model");
            self.collecting
                .send(())
                .expect("the collector hears the model");
            self.gate
                .lock()
                .expect("one request")
                .recv_timeout(Duration::from_secs(10))
                .expect("the test releases its model");
        }
        let mut scripted = Scripted {
            answers: vec![if at == 0 {
                good()
            } else {
                Ok(self.answer.clone())
            }]
            .into(),
            asked: Vec::new(),
        };
        scripted.ask(system, user, left)
    }
}

/// What the collector did while the re-plan's model was held: the helper's
/// reads, the stops, the questions, and the capture age the fake answered.
struct Progress {
    passes: usize,
    stops: usize,
    questions: usize,
    capture_age_ns: u64,
}

/// One autopilot whose re-plan's model is held: sixteen collects go by
/// while it is, then `case` changes one premise — or nothing, for `healthy`
/// — and the model is released. While it was held the run must have been
/// read every collect, never stopped, and judged; afterwards the answer
/// replaces the run only for `healthy`, and every other case ends the
/// autopilot with no second run and no run left accepting input.
fn held_replan(case: &'static str) {
    let (entered, started) = mpsc::channel();
    let (collecting, collecting_started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let (progress, collected) = mpsc::channel();
    let (finish, finishing) = mpsc::channel();
    let (finished, drained) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let budgets = Arc::new(Mutex::new(Vec::new()));
    let delayed = Delayed {
        calls: Arc::clone(&calls),
        entered,
        collecting,
        finished,
        gate: Arc::new(Mutex::new(gate)),
        answer: if case == "stop" {
            "not json".into()
        } else {
            good().expect("a plan")
        },
        budgets: Arc::clone(&budgets),
    };
    let driver = std::thread::spawn(move || {
        let mut fake = Fake::new((JevMode::Auto, true), Vec::new());
        fake.teacher.says(REPLAN);
        let first = delayed.clone();
        let mut generator = plan::Background::new(
            Box::new(first),
            Box::new(move || Box::new(delayed) as Box<dyn Generator>),
        );
        let (mut pilot, answer) = fake
            .with(Some(&mut generator), |world| {
                Autopilot::start(asked(None), open(), None, world)
            })
            .expect("started");
        let run = answer["runId"].as_str().expect("a run").to_string();
        for _ in 0..400 {
            fake.with(Some(&mut generator), |world| pilot.tick(world));
            if !fake.asked_rows().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        collecting_started
            .recv_timeout(Duration::from_secs(2))
            .expect("model entered");
        let before = fake.helper.calls.len();
        let questions = fake.teacher.heard.load(Ordering::SeqCst);
        if case == "healthy" {
            fake.with(Some(&mut generator), |world| {
                let request_at = world.wall_ms;
                pilot.carry_out(
                    world,
                    &Carry {
                        run: run.clone(),
                        decision: 99,
                        request_at,
                        chosen: REPLAN,
                    },
                );
            });
        }
        for _ in 0..16 {
            fake.helper.moment += 1;
            fake.now += REFLEX_COLLECT_MS;
            fake.with(Some(&mut generator), |world| pilot.tick(world));
            std::thread::sleep(Duration::from_millis(2));
        }
        let passes = fake.helper.calls[before..]
            .iter()
            .filter(|(method, _)| method == "reflexReceipts")
            .count();
        let p = Progress {
            passes,
            stops: fake.helper.stops().len(),
            questions: fake.teacher.heard.load(Ordering::SeqCst) - questions,
            capture_age_ns: fake.helper.age_ns,
        };
        match case {
            "stop" => fake.stopped = Some("hotkey".into()),
            "disabled" => fake.enabled = false,
            "owner_stop" => pilot.stop.store(true, Ordering::SeqCst),
            "missing_scene" => fake.helper.scene = Value::Null,
            "deadline" => fake.now = pilot.deadline_ms,
            "jev_off" => fake.standing = (JevMode::Off, false),
            "helper_hold" => fake.helper.ends(&run, "paused", "external_input"),
            "wrong_plan" => fake.helper.plan_hash = Some("another"),
            "stream" | "geometry" | "owner" | "plan" => fake.helper.scene[case] = json!(9),
            "healthy" => {}
            _ => panic!("unknown case"),
        }
        if case != "healthy" {
            fake.with(Some(&mut generator), |world| pilot.tick(world));
        }
        progress.send(p).expect("the test reads its progress");
        finishing
            .recv_timeout(Duration::from_secs(5))
            .expect("finish");
        for _ in 0..400 {
            fake.with(Some(&mut generator), |world| pilot.tick(world));
            if fake.helper.runs.len() == 2 || pilot.ended().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let result = (
            fake.helper.runs.len(),
            fake.helper.stops().len(),
            pilot.ended().cloned(),
            fake.helper
                .held
                .values()
                .map(|held| held.policy["run_ns"].as_u64().unwrap_or(0))
                .min()
                .unwrap_or(0),
            fake.helper
                .held
                .values()
                .any(|held| held.state == "running"),
        );
        if pilot.ended().is_none() {
            fake.stopped = Some("hotkey".into());
            fake.with(Some(&mut generator), |world| pilot.tick(world));
        }
        result
    });
    let entered = started.recv_timeout(Duration::from_secs(3));
    let progress = collected.recv_timeout(Duration::from_secs(2));
    let _ = release.send(());
    let _ = finish.send(());
    let result = driver.join().expect("the driver finished");
    for _ in 0..2 {
        drained
            .recv_timeout(Duration::from_secs(3))
            .expect("both writers have ended");
    }
    entered.expect("the re-plan actually asked its model");
    let progress = progress
        .expect("collect and fast judgment must progress while the plan's model stays blocked");
    assert_eq!(progress.passes, 16, "one fresh helper read each collect");
    assert_eq!(
        progress.stops, 0,
        "the valid old run stays live while planning"
    );
    assert!(progress.questions > 0, "the fast judgment keeps asking");
    assert_eq!(
        progress.capture_age_ns, 3_000_000,
        "the fake's fresh-frame age is unchanged"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "one initial request and one re-plan; no duplicate or cancelled retry"
    );
    assert!(budgets.lock().expect("budgets")[1] <= Duration::from_secs(60));
    if case == "healthy" {
        assert_eq!(
            (result.0, result.1),
            (2, 1),
            "replace only after the answer is ready"
        );
        assert!(result.2.is_none());
        assert!(
            result.3 < 60_000_000_000,
            "the new run gets only the remaining wall"
        );
    } else {
        assert_eq!(
            result.0, 1,
            "a late answer must not start a run after {case}"
        );
        assert!(result.2.is_some(), "{case} ends the autopilot");
        assert!(!result.4, "{case} leaves no old run accepting input");
    }
}

/// The person's setting is read again at the door: turned off since the
/// facts were read, it refuses before the helper, the screen or the
/// generator is asked.
#[test]
fn the_current_live_setting_wins_over_startup_facts() {
    let mut fake = Fake::new((JevMode::Auto, true), vec![good()]);
    fake.enabled = false;
    let result = fake.with(None, |world| {
        Autopilot::start(asked(None), open(), None, world)
    });
    let refusal = result.err().expect("the current setting closes the door");
    assert_eq!(refusal.code, error_code::UNSUPPORTED_CAPABILITY);
    assert!(fake.helper.calls.is_empty());
    assert!(fake.generator.asked.is_empty());
}

/// While the re-plan's model is held the valid run is read every collect,
/// judged, and never stopped; its answer then replaces the run once, for
/// what is left of the wall.
#[test]
fn a_blocked_replan_keeps_collecting_and_judging_before_one_handoff() {
    held_replan("healthy");
}

/// A stop — the operator's or the owner's — the setting turned off, the
/// wall, a seat that no longer applies or the helper's own hold each end
/// the autopilot while a plan is being written, and its late answer starts
/// nothing.
#[test]
fn a_pending_plan_never_outlives_stop_permission_or_deadline() {
    for case in [
        "stop",
        "owner_stop",
        "disabled",
        "deadline",
        "jev_off",
        "helper_hold",
    ] {
        held_replan(case);
    }
}

/// A plan asked on one scene is never run on another: a scene the reading
/// no longer shows, a changed stream, geometry, owner or plan, or a helper
/// running another plan ends the autopilot, and the late answer starts
/// nothing.
#[test]
fn a_pending_plan_cannot_use_changed_execution_context() {
    for case in [
        "missing_scene",
        "stream",
        "geometry",
        "owner",
        "plan",
        "wrong_plan",
    ] {
        held_replan(case);
    }
}

/// How long the held model takes to answer when nobody releases it: a
/// second — long enough for a collector that waits on it to show that it
/// waited, short enough for a test.
const LATE_MODEL: Duration = Duration::from_secs(1);

/// A reading the window cannot date, or one older than the table lets a
/// decision act on, holds the plan waiting to be applied: nothing of the
/// plan reaches the hand while its premise is unproven, the run stands and
/// keeps being read, and the plan starts once a fresh reading comes — the
/// autopilot does not end for a frame the stream was late with.
#[test]
fn a_stale_reading_holds_the_pending_plan_until_a_fresh_one() {
    for case in ["stale", "missing_capture"] {
        let (entered, started) = mpsc::channel();
        let (collecting, collecting_started) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (finished, drained) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let delayed = Delayed {
            calls: Arc::clone(&calls),
            entered,
            collecting,
            finished,
            gate: Arc::new(Mutex::new(gate)),
            answer: good().expect("a plan"),
            budgets: Arc::new(Mutex::new(Vec::new())),
        };
        // The model answers late, whether or not the collector waits on it.
        std::thread::spawn(move || {
            std::thread::sleep(LATE_MODEL);
            let _ = release.send(());
        });
        let mut fake = Fake::new((JevMode::Auto, true), Vec::new());
        fake.teacher.says(REPLAN);
        let first = delayed.clone();
        let mut generator = plan::Background::new(
            Box::new(first),
            Box::new(move || Box::new(delayed) as Box<dyn Generator>),
        );
        let (mut pilot, answer) = fake
            .with(Some(&mut generator), |world| {
                Autopilot::start(asked(None), open(), None, world)
            })
            .expect("started");
        let run = answer["runId"].as_str().expect("a run").to_string();
        for _ in 0..400 {
            fake.with(Some(&mut generator), |world| pilot.tick(world));
            if !fake.asked_rows().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        started
            .recv_timeout(Duration::from_secs(3))
            .expect("the re-plan asked its model");
        collecting_started
            .recv_timeout(Duration::from_secs(2))
            .expect("the collector heard the model");
        let fresh_age_ns = fake.helper.age_ns;
        match case {
            "stale" => {
                fake.helper.age_ns =
                    (zerocode_core::computer_use::REFLEX_APPLY_MAX_AGE_MS + 1) * 1_000_000;
            }
            "missing_capture" => fake.helper.capture_known = false,
            _ => unreachable!("the cases above"),
        }
        // The answer comes while the reading is stale: collects go on, the
        // run stands, nothing starts.
        let before = fake.helper.calls.len();
        let began = Instant::now();
        while began.elapsed() < LATE_MODEL + Duration::from_millis(500) {
            fake.now += 10;
            fake.with(Some(&mut generator), |world| pilot.tick(world));
            std::thread::sleep(Duration::from_millis(10));
        }
        let passes = fake.helper.calls[before..]
            .iter()
            .filter(|(method, _)| method == "reflexReceipts")
            .count();
        assert!(passes > 0, "{case}: the run keeps being read");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "{case}: one re-plan asked");
        assert_eq!(pilot.ended(), None, "{case}: a stale reading ends nothing");
        assert_eq!(
            fake.helper.runs.len(),
            1,
            "{case}: the plan waits for a reading it can trust"
        );
        assert!(fake.helper.stops().is_empty(), "{case}: the run stands");
        assert_eq!(pilot.rendered()["planning"], json!(true), "{case}");
        // A fresh reading: the plan starts on it, the old run stopped first.
        match case {
            "stale" => fake.helper.age_ns = fresh_age_ns,
            "missing_capture" => fake.helper.capture_known = true,
            _ => unreachable!("the cases above"),
        }
        fake.now += 10;
        fake.with(Some(&mut generator), |world| pilot.tick(world));
        assert_eq!(
            fake.helper.runs.len(),
            2,
            "{case}: the plan starts on a fresh reading"
        );
        assert_eq!(fake.helper.stops(), std::slice::from_ref(&run), "{case}");
        assert_eq!(pilot.ended(), None, "{case}");
        assert_eq!(pilot.rendered()["planning"], json!(false), "{case}");
        fake.stopped = Some("hotkey".into());
        fake.with(Some(&mut generator), |world| pilot.tick(world));
        drop(generator);
        for _ in 0..2 {
            drained
                .recv_timeout(Duration::from_secs(3))
                .expect("both writers have ended");
        }
    }
}

/// What the measurement's model remembers of its re-plan: when it was asked
/// and when it answered, how many questions the teacher had heard at each,
/// and whether it was asked on the collector's own thread.
#[derive(Default)]
struct Thought {
    entered: Option<Instant>,
    answered: Option<Instant>,
    heard_at_entry: usize,
    heard_at_answer: usize,
    on_collector: bool,
}

/// A model that thinks for a fixed time on its re-plan, nobody holding it,
/// and remembers where it was asked from.
#[derive(Clone)]
struct Thinking {
    calls: Arc<AtomicUsize>,
    hold: Duration,
    heard: Arc<AtomicUsize>,
    collector: std::thread::ThreadId,
    thought: Arc<Mutex<Thought>>,
}

impl Generator for Thinking {
    fn unready(&self) -> Option<String> {
        None
    }

    fn model(&self) -> Option<String> {
        Some("thinking-model-test".into())
    }

    fn ask(
        &mut self,
        system: &str,
        user: &str,
        left: Duration,
    ) -> Result<crate::computer_use::errand::value::Said, String> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            {
                let mut thought = self.thought.lock().expect("the thought");
                thought.entered = Some(Instant::now());
                thought.heard_at_entry = self.heard.load(Ordering::SeqCst);
                thought.on_collector = std::thread::current().id() == self.collector;
            }
            std::thread::sleep(self.hold);
            let mut thought = self.thought.lock().expect("the thought");
            thought.answered = Some(Instant::now());
            thought.heard_at_answer = self.heard.load(Ordering::SeqCst);
        }
        Scripted {
            answers: vec![good()].into(),
            asked: Vec::new(),
        }
        .ask(system, user, left)
    }
}

/// Printed, for the report (t-21494 §5): how the collector fares while a
/// re-plan's model thinks, against a fake helper and a fake teacher, on a
/// clock the test moves — how long of the think the old run kept standing
/// (the hand has a run to evaluate only while one stands), the collects it
/// completes and the questions it asks during the think, its longest
/// collect, the time it itself waited on the model, and the share of its
/// collects made while a plan was pending.
/// No hand, no screen, no model: a leaf's first-input age and a press's
/// confirmed effect are the fixture round's to measure
/// (`tools/computer-bench/fixture_reflex.py`), never this fake's.
#[test]
#[ignore = "a measurement for the report"]
fn measure_the_collector_while_a_replan_is_written() {
    /// How long the model thinks: longer than many collects.
    const HOLD_MS: u64 = 1_500;
    const HOLD: Duration = Duration::from_millis(HOLD_MS);
    /// The wall between two collects here — the fake's clock moves as much.
    const TICK_MS: u64 = 25;
    const TICK: Duration = Duration::from_millis(TICK_MS);
    /// Collects after the answer, enough for the plan to be applied.
    const AFTER: Duration = Duration::from_millis(400);
    let calls = Arc::new(AtomicUsize::new(0));
    let thought = Arc::new(Mutex::new(Thought::default()));
    let mut fake = Fake::new((JevMode::Auto, true), Vec::new());
    fake.teacher.says(REPLAN);
    let thinking = Thinking {
        calls: Arc::clone(&calls),
        hold: HOLD,
        heard: Arc::clone(&fake.teacher.heard),
        collector: std::thread::current().id(),
        thought: Arc::clone(&thought),
    };
    let mut generator = plan::Background::new(
        Box::new(thinking.clone()),
        Box::new(move || Box::new(thinking) as Box<dyn Generator>),
    );
    let (mut pilot, _) = fake
        .with(Some(&mut generator), |world| {
            Autopilot::start(asked(None), open(), None, world)
        })
        .expect("started");
    let began = Instant::now();
    let mut collects: Vec<(Instant, Instant, bool)> = Vec::new();
    loop {
        fake.helper.moment += 1;
        fake.now += TICK_MS;
        let from = Instant::now();
        fake.with(Some(&mut generator), |world| pilot.tick(world));
        let planning = pilot.rendered()["planning"].as_bool().unwrap_or(false);
        collects.push((from, Instant::now(), planning));
        let answered = thought.lock().expect("the thought").answered;
        if answered.is_some_and(|at| at.elapsed() > AFTER) || began.elapsed() > HOLD * 4 {
            break;
        }
        std::thread::sleep(TICK);
    }
    let (entered, answered, questions, on_collector) = {
        let thought = thought.lock().expect("the thought");
        (
            thought.entered.expect("the re-plan asked its model"),
            thought.answered.expect("the model answered"),
            thought.heard_at_answer - thought.heard_at_entry,
            thought.on_collector,
        )
    };
    let during = collects
        .iter()
        .filter(|(from, to, _)| *from >= entered && *to <= answered)
        .count();
    let longest_ms = collects
        .iter()
        .map(|(from, to, _)| to.duration_since(*from).as_secs_f64() * 1_000.0)
        .fold(0.0_f64, f64::max);
    let while_planning = collects.iter().filter(|(_, _, planning)| *planning).count();
    let think_s = answered.duration_since(entered).as_secs_f64();
    // The old run stands until its stop: before the model is asked when the
    // plan is written on the collector, with the handoff when it is not.
    let stopped_at = fake
        .helper
        .calls
        .iter()
        .zip(&fake.helper.called_at)
        .find(|((method, _), _)| method == "reflexStop")
        .map_or(answered, |(_, at)| (*at).clamp(entered, answered));
    let stood_s = stopped_at.duration_since(entered).as_secs_f64();
    println!(
        "{}",
        json!({
            "basis": "fake helper, fake teacher, a model that sleeps: no input, no frame, no live model",
            "holdMs": HOLD_MS, "tickMs": TICK_MS,
            "modelThinkMs": think_s * 1_000.0,
            "runStandingDuringModelMs": stood_s * 1_000.0,
            "runStandingShare": stood_s / think_s,
            "evaluation": {
                "collectsDuringModel": during,
                "hzDuringModel": during as f64 / think_s,
                "longestCollectMs": longest_ms,
                "collects": collects.len(),
            },
            "questionsDuringModel": questions,
            "modelWaitOnCollectorMs": if on_collector { think_s * 1_000.0 } else { 0.0 },
            "evaluationsWhilePlanningShare": while_planning as f64 / collects.len() as f64,
            "firstInputAgeP95Ms": Value::Null,
            "unconfirmedCoverage": Value::Null,
            "runs": fake.helper.runs.len(),
            "realInput": 0, "liveModelCalls": 0,
        })
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "one initial plan, one re-plan"
    );
    assert_eq!(fake.helper.runs.len(), 2, "the re-plan ran");
    fake.stopped = Some("hotkey".into());
    fake.with(Some(&mut generator), |world| pilot.tick(world));
}
