//! The autopilot while a re-plan is written (t-21494): the model that writes
//! the next plan is held by the test, and the valid run standing meanwhile
//! must keep being read and judged; only an answer whose premises still
//! stand replaces that run, under the same door every plan passes.

use std::time::Instant;

use super::*;

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

struct Progress {
    passes: usize,
    stops: usize,
    questions: usize,
    capture_age_ns: u64,
}

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

#[test]
fn a_blocked_replan_keeps_collecting_and_judging_before_one_handoff() {
    held_replan("healthy");
}

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
        assert_eq!(fake.helper.stops(), [run.clone()], "{case}");
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
