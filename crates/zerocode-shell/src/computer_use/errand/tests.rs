//! What a walk by judgment may do, and what it may not.

use std::collections::BTreeMap;
use std::time::Duration;

use super::*;
use zerocode_core::computer_flow::{Confirm, EvidenceLevel, Fingerprint, Money};
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
use zerocode_core::jev::summary::MODEL;
use zerocode_core::screen_action::{DONE, GIVE_UP};

/// One control the page is showing.
fn control(mark: usize, label: &str) -> Value {
    json!({
        "mark": mark,
        "role": "button",
        "label": label,
        "centerX": 100.0 + mark as f64,
        "centerY": 40.0,
    })
}

/// A judge that answers what the test says, and counts what it was asked —
/// presses, comparisons (t-6044), and the questions begun ahead of the walk
/// (t-6132).
pub(super) struct FakeJudge {
    pub(super) answers: Vec<Judged>,
    pub(super) asked: Vec<Vec<usize>>,
    /// The comparisons it answers, in order; empty refuses.
    pub(super) compares: Vec<Compared>,
    /// The comparisons it was asked: the candidates offered, in order.
    pub(super) compared: Vec<Vec<usize>>,
    /// The state of the last comparison it was asked, as the wire would
    /// carry it.
    pub(super) compared_state: Vec<Value>,
    /// The questions it was asked ahead of the walk ([`ActionJudge::begin`]),
    /// answered from the same list on a thread of their own.
    pub(super) begun: Vec<Vec<usize>>,
    /// The state of each question begun ahead, as the wire would carry it.
    pub(super) begun_state: Vec<Value>,
    /// Whether it answers from the judgment memo ([`ActionJudge::cached`]),
    /// in turn or ahead — a test's stand-in for the cache seat's hit.
    pub(super) cached: bool,
    /// Whether it asks ahead at all — a judge that cannot answers `None`.
    overlaps: bool,
    /// How long one answer takes, in turn or ahead.
    latency: Duration,
    /// What [`ActionJudge::finish`] handed back.
    finished: usize,
}

impl FakeJudge {
    pub(super) fn chose(marks: &[usize]) -> Self {
        Self::saying(marks.iter().map(|mark| pick(*mark)).collect())
    }
    fn saying(answers: Vec<Judged>) -> Self {
        Self {
            answers,
            asked: Vec::new(),
            compares: Vec::new(),
            compared: Vec::new(),
            compared_state: Vec::new(),
            begun: Vec::new(),
            begun_state: Vec::new(),
            cached: false,
            overlaps: true,
            latency: Duration::ZERO,
            finished: 0,
        }
    }
    /// The same judge, each answer taking `ms`.
    fn slow(mut self, ms: u64) -> Self {
        self.latency = Duration::from_millis(ms);
        self
    }
    fn next_answer(&mut self) -> Judged {
        if self.answers.is_empty() {
            Judged::Refused("timeout".to_string())
        } else {
            self.answers.remove(0)
        }
    }
}

/// A validated choice of `mark`, as the pure module would have read one.
pub(super) fn pick(mark: usize) -> Judged {
    Judged::Chose(ActionChoice {
        chosen: Chosen::Mark(mark),
        probabilities: BTreeMap::new(),
        confidence: 0.7,
        guard: None,
    })
}

impl ActionJudge for FakeJudge {
    fn choose(&mut self, ask: &ActionAsk) -> Judged {
        self.asked.push(ask.marks().to_vec());
        std::thread::sleep(self.latency);
        self.next_answer()
    }

    fn begin(&mut self, ask: &ActionAsk) -> Option<Pending> {
        if !self.overlaps {
            return None;
        }
        self.begun.push(ask.marks().to_vec());
        self.begun_state.push(ask.state.clone());
        let judged = self.next_answer();
        let latency = self.latency;
        let cached = self.cached;
        let (done, waited) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(latency);
            let _ = done.send(Done {
                judged,
                spent: None,
                cached,
                rows: Vec::new(),
            });
        });
        Some(Pending::new(ask.clone(), waited))
    }

    fn finish(&mut self, done: Done) -> Judged {
        self.finished += 1;
        done.judged
    }

    fn cached(&self) -> bool {
        self.cached
    }

    fn compare(&mut self, ask: &zerocode_core::branching::BranchAsk) -> Compared {
        self.compared.push(ask.marks().to_vec());
        self.compared_state.push(ask.state.clone());
        if self.compares.is_empty() {
            Compared::Refused("timeout".to_string())
        } else {
            self.compares.remove(0)
        }
    }
}

/// A world that answers what the test says and remembers what was done to it —
/// shared with the wire's tests, which put the live judge in front of it.
pub(super) struct FakeWorld {
    screen: Option<Screen>,
    pub(super) presses: Vec<usize>,
    pub(super) observed: Vec<Option<Value>>,
    press_takes: bool,
    walked_from: Vec<usize>,
    /// The step each re-walk says it stopped at; `None` means it finished.
    walks: Vec<Option<usize>>,
    pub(super) left_ms: u64,
    /// Whether every press renumbers the screen — a world that moves.
    moves: bool,
    /// Whether only every second press moves the screen — a world where a
    /// press lands on the screen it was pressed on half the time, which is
    /// where a judgment begun on the last look can be used.
    moves_every_other: bool,
    /// How long a press holds the walk — the door's own landing, which a
    /// judgment begun ahead runs behind.
    press_holds: Duration,
    /// What the caller's own condition answers, press by press; a world whose
    /// caller wrote no condition has none.
    pub(super) reached: Vec<bool>,
    /// The device's saved states (t-6044): `None` is a world that cannot
    /// save — a page, an iOS simulator. `Some(cost)` saves in `cost` ms of
    /// the world's own clock.
    pub(super) snapshots: Option<u64>,
    /// What each press leads to, by mark: the screen's items after it. A
    /// mark not named here leaves the screen as it was (or, for a world
    /// that moves, renumbers it as before).
    pub(super) leads_to: BTreeMap<usize, Vec<Value>>,
    /// Whether a load takes; a world whose load fails stays where the last
    /// press put it.
    pub(super) restore_takes: bool,
    /// Every save, load and forget, in order: `("save", name)` and so on.
    pub(super) snapshot_log: Vec<(&'static str, String)>,
    /// The screen the last save kept, restored on load.
    saved_screen: Option<Screen>,
    /// What one press and one look cost, in ms of the world's own clock.
    pub(super) press_ms: u64,
    pub(super) look_ms: u64,
    /// How much of `left_ms` the world has spent — the clock a test reads.
    pub(super) spent_ms: u64,
    /// What each press says of its screen settling (t-6385): `None` is a
    /// world whose press does not wait for that — a page, the desktop.
    pub(super) settles: Option<Value>,
    /// Whether a settled press hands back the screen it stopped on — a
    /// phone's press asked for a preview.
    pub(super) settles_on_screen: bool,
    /// What that preview shows instead of the screen, when it disagrees with
    /// the full look (an element only the grid finds).
    pub(super) preview_items: Option<Vec<Value>>,
    /// Whether a judgment begun before a press can stand for the next look's
    /// question here — `false` for a phone's world.
    pub(super) asks_before_press: bool,
    /// How long one look holds the walk — what a judgment begun on a settled
    /// screen runs behind.
    pub(super) look_holds: Duration,
}

impl FakeWorld {
    pub(super) fn showing(marks: &[usize]) -> Self {
        Self {
            screen: Some(Screen {
                shows: Vec::new(),
                at: Seen::Page {
                    host: "app.local".into(),
                    path: "/settings".into(),
                },
                items: marks.iter().map(|mark| control(*mark, "저장")).collect(),
            }),
            presses: Vec::new(),
            observed: Vec::new(),
            press_takes: true,
            walked_from: Vec::new(),
            walks: vec![None],
            left_ms: 60_000,
            moves: false,
            moves_every_other: false,
            press_holds: Duration::ZERO,
            reached: Vec::new(),
            snapshots: None,
            leads_to: BTreeMap::new(),
            restore_takes: true,
            snapshot_log: Vec::new(),
            saved_screen: None,
            press_ms: 0,
            look_ms: 0,
            spent_ms: 0,
            settles: None,
            settles_on_screen: false,
            preview_items: None,
            asks_before_press: true,
            look_holds: Duration::ZERO,
        }
    }

    /// The same world as an Android phone that can be saved and loaded
    /// (t-6044), showing `marks`.
    pub(super) fn android(marks: &[usize]) -> Self {
        let mut world = Self::showing(marks);
        if let Some(screen) = world.screen.as_mut() {
            screen.at = Seen::Phone {
                platform: zerocode_core::computer_use::EmulatorPlatform::Android,
                device: "Pixel_6".into(),
            };
        }
        world.snapshots = Some(0);
        world
    }

    fn spend(&mut self, ms: u64) {
        self.spent_ms = self.spent_ms.saturating_add(ms);
        self.left_ms = self.left_ms.saturating_sub(ms);
    }

    /// The same world, whose screen shows different words after every press.
    pub(super) fn that_moves(marks: &[usize]) -> Self {
        Self {
            moves: true,
            ..Self::showing(marks)
        }
    }

    /// The same world, whose screen moves after every second press only.
    fn that_moves_every_other(marks: &[usize]) -> Self {
        Self {
            moves_every_other: true,
            ..Self::showing(marks)
        }
    }

    /// The same world, each press holding the walk `ms`.
    fn holding(mut self, ms: u64) -> Self {
        self.press_holds = Duration::from_millis(ms);
        self
    }

    /// Stand the world on `screen` — a fixture's, say (t-6187).
    pub(super) fn screen_is(&mut self, screen: Screen) {
        self.screen = Some(screen);
    }

    /// The screen the world stands on now, for a test to change a piece of.
    pub(super) fn look_now(&self) -> Screen {
        self.screen.clone().expect("a world that shows a screen")
    }
}

impl World for FakeWorld {
    fn look(&mut self) -> Option<Screen> {
        self.spend(self.look_ms);
        std::thread::sleep(self.look_holds);
        self.screen.clone()
    }
    fn press(&mut self, mark: usize) -> bool {
        self.observed.push(crate::run_evidence::observation());
        self.presses.push(mark);
        // Time first — the world's own clock, then the door's landing — then
        // whether the screen moved, then where the press led.
        self.spend(self.press_ms);
        std::thread::sleep(self.press_holds);
        let round = self.presses.len();
        let moves = self.moves || (self.moves_every_other && round.is_multiple_of(2));
        if self.press_takes
            && moves
            && let Some(screen) = self.screen.as_mut()
        {
            // A screen that moved: same numbers, different words.
            for item in &mut screen.items {
                item["label"] = json!(format!("저장 {round}"));
            }
        }
        if self.press_takes
            && let Some(leads_to) = self.leads_to.get(&mark)
            && let Some(screen) = self.screen.as_mut()
        {
            screen.items.clone_from(leads_to);
            return true;
        }
        self.press_takes
    }

    fn save(&mut self) -> Option<Saved> {
        let cost = self.snapshots?;
        self.spend(cost);
        let name = format!("fake-{}", self.snapshot_log.len());
        self.snapshot_log.push(("save", name.clone()));
        self.saved_screen = self.screen.clone();
        Some(Saved {
            name,
            took_ms: cost,
        })
    }

    fn restore(&mut self, saved: &Saved) -> bool {
        self.snapshot_log.push(("load", saved.name.clone()));
        if let Some(cost) = self.snapshots {
            self.spend(cost);
        }
        if !self.restore_takes {
            return false;
        }
        self.screen = self.saved_screen.clone();
        true
    }

    fn forget(&mut self, saved: &Saved) {
        self.snapshot_log.push(("forget", saved.name.clone()));
    }

    fn reached(&mut self) -> Option<bool> {
        if self.reached.is_empty() {
            return None;
        }
        Some(self.reached.remove(0))
    }
    fn settled(&mut self) -> Option<Settled> {
        let note = self.settles.clone()?;
        let screen = self
            .settles_on_screen
            .then(|| self.screen.clone())
            .flatten()
            .map(|mut screen| {
                if let Some(items) = &self.preview_items {
                    screen.items.clone_from(items);
                }
                screen
            });
        Some(Settled { note, screen })
    }
    fn asks_ahead_of_the_press(&self) -> bool {
        self.asks_before_press
    }
    fn walk_from(&mut self, step: usize) -> Option<Value> {
        self.walked_from.push(step);
        let stopped = if self.walks.is_empty() {
            None
        } else {
            self.walks.remove(0)
        };
        Some(match stopped {
            None => json!({ "done": true, "stoppedAt": Value::Null }),
            Some(at) => json!({ "done": false, "stoppedAt": at }),
        })
    }
    fn left_ms(&mut self) -> u64 {
        self.left_ms
    }
}

pub(super) fn stopped(stop: RecipeStop) -> Errand<'static> {
    Errand {
        goal: "settings smoke",
        why: Why::Cleared {
            stop,
            step: "click",
            refusal: "the selector matched nothing",
            next: 4,
        },
        flow: None,
        moves_money: false,
    }
}

/// A goal walk of `steps` presses, with nothing failed.
pub(super) fn goal(steps: usize) -> Errand<'static> {
    Errand {
        goal: "채팅방 열기",
        why: Why::Goal { steps },
        flow: None,
        moves_money: false,
    }
}

#[test]
fn mobile_and_other_surfaces_refuse_low_confidence() {
    for seen in [
        Seen::default(),
        Seen::Desk {
            app: "Settings".into(),
            window: "General".into(),
        },
        Seen::Phone {
            platform: zerocode_core::computer_use::EmulatorPlatform::Ios,
            device: "phone".into(),
        },
    ] {
        let mut judge = FakeJudge::chose(&[1]);
        let Judged::Chose(choice) = &mut judge.answers[0] else {
            unreachable!()
        };
        choice.confidence = 0.29;
        let mut world = FakeWorld::showing(&[1]);
        world.screen.as_mut().unwrap().at = seen;
        let walked = run(Mode::On, true, &goal(1), &mut judge, &mut world);
        assert!(world.presses.is_empty(), "low confidence must not press");
        assert_eq!(walked.pressed, 0);
        assert_eq!(walked.rows[0]["barred"], Barred::LowConfidence.as_str());
        assert_eq!(walked.rows[0]["confidence"], 0.29);
    }
}

#[test]
fn off_asks_nothing_presses_nothing_and_writes_nothing() {
    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[1, 2]);

    let recovered = run(
        Mode::Off,
        false,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert!(
        recovered.rows.is_empty(),
        "off is the default and says nothing"
    );
    assert!(recovered.report.is_none());
    assert!(judge.asked.is_empty());
    assert!(world.presses.is_empty() && world.walked_from.is_empty());
}

#[test]
fn shadow_records_what_it_would_have_pressed_and_presses_nothing() {
    // `auto` records until its own ledger promotes it, so an `auto` nobody
    // has raised is held to every word `shadow` is.
    for mode in [Mode::Shadow, Mode::Auto] {
        let mut judge = FakeJudge::chose(&[2]);
        let mut world = FakeWorld::showing(&[1, 2]);

        let recovered = run(
            mode,
            mode.applies(),
            &stopped(RecipeStop::CheckFailed),
            &mut judge,
            &mut world,
        );

        assert_eq!(judge.asked, [vec![1, 2]], "{mode:?}");
        assert_eq!(recovered.rows.len(), 1, "{mode:?}");
        assert_eq!(recovered.rows[0]["chosen"], json!("mark:2"), "{mode:?}");
        assert_eq!(recovered.rows[0]["routeUse"], json!(USE_SHADOW), "{mode:?}");
        assert!(
            world.presses.is_empty() && world.walked_from.is_empty(),
            "{mode:?} leaves the walk exactly where it stopped"
        );
        assert!(recovered.report.is_none(), "{mode:?}");
    }
}

#[test]
fn a_recording_seat_says_on_the_row_and_in_one_word_why_it_pressed_nothing() {
    // A walk under a seat that only records looked, from the outside, exactly
    // like a walk that found nothing worth pressing: `pressed: 0`, no error,
    // and a `routeUse` nobody reads as an explanation. Thirteen walks of the
    // v1.1.3 measurement went that way, every one of them judged at 0.95 or
    // better, before the seat itself was looked at (t-5455).
    for (mode, why) in [
        (Mode::Shadow, stopped(RecipeStop::CheckFailed)),
        (Mode::Auto, goal(1)),
    ] {
        let mut judge = FakeJudge::chose(&[2]);
        let mut world = FakeWorld::showing(&[1, 2]);

        let recorded = run(mode, false, &why, &mut judge, &mut world);

        let said = &recorded.rows[0];
        assert_eq!(said["routeUse"], json!(USE_SHADOW), "{mode:?}");
        assert_eq!(said[REASON], json!(SEAT_RECORDING), "{mode:?}");
        assert_eq!(said["pressed"], json!(false), "{mode:?}");
        assert_eq!(
            no_press_reason(&recorded.rows),
            Some(SEAT_RECORDING),
            "{mode:?}"
        );
    }

    // The word says something only because an acting walk does not carry it:
    // a reason on every row would be no reason at all.
    let mut judge = FakeJudge::chose(&[2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    let applied = run(Mode::On, true, &goal(1), &mut judge, &mut world);
    assert_eq!(applied.rows[0]["routeUse"], json!(USE_APPLIED));
    assert_eq!(no_press_reason(&applied.rows), None);
}

/// A judge that says what asking cost at the Jev door has it written on the
/// row, under the keys every Jev ledger spells; one that says nothing leaves
/// the row as it always was.
/// What asking cost at the door — and which version answered (t-6187) — is
/// on the row, through the one writer every seat's row goes through; a
/// judge that sent nowhere leaves all three off.
#[test]
fn what_asking_cost_at_the_door_is_on_the_row() {
    struct Spending(FakeJudge);
    impl ActionJudge for Spending {
        fn choose(&mut self, ask: &ActionAsk) -> Judged {
            self.0.choose(ask)
        }
        fn spent(&self) -> Option<Spent> {
            Some(Spent {
                requests: 1,
                redacted_lines: 2,
                model: Some("jev-1.13.0".to_string()),
            })
        }
    }
    let mut judge = Spending(FakeJudge::chose(&[2]));
    let mut world = FakeWorld::showing(&[1, 2]);

    let recovered = run(
        Mode::Shadow,
        false,
        &stopped(RecipeStop::CheckFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(recovered.rows[0][REQUESTS_KEY], json!(1));
    assert_eq!(recovered.rows[0][REDACTED_LINES_KEY], json!(2));
    assert_eq!(recovered.rows[0][MODEL.canonical], json!("jev-1.13.0"));

    let mut quiet = FakeJudge::chose(&[2]);
    let recovered = run(
        Mode::Shadow,
        false,
        &stopped(RecipeStop::CheckFailed),
        &mut quiet,
        &mut FakeWorld::showing(&[1, 2]),
    );
    assert!(recovered.rows[0].get(REQUESTS_KEY).is_none());
    assert!(recovered.rows[0].get(MODEL.canonical).is_none());
}

#[test]
fn on_presses_the_number_it_chose_and_walks_again_from_the_reports_own_next() {
    let mut judge = FakeJudge::chose(&[2]);
    let mut world = FakeWorld::showing(&[1, 2, 3]);

    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(world.presses, [2]);
    assert_eq!(
        world.walked_from,
        [4],
        "the resume point is the report's, not a new one"
    );
    let row = &recovered.rows[0];
    assert_eq!(row["routeUse"], json!(USE_APPLIED));
    assert_eq!(row["pressed"], json!(true));
    assert_eq!(row["resumedFrom"], json!(4));
    assert_eq!(row["recheck"], json!(true));
    assert!(recovered.report.is_some());
}

#[test]
fn pressing_is_not_succeeding() {
    let mut judge = FakeJudge::chose(&[1, 3]);
    let mut world = FakeWorld::showing(&[1, 2, 3]);
    // Both re-walks stop at the very step that stopped before.
    world.walks = vec![Some(4), Some(4)];

    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(world.presses, [1, 3]);
    assert!(
        recovered
            .rows
            .iter()
            .all(|row| row["recheck"] == json!(false)),
        "a walk that stopped at the same step again did not clear it: {:?}",
        recovered.rows
    );
    // The second question never offers the number the first one spent.
    assert_eq!(judge.asked, [vec![1, 2, 3], vec![2, 3]]);
}

#[test]
fn a_walk_spends_no_more_than_the_attempt_cap() {
    let mut judge = FakeJudge::chose(&[1, 2, 3, 4]);
    let mut world = FakeWorld::showing(&[1, 2, 3, 4]);
    world.walks = vec![Some(4), Some(4), Some(4), Some(4)];

    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(world.presses.len(), MAX_RECOVERY_ATTEMPTS);
    assert_eq!(recovered.rows.len(), MAX_RECOVERY_ATTEMPTS);
}

#[test]
fn a_door_that_refused_the_press_is_not_tried_again() {
    let mut judge = FakeJudge::chose(&[1, 2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    // A stale pin, a host outside the recording, a shut door: all of them.
    world.press_takes = false;

    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(world.presses, [1], "one refusal ends the recovery");
    assert!(
        world.walked_from.is_empty(),
        "nothing was walked after a refused press"
    );
    assert_eq!(recovered.rows[0]["pressed"], json!(false));
    assert_eq!(recovered.rows[0]["routeUse"], json!(USE_FALLBACK));
    assert!(recovered.report.is_none());
}

#[test]
fn an_unusable_answer_leaves_the_walk_where_it_stopped() {
    for token in ["no_key", "timeout", "schema", "unauthorized", "http_503"] {
        let mut judge = FakeJudge::saying(vec![Judged::Refused(token.to_string())]);
        let mut world = FakeWorld::showing(&[1, 2]);

        let recovered = run(
            Mode::On,
            true,
            &stopped(RecipeStop::StepFailed),
            &mut judge,
            &mut world,
        );

        assert!(world.presses.is_empty() && world.walked_from.is_empty());
        assert_eq!(recovered.rows[0]["outcome"], json!(token));
        assert_eq!(recovered.rows[0]["routeUse"], json!(USE_FALLBACK));
        assert!(recovered.report.is_none());
    }
}

#[test]
fn giving_up_presses_nothing() {
    let mut judge = FakeJudge::saying(vec![Judged::Chose(ActionChoice {
        chosen: Chosen::GiveUp,
        probabilities: BTreeMap::new(),
        confidence: 0.3,
        guard: None,
    })]);
    let mut world = FakeWorld::showing(&[1, 2]);

    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert!(world.presses.is_empty());
    assert_eq!(recovered.rows[0]["chosen"], json!(GIVE_UP));
    assert_eq!(recovered.rows[0]["routeUse"], json!(USE_FALLBACK));
}

#[test]
fn only_a_failed_step_or_check_is_ever_recovered() {
    for stop in RecipeStop::ALL {
        let recoverable = matches!(stop, RecipeStop::StepFailed | RecipeStop::CheckFailed);
        let mut judge = FakeJudge::chose(&[1]);
        let mut world = FakeWorld::showing(&[1]);

        run(Mode::On, true, &stopped(stop), &mut judge, &mut world);

        assert_eq!(
            judge.asked.is_empty(),
            !recoverable,
            "{} was asked about when it should not have been (or the reverse)",
            stop.as_str()
        );
        if !recoverable {
            assert!(world.presses.is_empty());
            assert_eq!(
                barred(Mode::On, &stopped(stop), 60_000),
                Some(Barred::NotRecoverable)
            );
        }
    }
}

#[test]
fn money_is_never_recovered() {
    let mut moneyed = stopped(RecipeStop::StepFailed);
    moneyed.moves_money = true;

    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[1, 2]);
    let recovered = run(Mode::On, true, &moneyed, &mut judge, &mut world);

    assert!(judge.asked.is_empty() && world.presses.is_empty());
    assert_eq!(
        recovered.rows[0]["barred"],
        json!(Barred::MovesMoney.as_str())
    );
    assert_eq!(barred(Mode::On, &moneyed, 60_000), Some(Barred::MovesMoney));
}

#[test]
fn a_guarded_flow_refuses_recovery_even_with_no_money_line_of_its_own() {
    // Built rather than parsed: the document grammar refuses `guarded`
    // WITHOUT a money line, so parsing could only ever reach the money gate.
    // The policy is its own bar, and this is the only way to prove it.
    let spec = FlowSpec {
        policy: Policy::Guarded,
        evidence: EvidenceLevel::default(),
        fingerprint: Fingerprint::default(),
        checks: Vec::new(),
        money: None,
        confirm: Confirm::default(),
        trigger: None,
    };
    let mut guarded = stopped(RecipeStop::StepFailed);
    guarded.flow = Some(&spec);

    assert_eq!(barred(Mode::On, &guarded, 60_000), Some(Barred::Guarded));

    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[1]);
    let recovered = run(Mode::On, true, &guarded, &mut judge, &mut world);
    assert!(judge.asked.is_empty() && world.presses.is_empty());
    assert_eq!(recovered.rows[0]["barred"], json!(Barred::Guarded.as_str()));

    // A Flow whose money line stands is barred by the money, whatever else.
    let moneyed = FlowSpec {
        policy: Policy::Dry,
        money: Some(Money {
            id: "txn".into(),
            amount: "total".into(),
            recipient: "who".into(),
            step: 1,
        }),
        ..spec
    };
    let mut dry_but_paying = stopped(RecipeStop::StepFailed);
    dry_but_paying.flow = Some(&moneyed);
    assert_eq!(
        barred(Mode::On, &dry_but_paying, 60_000),
        Some(Barred::MovesMoney)
    );
}

#[test]
fn a_walk_without_clock_enough_to_ask_and_still_walk_asks_nothing() {
    let deadline = u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    assert_eq!(
        barred(Mode::On, &stopped(RecipeStop::StepFailed), deadline),
        Some(Barred::NoBudget)
    );
    assert!(barred(Mode::On, &stopped(RecipeStop::StepFailed), deadline + 1).is_none());

    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[1]);
    world.left_ms = deadline;
    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );
    assert!(judge.asked.is_empty() && world.presses.is_empty());
    assert_eq!(
        recovered.rows[0]["barred"],
        json!(Barred::NoBudget.as_str())
    );
}

#[test]
fn a_screen_that_cannot_be_read_or_shows_nothing_presses_nothing() {
    let mut judge = FakeJudge::chose(&[1]);
    let mut blind = FakeWorld::showing(&[1]);
    blind.screen = None;
    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut blind,
    );
    assert_eq!(recovered.rows[0]["outcome"], json!("no_look"));
    assert!(judge.asked.is_empty() && blind.presses.is_empty());

    let mut judge = FakeJudge::chose(&[1]);
    let mut bare = FakeWorld::showing(&[]);
    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut bare,
    );
    assert_eq!(recovered.rows[0]["outcome"], json!("no_candidate"));
    assert!(judge.asked.is_empty() && bare.presses.is_empty());
}

#[test]
fn the_mode_word_is_read_the_way_the_router_card_reads_its_own() {
    let word = |word: &str| BROWSER.mode_of(Some(&json!(word)));
    assert_eq!(word("on"), Mode::On);
    assert_eq!(word("  Shadow "), Mode::Shadow);
    assert_eq!(word("off"), Mode::Off);
    assert_eq!(word("AUTO"), Mode::Auto);
    // A typo never starts sending a screen anywhere.
    assert_eq!(word("onn"), Mode::Off);
    assert_eq!(word(""), Mode::Off);
    assert_eq!(BROWSER.mode_of(None), Mode::Off);
    for mode in BROWSER.modes {
        assert_eq!(word(mode.key()), *mode);
    }
    // Read with nothing raised, `auto` records; read with the judge's own
    // answer, it is that answer. The walk is handed the second reading.
    assert!(!Mode::Auto.applies());
    assert!(Mode::Auto.applies_with(true));
    assert!(
        !Mode::Shadow.applies_with(true),
        "a person's shadow is theirs"
    );
}

/// One recipe line as the parser would have read it.
fn line(step: usize, tool: RecipeTool, argv: &[&str]) -> RecipeLine {
    RecipeLine {
        step,
        shown: step,
        tool,
        argv: argv.iter().map(|word| (*word).to_string()).collect(),
        failed_then: false,
        money: false,
    }
}

/// A report of a walk that stopped at `at`, as `recipe_run::report` writes one.
fn stopped_report(kind: &str, at: usize, next: usize) -> Value {
    json!({
        "done": false,
        "stoppedAt": at,
        "next": next,
        "stop": { "kind": kind, "message": "the selector matched nothing" },
    })
}

#[test]
fn a_report_reads_as_the_stop_the_step_and_the_pane_it_aimed_at() {
    let lines = [
        line(
            1,
            RecipeTool::Browser,
            &["goto", "app", "https://app.local"],
        ),
        line(2, RecipeTool::Browser, &["click", "app", "#save"]),
    ];

    let read = read_report(&stopped_report("step_failed", 2, 2), &lines).expect("a stopped walk");

    assert_eq!(read.stop, RecipeStop::StepFailed);
    assert_eq!((read.at, read.next), (2, 2));
    assert_eq!(read.step, "click");
    assert_eq!(read.pane, "app");
    assert_eq!(read.refusal, "the selector matched nothing");
}

#[test]
fn a_type_lines_argv_never_reaches_the_read() {
    let lines = [line(
        1,
        RecipeTool::Browser,
        &["type", "app", "#password", "hunter2"],
    )];

    let read = read_report(&stopped_report("step_failed", 1, 1), &lines).expect("a stopped walk");

    assert_eq!(read.step, "type", "the verb alone");
    assert_eq!(read.pane, "app");
    let whole = format!("{read:?}");
    assert!(
        !whole.contains("hunter2") && !whole.contains("#password"),
        "what a person typed is not state: {whole}"
    );
}

#[test]
fn there_is_nothing_to_read_in_a_walk_that_finished_or_did_not_press_a_page() {
    let browser = [line(1, RecipeTool::Browser, &["click", "app", "#save"])];

    // It finished.
    let mut done = stopped_report("step_failed", 1, 1);
    done["done"] = json!(true);
    assert!(read_report(&done, &browser).is_none());

    // It names no stop, or no resume point.
    let mut no_stop = stopped_report("step_failed", 1, 1);
    no_stop["stop"] = Value::Null;
    assert!(read_report(&no_stop, &browser).is_none());
    let mut no_next = stopped_report("step_failed", 1, 1);
    no_next["next"] = Value::Null;
    assert!(read_report(&no_next, &browser).is_none());

    // A word no stop is named by.
    assert!(read_report(&stopped_report("moon_phase", 1, 1), &browser).is_none());

    // The stopped step was not the browser's: numbering a page is that door's.
    let desktop = [line(1, RecipeTool::Computer, &["click", "Finder", "Save"])];
    assert!(read_report(&stopped_report("step_failed", 1, 1), &desktop).is_none());

    // No line answers to the number the report stopped at.
    assert!(read_report(&stopped_report("step_failed", 9, 9), &browser).is_none());
}

#[test]
fn every_stop_word_a_report_can_write_reads_back() {
    for stop in RecipeStop::ALL {
        assert_eq!(RecipeStop::from_word(stop.as_str()), Some(stop));
    }
    assert_eq!(RecipeStop::from_word("not_a_stop"), None);
}

// ---- the goal errand ------------------------------------------------------

#[test]
fn a_goal_walk_presses_step_after_step_and_never_resumes_a_document() {
    let mut judge = FakeJudge::chose(&[1, 2, 3]);
    let mut world = FakeWorld::that_moves(&[1, 2, 3]);

    let walked = run(Mode::On, true, &goal(3), &mut judge, &mut world);

    assert_eq!(world.presses, [1, 2, 3]);
    assert!(
        world.walked_from.is_empty(),
        "a goal walk has no document to resume"
    );
    assert_eq!(walked.pressed, 3);
    assert_eq!(walked.rows.len(), 3);
    for (n, row) in walked.rows.iter().enumerate() {
        assert_eq!(row["errand"], json!("goal"));
        assert_eq!(row["attempt"], json!(n + 1));
        assert_eq!(row["routeUse"], json!(USE_APPLIED));
        assert!(row.get("resumedFrom").is_none());
        assert!(row.get("stop").is_none(), "nothing stopped");
    }
    // Nothing said it worked, so nothing claims it did.
    assert_eq!(walked.reached, Some(false));
}

/// A press whose world waited for its screen to stop changing (a phone's,
/// t-6385) says on its row how that went, under the emulator door's own
/// word; a world whose press does not wait says nothing of it.
#[test]
fn a_press_that_waited_for_its_screen_says_how_it_settled_on_its_row() {
    let settled = json!({ "ms": 1_080, "reads": 18, "settle": "still" });
    let mut judge = FakeJudge::chose(&[1, 2]);
    let mut world = FakeWorld::that_moves(&[1, 2]);
    world.settles = Some(settled.clone());
    world.reached = vec![false, true];
    let walked = run(Mode::On, true, &goal(3), &mut judge, &mut world);
    assert_eq!(walked.pressed, 2);
    for row in &walked.rows {
        assert_eq!(row[SETTLE], settled);
    }
    assert_eq!(SETTLE, zerocode_core::agent_emulator::EMULATOR_SETTLE_KEY);

    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::that_moves(&[1]);
    world.reached = vec![true];
    let walked = run(Mode::On, true, &goal(3), &mut judge, &mut world);
    assert!(walked.rows[0].get(SETTLE).is_none());
}

#[test]
fn the_callers_own_condition_ends_a_goal_walk_and_the_judgments_word_is_the_weaker_one() {
    // The condition the caller wrote down: checked on the screen, and the
    // walk stops the moment it holds.
    let mut judge = FakeJudge::chose(&[1, 2, 3]);
    let mut world = FakeWorld::that_moves(&[1, 2, 3]);
    world.reached = vec![false, true];

    let walked = run(Mode::On, true, &goal(10), &mut judge, &mut world);

    assert_eq!(world.presses, [1, 2], "it stopped when the check held");
    assert_eq!(walked.reached, Some(true));
    assert_eq!(walked.rows[0]["recheck"], json!(false));
    assert_eq!(walked.rows[1]["recheck"], json!(true));
    assert!(
        walked.rows[1].get("reachedBy").is_none(),
        "a checked end is not a judgment's word"
    );

    // No condition written down: only the judgment's own `done` can end it,
    // and the row says which of the two ends it was.
    let mut judge = FakeJudge::saying(vec![
        pick(1),
        Judged::Chose(ActionChoice {
            chosen: Chosen::Done,
            probabilities: BTreeMap::new(),
            confidence: 0.9,
            guard: None,
        }),
    ]);
    let mut world = FakeWorld::that_moves(&[1, 2]);

    let walked = run(Mode::On, true, &goal(10), &mut judge, &mut world);

    assert_eq!(world.presses, [1]);
    assert_eq!(walked.reached, Some(true));
    let last = walked.rows.last().expect("a row per step");
    assert_eq!(last["chosen"], json!(DONE));
    assert_eq!(last["reachedBy"], json!("judgment"));
}

#[test]
fn a_screen_that_will_not_move_ends_the_walk_whatever_its_step_budget_says() {
    // The rule that makes an unattended walk terminate: not the budget — a
    // screen that has not moved under two presses.
    let mut judge = FakeJudge::chose(&[1, 2, 3, 4, 5, 6]);
    let mut world = FakeWorld::showing(&[1, 2, 3, 4, 5, 6]);

    let walked = run(
        Mode::On,
        true,
        &goal(zerocode_core::computer_use::WALK_STEPS_MAX),
        &mut judge,
        &mut world,
    );

    assert_eq!(
        world.presses.len(),
        SAME_SCREEN_LIMIT,
        "two presses that changed nothing, and no third"
    );
    let last = walked.rows.last().expect("a row per step");
    assert_eq!(last["outcome"], json!("stuck"));
    assert_eq!(last["pressed"], json!(SAME_SCREEN_LIMIT));
    assert_eq!(walked.reached, Some(false));

    // And while the screen stood still, the numbers already spent were never
    // offered again — the second guess is a different one.
    assert_eq!(judge.asked, [vec![1, 2, 3, 4, 5, 6], vec![2, 3, 4, 5, 6]]);
}

#[test]
fn a_screen_that_moved_offers_its_numbers_again_because_they_mean_something_else() {
    let mut judge = FakeJudge::chose(&[1, 1, 1]);
    let mut world = FakeWorld::that_moves(&[1, 2]);

    run(Mode::On, true, &goal(3), &mut judge, &mut world);

    assert_eq!(world.presses, [1, 1, 1]);
    assert_eq!(
        judge.asked,
        [vec![1, 2], vec![1, 2], vec![1, 2]],
        "a screen that moved is a new screen, and its numbering starts again"
    );
}

#[test]
fn a_goal_walk_spends_no_more_than_the_steps_it_was_given() {
    let mut judge = FakeJudge::chose(&[1, 2, 3, 4, 5, 6, 7]);
    let mut world = FakeWorld::that_moves(&[1, 2, 3, 4, 5, 6, 7]);

    let walked = run(Mode::On, true, &goal(4), &mut judge, &mut world);

    assert_eq!(world.presses.len(), 4);
    assert_eq!(walked.rows.len(), 4);
    assert_eq!(walked.reached, Some(false), "out of steps is not arriving");
}

#[test]
fn a_goal_walk_passes_every_gate_a_recovery_does() {
    // The point of one `barred`: a goal walk cannot quietly acquire a
    // narrower set of gates than the errand the gates were written for.
    let spec = FlowSpec {
        policy: Policy::Guarded,
        evidence: EvidenceLevel::default(),
        fingerprint: Fingerprint::default(),
        checks: Vec::new(),
        money: None,
        confirm: Confirm::default(),
        trigger: None,
    };
    let mut guarded = goal(10);
    guarded.flow = Some(&spec);
    assert_eq!(barred(Mode::On, &guarded, 60_000), Some(Barred::Guarded));

    let mut moneyed = goal(10);
    moneyed.moves_money = true;
    assert_eq!(barred(Mode::On, &moneyed, 60_000), Some(Barred::MovesMoney));

    assert_eq!(barred(Mode::Off, &goal(10), 60_000), Some(Barred::Off));
    let deadline = u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    assert_eq!(
        barred(Mode::On, &goal(10), deadline),
        Some(Barred::NoBudget)
    );
    assert_eq!(barred(Mode::On, &goal(0), 60_000), Some(Barred::NoSteps));
    assert!(barred(Mode::On, &goal(1), 60_000).is_none());

    // Off and record-only change nothing about the walk, for a goal as for a
    // stop: nothing is pressed and the caller ends where it began.
    for mode in [Mode::Off, Mode::Shadow, Mode::Auto] {
        let mut judge = FakeJudge::chose(&[1, 2]);
        let mut world = FakeWorld::that_moves(&[1, 2]);
        let walked = run(mode, mode.applies(), &goal(10), &mut judge, &mut world);
        assert!(world.presses.is_empty(), "{mode:?}");
        assert_eq!(walked.pressed, 0, "{mode:?}");
        assert_eq!(walked.reached, Some(false), "{mode:?}");
    }
}

#[test]
fn the_two_surfaces_sit_in_their_own_seats() {
    // A seat added to the Jev table arrives with its own ledger and its own
    // settings key; neither surface can be turned on by the other's switch.
    assert_eq!(seat_of(Surface::Page).id, "browser");
    assert_eq!(seat_of(Surface::Desk).id, "desktop");
    assert_ne!(
        seat_of(Surface::Page).setting,
        seat_of(Surface::Desk).setting
    );
    assert_ne!(seat_of(Surface::Page).ledger, seat_of(Surface::Desk).ledger);
    assert_eq!(
        seat_of(Surface::Desk),
        zerocode_core::jev::jev_use("desktop").expect("the desktop seat is in the table")
    );
    for seat in [
        seat_of(Surface::Page),
        seat_of(Surface::Desk),
        seat_of(Surface::Phone),
    ] {
        assert!(seat.promotes, "{}: a screen seat's auto can rise", seat.id);
        assert_eq!(
            seat.apply_deadline_ms,
            Some(u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX)),
            "{}: the wall the walk waits is the wall the judge reads",
            seat.id
        );
    }
}

#[test]
fn two_looks_are_the_same_screen_when_the_question_would_read_the_same_words() {
    let page = |host: &str, label: &str| Screen {
        at: Seen::Page {
            host: host.into(),
            path: "/x".into(),
        },
        items: vec![control(1, label)],
        shows: Vec::new(),
    };
    assert!(page("a.local", "저장").same_as(&page("a.local", "저장")));
    assert!(!page("a.local", "저장").same_as(&page("a.local", "삭제")));
    assert!(!page("a.local", "저장").same_as(&page("b.local", "저장")));

    // A pixel that moved is not a screen that moved: the comparison is the
    // legend line, which is what the question reads.
    let mut nudged = page("a.local", "저장");
    nudged.items[0]["width"] = json!(99.0);
    assert!(page("a.local", "저장").same_as(&nudged));

    // A control that appeared is a screen that moved.
    let mut grown = page("a.local", "저장");
    grown.items.push(control(2, "취소"));
    assert!(!page("a.local", "저장").same_as(&grown));

    // The desktop's address counts the same way.
    let desk = |app: &str| Screen {
        at: Seen::Desk {
            app: app.into(),
            window: "채팅".into(),
        },
        items: vec![control(1, "보내기")],
        shows: Vec::new(),
    };
    assert!(desk("카카오톡").same_as(&desk("카카오톡")));
    assert!(!desk("카카오톡").same_as(&desk("Finder")));
    assert!(!desk("카카오톡").same_as(&page("a.local", "보내기")));
}

#[test]
fn the_actual_goal_press_carries_its_judgment_and_restores_the_recording_context() {
    let mut world = FakeWorld::showing(&[1]);
    let mut judge = FakeJudge::chose(&[1]);
    run(Mode::On, true, &goal(1), &mut judge, &mut world);
    assert_eq!(world.observed.len(), 1);
    let observed = world.observed[0].as_ref().unwrap();
    assert_eq!(observed["judgment"]["asked"], true);
    assert_eq!(observed["judgment"]["confidence"], 0.7);
    assert!(observed["judgment"]["ms"].is_u64());
    assert!(observed["look_ms"].is_u64());
    assert!(crate::run_evidence::observation().is_none());
}

// ---- the seat's standing, and the hindsight it is earned on ---------------

#[test]
fn an_auto_its_own_ledger_raised_presses_what_an_unraised_one_only_recorded() {
    // The one difference promotion makes, held in one test: the same mode,
    // the same screen, the same answer — and the caller's `acting` is what
    // decides whether the number is pressed or only written down.
    for (acting, presses, route) in [(false, 0, USE_SHADOW), (true, 1, USE_APPLIED)] {
        let mut judge = FakeJudge::chose(&[2]);
        let mut world = FakeWorld::showing(&[1, 2]);

        let walked = run(
            Mode::Auto,
            acting,
            &stopped(RecipeStop::CheckFailed),
            &mut judge,
            &mut world,
        );

        assert_eq!(world.presses.len(), presses, "acting={acting}");
        assert_eq!(walked.rows[0]["mode"], json!(Mode::Auto.key()));
        assert_eq!(walked.rows[0]["chosen"], json!("mark:2"), "acting={acting}");
        assert_eq!(walked.rows[0]["routeUse"], json!(route), "acting={acting}");
    }
}

#[test]
fn a_walk_that_got_there_agrees_with_every_press_it_took_to_get_there() {
    // The unit is the walk and not the press: the first press left the
    // caller's condition unmet and is still one of the presses that arrived.
    let mut judge = FakeJudge::chose(&[1, 2, 3]);
    let mut world = FakeWorld::that_moves(&[1, 2, 3]);
    world.reached = vec![false, true];

    let walked = run(Mode::On, true, &goal(10), &mut judge, &mut world);

    assert_eq!(walked.pressed, 2);
    assert_eq!(walked.agreed, Some(true));
    assert_eq!(walked.rows[0]["recheck"], json!(false));
    for row in &walked.rows {
        assert_eq!(row[AGREED.canonical], json!(true), "{row}");
    }
}

#[test]
fn a_walk_that_never_got_there_disagrees_with_the_presses_it_spent() {
    let mut judge = FakeJudge::chose(&[1, 2, 3]);
    let mut world = FakeWorld::showing(&[1, 2, 3]);
    world.reached = vec![false, false, false];

    let walked = run(Mode::On, true, &goal(10), &mut judge, &mut world);

    // Two presses that moved nothing, and then the screen ended the walk.
    assert_eq!(walked.pressed, SAME_SCREEN_LIMIT);
    assert_eq!(walked.agreed, Some(false));
    let stuck = walked.rows.last().expect("the walk says why it stopped");
    assert_eq!(stuck["outcome"], json!("stuck"));
    assert!(
        stuck.get(AGREED.canonical).is_none(),
        "the mark is about presses, and this row is not one"
    );
    for row in &walked.rows[..walked.pressed] {
        assert_eq!(row[AGREED.canonical], json!(false), "{row}");
    }
}

#[test]
fn a_walk_nothing_checked_is_left_out_of_the_agreement_rather_than_guessed_at() {
    // No `until`: the world answers no condition, so nothing but the
    // judgment's own word says the walk arrived, and §4 counts neither way.
    let mut judge = FakeJudge::chose(&[1, 2, 3]);
    let mut world = FakeWorld::that_moves(&[1, 2, 3]);

    let walked = run(Mode::On, true, &goal(3), &mut judge, &mut world);

    assert_eq!(walked.pressed, 3);
    assert_eq!(walked.agreed, None);
    for row in &walked.rows {
        assert!(row.get(AGREED.canonical).is_none(), "{row}");
    }

    // And the judgment's own `done` is that same weaker word: it ends the
    // walk, it does not testify about the press before it.
    let mut judge = FakeJudge::saying(vec![
        pick(1),
        Judged::Chose(ActionChoice {
            chosen: Chosen::Done,
            probabilities: BTreeMap::new(),
            confidence: 0.9,
            guard: None,
        }),
    ]);
    let mut world = FakeWorld::that_moves(&[1, 2]);

    let walked = run(Mode::On, true, &goal(10), &mut judge, &mut world);

    assert_eq!(walked.reached, Some(true), "the judgment said it arrived");
    assert_eq!(walked.agreed, None, "and nothing checked that it had");
    for row in &walked.rows {
        assert!(row.get(AGREED.canonical).is_none(), "{row}");
    }
}

#[test]
fn a_recovery_agrees_with_its_press_exactly_when_the_stop_was_cleared() {
    let mut judge = FakeJudge::chose(&[2]);
    let mut world = FakeWorld::showing(&[1, 2, 3]);

    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(recovered.agreed, Some(true));
    assert_eq!(recovered.rows[0][AGREED.canonical], json!(true));

    // The same walk whose re-walks keep stopping at the step that stopped.
    let mut judge = FakeJudge::chose(&[1, 3]);
    let mut world = FakeWorld::showing(&[1, 2, 3]);
    world.walks = vec![Some(4), Some(4)];

    let recovered = run(
        Mode::On,
        true,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(recovered.agreed, Some(false));
    for row in &recovered.rows {
        assert_eq!(row[AGREED.canonical], json!(false), "{row}");
    }
}

#[test]
fn a_walk_that_pressed_nothing_agrees_with_nothing() {
    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[]);

    let walked = run(Mode::On, true, &goal(3), &mut judge, &mut world);

    assert_eq!(walked.rows[0]["outcome"], json!("no_candidate"));
    assert_eq!(walked.pressed, 0);
    assert_eq!(walked.agreed, None, "a shadow row testifies about no press");
    assert!(walked.rows[0].get(AGREED.canonical).is_none());
}

/// One row of a walk, as the seat writes them.
fn answered_press(at_ms: i64, agreed: bool) -> Value {
    json!({
        AT.canonical: at_ms,
        "outcome": "answered",
        ELAPSED_MS.canonical: 300,
        "pressed": true,
        AGREED.canonical: agreed,
    })
}

/// A wire whose config home is `home` and whose settings put `seat` in `mode`.
fn wire_at(home: &tempfile::TempDir, seat: &JevUse, mode: Mode) -> crate::systemone::Wire {
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({ zerocode_core::jev::SMART_SETTINGS_KEY: { seat.setting: mode.key() } }).to_string(),
    )
    .expect("zo's settings");
    crate::systemone::Wire::at("http://127.0.0.1:1", "test-key", Some(settings))
}

#[test]
fn a_walks_rows_land_in_its_evidence_folder_and_in_the_seats_one_ledger() {
    let home = tempfile::tempdir().expect("a zo home");
    let session = home.path().join("sessions").join("2026-09-21");
    let seat = seat_of(Surface::Page);
    let wire = wire_at(&home, seat, Mode::Auto);

    write_rows(seat, &wire, Some(&session), &[answered_press(1, true)], 1);

    let beside_the_evidence = session.join(seat.ledger);
    let one_ledger = crate::systemone::ledger_of(&wire, seat).expect("the seat's ledger");
    assert_eq!(
        std::fs::read_to_string(&beside_the_evidence).expect("the evidence copy"),
        std::fs::read_to_string(&one_ledger).expect("the judged copy"),
        "one row, written down in both places it belongs"
    );
    assert!(
        one_ledger.ends_with(
            std::path::Path::new(zerocode_core::jev::count::REQUESTS_DIR).join(seat.ledger)
        ),
        "{}",
        one_ledger.display()
    );
}

#[test]
fn a_screen_seats_auto_rises_on_its_own_rows_and_falls_when_the_wire_does() {
    let home = tempfile::tempdir().expect("a zo home");
    let seat = seat_of(Surface::Page);
    let wire = wire_at(&home, seat, Mode::Auto);
    let ledger = crate::systemone::ledger_of(&wire, seat).expect("the seat's ledger");
    assert!(
        !crate::systemone::applies(&wire, seat),
        "an auto nobody has raised records"
    );

    // A window's worth of presses the walks went on to confirm — the seat's
    // own width, read from the table, so the count lands on the judgment's
    // first cadence and the window it reads is full. The width is not spelled
    // here: it moves with the seat's floor and with what its window forgives.
    // The oldest few the walks were still stuck after: a label that has
    // never said no is not evidence (t-6342).
    let wanted = zerocode_core::jev::promote::window_wanted_for(seat).expect("a screen seat rises");
    let misses = seat.negatives_wanted.expect("a screen seat rises") as i64;
    let rows: Vec<Value> = (0..wanted as i64)
        .map(|n| answered_press(1_000 + n, n >= misses))
        .collect();
    write_rows(seat, &wire, None, &rows, 90_000);

    let judged = crate::systemone::read_rows(&ledger);
    let transition = judged.last().expect("a judgment stands beside the rows");
    assert_eq!(
        transition[zerocode_core::jev::summary::TRANSITION.canonical],
        json!(zerocode_core::jev::promote::ROSE),
        "{transition}"
    );
    assert!(
        crate::systemone::applies(&wire, seat),
        "and the next walk presses"
    );

    // Three answers in a row that never came back end it at once, whatever
    // the cadence says: that is the wire, the key or the model.
    let dead: Vec<Value> = (0..3)
        .map(|n| json!({ AT.canonical: 2_000 + n, "outcome": "timeout" }))
        .collect();
    write_rows(seat, &wire, None, &dead, 95_000);

    let judged = crate::systemone::read_rows(&ledger);
    assert_eq!(
        judged.last().expect("a fall stands too")
            [zerocode_core::jev::summary::TRANSITION.canonical],
        json!(zerocode_core::jev::promote::FELL)
    );
    assert!(!crate::systemone::applies(&wire, seat));
}

// ---- asking ahead of the look (t-6132 S2) ----------------------------------

/// A row with the two clocks and the wall time taken off, for comparing
/// what two walks wrote apart from when.
fn timeless(row: &Value) -> Value {
    let mut row = row.clone();
    if let Some(row) = row.as_object_mut() {
        row.remove(AT.canonical);
        row.remove(ELAPSED_MS.canonical);
    }
    row
}

#[test]
fn overlap_off_is_todays_walk_to_the_byte() {
    let mut plain_judge = FakeJudge::chose(&[1, 2, 1]);
    let mut plain_world = FakeWorld::that_moves_every_other(&[1, 2]);
    let plain = run(Mode::On, true, &goal(3), &mut plain_judge, &mut plain_world);

    let mut judge = FakeJudge::chose(&[1, 2, 1]);
    let mut world = FakeWorld::that_moves_every_other(&[1, 2]);
    let off = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(3),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(
        off.rows.iter().map(timeless).collect::<Vec<_>>(),
        plain.rows.iter().map(timeless).collect::<Vec<_>>()
    );
    assert_eq!((off.overlapped, off.discarded), (0, 0));
    assert!(judge.begun.is_empty(), "nothing is asked ahead");
    assert_eq!(judge.asked, plain_judge.asked);
    assert_eq!(world.presses, plain_world.presses);
    assert!(off.rows.iter().all(|row| row.get(OVERLAP).is_none()));
}

/// After a press on a screen that then stays, the walk's next question is
/// the one begun on the last look — the pressed number spent, its legend
/// among `pressed` — and the walk uses that answer instead of asking again.
#[test]
fn a_judgment_begun_on_the_last_look_answers_the_next_look_that_asks_the_same_question() {
    let mut judge = FakeJudge::chose(&[1, 2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(3),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert_eq!(world.presses, vec![1, 2]);
    assert_eq!(judge.asked, vec![vec![1, 2]], "asked in turn once");
    assert_eq!(judge.begun, vec![vec![2]], "begun ahead once: mark 1 spent");
    assert_eq!(judge.finished, 1);
    assert_eq!((walked.overlapped, walked.discarded), (1, 0));
    assert!(walked.rows[0].get(OVERLAP).is_none());
    assert_eq!(walked.rows[1][OVERLAP], json!(OVERLAP_USED));
    assert!(walked.rows[1]["hiddenMs"].is_u64());
    assert_eq!(walked.rows[1]["chosen"], json!("mark:2"));
    // The screen never moved: the third look is where the walk stops, and
    // no judgment was begun for it (one more stand would end the walk).
    assert_eq!(walked.rows[2]["outcome"], json!("stuck"));
}

/// A screen that moved asks another question: the judgment begun ahead is
/// dropped, its request spent, and the new screen is asked in turn.
#[test]
fn a_judgment_begun_ahead_is_dropped_when_the_next_look_asks_another_question() {
    let mut judge = FakeJudge::chose(&[1, 1, 1]);
    let mut world = FakeWorld::that_moves(&[1, 2]);
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(2),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert_eq!(world.presses, vec![1, 1]);
    assert_eq!(judge.begun, vec![vec![2]], "begun on the first screen");
    assert_eq!(judge.asked.len(), 2, "both screens asked in turn");
    assert_eq!(judge.finished, 0);
    assert_eq!((walked.overlapped, walked.discarded), (0, 1));
    assert_eq!(walked.rows[1][OVERLAP], json!(OVERLAP_DISCARDED));
}

/// No question is begun ahead after a link, on the last step, on a screen
/// one more stand from stuck, for a walk that clears a stop, or by a judge
/// that cannot ask ahead.
#[test]
fn nothing_is_begun_ahead_where_the_next_screen_is_another_page_or_the_walk_ends() {
    // A link: the screen after it is another page.
    let mut judge = FakeJudge::chose(&[1, 2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    world.screen.as_mut().unwrap().items[0]["role"] = json!("link");
    run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(3),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert!(judge.begun.is_empty(), "a link is not asked ahead");
    assert!(moves_the_page(&json!({ "role": "link" })));
    assert!(moves_the_page(&json!({ "role": "AXLink" })));
    assert!(!moves_the_page(&json!({ "role": "button" })));

    // The last step asks nothing more.
    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[1, 2]);
    run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(1),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert!(judge.begun.is_empty());

    // A recovery walks the document again; it never comes back to a look.
    let mut judge = FakeJudge::chose(&[1, 2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    world.walks = vec![Some(4), None];
    run_with(
        Mode::On,
        true,
        Branching::OFF,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert!(judge.begun.is_empty());

    // A judge that cannot ask ahead: the walk asks in turn.
    let mut judge = FakeJudge::chose(&[1, 2]);
    judge.overlaps = false;
    let mut world = FakeWorld::showing(&[1, 2]);
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(3),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert_eq!(judge.asked.len(), 2);
    assert_eq!((walked.overlapped, walked.discarded), (0, 0));
}

/// A phone world: its press settles and hands back the screen it stopped on,
/// and it never asks ahead of a press (t-6385).
fn a_phone_that_settles(world: FakeWorld) -> FakeWorld {
    FakeWorld {
        settles: Some(json!({ "ms": 900, "reads": 5, "settle": "still" })),
        settles_on_screen: true,
        asks_before_press: false,
        ..world
    }
}

/// A phone's walk asks its next question on the screen its press settled on
/// (t-6385): nothing is begun before a press, the question begun after one
/// is the very one the next look asks, and its answer is used.
#[test]
fn a_phone_walk_asks_its_next_question_on_the_screen_its_press_settled_on() {
    let mut judge = FakeJudge::chose(&[1, 1, 1]);
    let mut world = a_phone_that_settles(FakeWorld::that_moves(&[1, 2]));
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(3),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert_eq!(world.presses, vec![1, 1, 1]);
    assert_eq!(
        judge.asked.len(),
        1,
        "only the first screen is asked in turn"
    );
    assert_eq!(
        judge.begun.len(),
        2,
        "begun after the first two presses, not the last"
    );
    assert_eq!(judge.finished, 2);
    assert_eq!((walked.overlapped, walked.discarded), (2, 0));
    assert!(walked.rows[0].get(OVERLAP).is_none());
    assert_eq!(walked.rows[1][OVERLAP], json!(OVERLAP_USED));
    assert_eq!(walked.rows[2][OVERLAP], json!(OVERLAP_USED));
}

/// A preview the full look disagrees with — an element only the grid finds,
/// a screen still moving — asks another question: the answer begun on it is
/// dropped and the look is asked in turn.
#[test]
fn a_preview_the_full_look_disagrees_with_is_dropped_and_the_look_asked_again() {
    let mut judge = FakeJudge::chose(&[1, 1, 1]);
    let mut world = a_phone_that_settles(FakeWorld::that_moves(&[1, 2]));
    world.preview_items = Some(vec![control(1, "미리보기")]);
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(2),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert_eq!(judge.begun.len(), 1);
    assert_eq!(judge.asked.len(), 2, "both screens asked in turn");
    assert_eq!((walked.overlapped, walked.discarded), (0, 1));
    assert_eq!(walked.rows[1][OVERLAP], json!(OVERLAP_DISCARDED));
}

/// A world whose press moves its screen too often to ask ahead of it (a
/// phone's) asks nothing before a press, and — with no settled screen to ask
/// on — nothing after one either.
#[test]
fn a_world_that_asks_after_its_press_never_asks_before_it() {
    let mut judge = FakeJudge::chose(&[1, 2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    world.asks_before_press = false;
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(3),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert!(judge.begun.is_empty());
    assert_eq!((walked.overlapped, walked.discarded), (0, 0));
}

/// The measurement asking on the settled screen is for: a phone walk whose
/// looks hold the tree read's own time and whose judgments take the wire's,
/// with and without the question begun on the screen each press settled on.
/// Printed, and held on the count: every judgment after the first was
/// answered while the look was taken.
#[test]
fn a_judgment_asked_on_the_settled_screen_hides_behind_the_look() {
    const JUDGE_MS: u64 = 60;
    const LOOK_MS: u64 = 80;
    const STEPS: usize = 6;
    let walk = |overlap: bool| {
        let mut judge = FakeJudge::chose(&[1; STEPS]).slow(JUDGE_MS);
        let mut world = a_phone_that_settles(FakeWorld::that_moves(&[1, 2]));
        world.look_holds = Duration::from_millis(LOOK_MS);
        let began = std::time::Instant::now();
        let walked = run_with(
            Mode::On,
            true,
            Branching::OFF,
            &goal(STEPS),
            &mut judge,
            &mut world,
            Options {
                overlap,
                rescue: false,
            },
            None,
        );
        (
            began.elapsed().as_millis(),
            walked,
            judge.asked.len() + judge.begun.len(),
        )
    };
    let (before_ms, plain, plain_asks) = walk(false);
    let (after_ms, ahead, ahead_asks) = walk(true);
    assert_eq!((plain.pressed, ahead.pressed), (STEPS, STEPS));
    assert_eq!((plain.overlapped, plain.discarded), (0, 0));
    assert_eq!((ahead.overlapped, ahead.discarded), (STEPS - 1, 0));
    assert_eq!((plain_asks, ahead_asks), (STEPS, STEPS), "no question more");
    let hidden_per_step = ahead
        .rows
        .iter()
        .filter_map(|row| row["hiddenMs"].as_u64())
        .sum::<u64>()
        / ahead.overlapped as u64;
    assert!(
        hidden_per_step >= LOOK_MS,
        "a judgment begun on the settled screen ran at least as long as the look held: {hidden_per_step} ms"
    );
    println!(
        "measure: settled-screen overlap steps={STEPS} judge_ms={JUDGE_MS} look_ms={LOOK_MS} before_ms={before_ms} after_ms={after_ms} overlapped={} hidden_ms_per_used_step={hidden_per_step}",
        ahead.overlapped
    );
}

/// The measurement asking ahead is for: a walk whose presses hold the door's
/// own landing and whose judgments take the wire's own time, with and
/// without the judgment begun ahead — on a screen that stays after every
/// second press, where the answer in flight can be used. Printed, and held
/// on the count: every other judgment was hidden behind a press.
#[test]
fn a_judgment_hidden_behind_the_press_shortens_the_walk_by_what_it_hid() {
    const JUDGE_MS: u64 = 60;
    const PRESS_MS: u64 = 80;
    const STEPS: usize = 8;
    let walk = |overlap: bool| {
        let mut judge = FakeJudge::chose(&[1, 2, 1, 2, 1, 2, 1, 2]).slow(JUDGE_MS);
        let mut world = FakeWorld::that_moves_every_other(&[1, 2]).holding(PRESS_MS);
        let began = std::time::Instant::now();
        let walked = run_with(
            Mode::On,
            true,
            Branching::OFF,
            &goal(STEPS),
            &mut judge,
            &mut world,
            Options {
                overlap,
                rescue: false,
            },
            None,
        );
        (
            began.elapsed().as_millis(),
            walked,
            judge.asked.len() + judge.begun.len(),
        )
    };
    let (before_ms, plain, plain_asks) = walk(false);
    let (after_ms, ahead, ahead_asks) = walk(true);
    assert_eq!(plain.pressed, STEPS);
    assert_eq!(ahead.pressed, STEPS);
    assert_eq!((plain.overlapped, plain.discarded), (0, 0));
    assert_eq!(
        (ahead.overlapped, ahead.discarded),
        (STEPS / 2, 0),
        "every other judgment was begun ahead and used"
    );
    assert_eq!(plain_asks, STEPS);
    assert_eq!(ahead_asks, STEPS, "asking ahead asks no more questions");
    let hidden_per_step = ahead
        .rows
        .iter()
        .filter_map(|row| row["hiddenMs"].as_u64())
        .sum::<u64>()
        / ahead.overlapped.max(1) as u64;
    assert!(
        hidden_per_step >= PRESS_MS,
        "a judgment begun before the press ran at least as long as the press held: {hidden_per_step} ms"
    );
    // A step is the time from one judgment to the next, read off the rows'
    // own clocks.
    let steps_of = |walked: &Walked| -> Vec<u64> {
        let at: Vec<i64> = walked
            .rows
            .iter()
            .filter_map(|row| row[AT.canonical].as_i64())
            .collect();
        let mut steps: Vec<u64> = at
            .windows(2)
            .map(|pair| u64::try_from(pair[1] - pair[0]).unwrap_or(0))
            .collect();
        steps.sort_unstable();
        steps
    };
    let (plain_steps, ahead_steps) = (steps_of(&plain), steps_of(&ahead));
    let pct = |steps: &[u64], share: f64| {
        zerocode_core::jev::summary::percentile(steps, share).unwrap_or(0)
    };
    println!(
        "measure: overlap steps={STEPS} judge_ms={JUDGE_MS} press_ms={PRESS_MS} before_ms={before_ms} after_ms={after_ms} step_p50_before={} step_p95_before={} step_p50_after={} step_p95_after={} overlapped={} discarded={} discard_share={:.2} hidden_ms_per_used_step={hidden_per_step}",
        pct(&plain_steps, 0.5),
        pct(&plain_steps, 0.95),
        pct(&ahead_steps, 0.5),
        pct(&ahead_steps, 0.95),
        ahead.overlapped,
        ahead.discarded,
        ahead.discarded as f64 / (ahead.overlapped + ahead.discarded).max(1) as f64
    );
}

// ---- the second rung (t-6132 S3) -------------------------------------------

/// A judge whose every answer sits under the seat's press floor — what the
/// second rung is for.
fn unsure(marks: &[usize]) -> FakeJudge {
    let mut judge = FakeJudge::chose(marks);
    for answer in &mut judge.answers {
        if let Judged::Chose(choice) = answer {
            choice.confidence = 0.29;
        }
    }
    judge
}

fn sure(marks: &[usize]) -> FakeJudge {
    let mut judge = FakeJudge::chose(marks);
    for answer in &mut judge.answers {
        if let Judged::Chose(choice) = answer {
            choice.confidence = 0.9;
        }
    }
    judge
}

#[test]
fn rescue_off_is_todays_walk_to_the_byte_whoever_was_handed_in() {
    let mut plain_judge = unsure(&[1]);
    let mut plain_world = FakeWorld::showing(&[1, 2]);
    let plain = run(Mode::On, true, &goal(3), &mut plain_judge, &mut plain_world);

    let mut judge = unsure(&[1]);
    let mut team = sure(&[2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    let off = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(3),
        &mut judge,
        &mut world,
        Options::default(),
        Some(&mut team),
    );
    assert_eq!(
        off.rows.iter().map(timeless).collect::<Vec<_>>(),
        plain.rows.iter().map(timeless).collect::<Vec<_>>()
    );
    assert!(team.asked.is_empty(), "the second reader is never asked");
    assert!(world.presses.is_empty());
    assert_eq!((off.rescued, off.rescue_failed), (0, 0));
    assert_eq!(off.rows[0]["barred"], json!(Barred::LowConfidence.as_str()));
}

/// The seat's judgment under its floor, the second reader's above it: the
/// second reader's number is pressed under the seat's own rule, and the row
/// says who pressed and what each reader said.
#[test]
fn a_judgment_under_the_floor_is_pressed_for_by_the_second_reader_when_it_is_sure() {
    let mut judge = unsure(&[1]);
    let mut team = sure(&[2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(1),
        &mut judge,
        &mut world,
        Options {
            overlap: false,
            rescue: true,
        },
        Some(&mut team),
    );
    assert_eq!(world.presses, vec![2], "the second reader's number");
    assert_eq!(team.asked, vec![vec![1, 2]], "the same closed choice");
    assert_eq!((walked.rescued, walked.rescue_failed), (1, 0));
    let row = &walked.rows[0];
    assert_eq!(row["pressed"], json!(true));
    assert_eq!(row["chosen"], json!("mark:2"));
    assert_eq!(
        row["confidence"],
        json!(0.29),
        "the seat's own judgment stays on the row"
    );
    assert_eq!(row[RESCUED_BY], json!(RESCUED_BY_TEAM));
    assert_eq!(row[RESCUE]["outcome"], json!("pressed"));
    assert_eq!(row[RESCUE]["chosen"], json!("mark:2"));
    assert_eq!(row[RESCUE]["confidence"], json!(0.9));
    assert!(row[RESCUE][ELAPSED_MS.canonical].is_u64());
    assert!(row.get("barred").is_none());
    assert_eq!(row["routeUse"], json!("applied"));
}

/// Where the second reader cannot press for the walk — unsure itself,
/// refused, a link, `give_up`, `done` — the walk steps back to the person
/// exactly as it does today, and the row says why the rung did not hold.
#[test]
fn a_second_reader_that_cannot_press_leaves_the_walk_where_today_leaves_it() {
    let cases: Vec<(&str, FakeJudge, Option<&str>)> = vec![
        ("low_confidence", unsure(&[2]), None),
        ("timeout", FakeJudge::saying(Vec::new()), None),
        (
            "give_up",
            FakeJudge::saying(vec![Judged::Chose(ActionChoice {
                chosen: Chosen::GiveUp,
                probabilities: BTreeMap::new(),
                confidence: 0.9,
                guard: None,
            })]),
            None,
        ),
        (
            "done",
            FakeJudge::saying(vec![Judged::Chose(ActionChoice {
                chosen: Chosen::Done,
                probabilities: BTreeMap::new(),
                confidence: 0.9,
                guard: None,
            })]),
            None,
        ),
        ("link", sure(&[2]), Some("link")),
    ];
    for (why, mut team, role_of_two) in cases {
        let mut judge = unsure(&[1]);
        let mut world = FakeWorld::showing(&[1, 2]);
        if let Some(role) = role_of_two {
            world.screen.as_mut().unwrap().items[1]["role"] = json!(role);
        }
        let walked = run_with(
            Mode::On,
            true,
            Branching::OFF,
            &goal(1),
            &mut judge,
            &mut world,
            Options {
                overlap: false,
                rescue: true,
            },
            Some(&mut team),
        );
        assert!(world.presses.is_empty(), "{why}: nothing is pressed");
        assert_eq!(team.asked.len(), 1, "{why}: the second reader was asked");
        assert_eq!((walked.rescued, walked.rescue_failed), (0, 1), "{why}");
        let row = &walked.rows[0];
        assert_eq!(
            row["barred"],
            json!(Barred::LowConfidence.as_str()),
            "{why}"
        );
        assert_eq!(row["pressed"], json!(false), "{why}");
        assert_eq!(row["routeUse"], json!("fallback"), "{why}");
        assert_eq!(row[RESCUE]["outcome"], json!(why), "{why}: {row}");
        assert!(row.get(RESCUED_BY).is_none(), "{why}");
        assert_eq!(
            row["chosen"],
            json!("mark:1"),
            "{why}: the seat's own number stays"
        );
    }
}

/// A recovery is rescued the same way: the second reader's press, then the
/// document walked again from the report's own resume point.
#[test]
fn a_recovery_under_the_floor_is_pressed_for_and_the_document_walked_again() {
    let mut judge = unsure(&[1]);
    let mut team = sure(&[2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    let walked = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut world,
        Options {
            overlap: false,
            rescue: true,
        },
        Some(&mut team),
    );
    assert_eq!(world.presses, vec![2]);
    assert_eq!(world.walked_from, vec![4]);
    assert_eq!(walked.rescued, 1);
    assert_eq!(walked.rows[0]["recheck"], json!(true));
}

/// A seat that is sure never climbs the rung: the second reader is not
/// asked, and its cost is not paid.
#[test]
fn a_sure_judgment_never_asks_the_second_reader() {
    let mut judge = sure(&[1]);
    let mut team = sure(&[2]);
    let mut world = FakeWorld::showing(&[1, 2]);
    run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(1),
        &mut judge,
        &mut world,
        Options {
            overlap: false,
            rescue: true,
        },
        Some(&mut team),
    );
    assert_eq!(world.presses, vec![1]);
    assert!(team.asked.is_empty());
}

/// The measurement the rung is for: a Flow whose seat is unsure on half of
/// its steps, walked with and without the second reader — how many steps it
/// pressed for that would have gone to the person, how many it could not,
/// and what each rung cost the walk in time. Printed; held on the counts.
#[test]
fn the_second_rung_presses_for_the_steps_the_seat_left_and_costs_its_own_turn() {
    const STEPS: usize = 6;
    const JUDGE_MS: u64 = 40;
    const TEAM_MS: u64 = 120;
    // The seat: sure, unsure, sure, unsure … ; the second reader: always sure.
    let seat = || {
        let mut judge = FakeJudge::chose(&[1, 1, 1, 1, 1, 1]).slow(JUDGE_MS);
        for (n, answer) in judge.answers.iter_mut().enumerate() {
            if let Judged::Chose(choice) = answer {
                choice.confidence = if n % 2 == 0 { 0.9 } else { 0.29 };
            }
        }
        judge
    };
    // Without the rung the walk stops at its first unsure step: the person.
    let mut plain_judge = seat();
    let mut plain_world = FakeWorld::that_moves(&[1, 2]);
    let began = std::time::Instant::now();
    let plain = run(
        Mode::On,
        true,
        &goal(STEPS),
        &mut plain_judge,
        &mut plain_world,
    );
    let plain_ms = began.elapsed().as_millis();
    assert_eq!(
        plain.pressed, 1,
        "today the walk ends at the first unsure step"
    );
    assert_eq!(
        plain.rows.last().unwrap()["barred"],
        json!(Barred::LowConfidence.as_str())
    );

    let mut judge = seat();
    let mut team = sure(&[2, 2, 2]).slow(TEAM_MS);
    let mut world = FakeWorld::that_moves(&[1, 2]);
    let began = std::time::Instant::now();
    let rescued = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(STEPS),
        &mut judge,
        &mut world,
        Options {
            overlap: false,
            rescue: true,
        },
        Some(&mut team),
    );
    let rescued_ms = began.elapsed().as_millis();
    assert_eq!(
        rescued.pressed, STEPS,
        "every step pressed, half by the second reader"
    );
    assert_eq!((rescued.rescued, rescued.rescue_failed), (STEPS / 2, 0));
    assert_eq!(team.asked.len(), STEPS / 2);

    // A second reader that cannot help: the walk still ends at the person,
    // one turn later.
    let mut judge = seat();
    let mut team = FakeJudge::saying(Vec::new()).slow(TEAM_MS);
    let mut world = FakeWorld::that_moves(&[1, 2]);
    let began = std::time::Instant::now();
    let unhelped = run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(STEPS),
        &mut judge,
        &mut world,
        Options {
            overlap: false,
            rescue: true,
        },
        Some(&mut team),
    );
    let unhelped_ms = began.elapsed().as_millis();
    assert_eq!(unhelped.pressed, 1);
    assert_eq!((unhelped.rescued, unhelped.rescue_failed), (0, 1));
    println!(
        "measure: rescue steps={STEPS} judge_ms={JUDGE_MS} team_ms={TEAM_MS} plain: pressed={} ms={plain_ms} | rescued: pressed={} rescued={} failed={} ms={rescued_ms} | unhelped: pressed={} failed={} ms={unhelped_ms}",
        plain.pressed,
        rescued.pressed,
        rescued.rescued,
        rescued.rescue_failed,
        unhelped.pressed,
        unhelped.rescue_failed
    );
}

use super::guard_fixtures::{CLEAN, Fixture, INJECTED, WALLED};
use zerocode_core::screen_action::{Guard, Stopped};

/// The world a fixture's screen stands in.
fn world_on(fixture: &Fixture) -> FakeWorld {
    let mut world = FakeWorld::showing(&[]);
    world.screen_is(fixture.screen());
    world
}

/// A goal walk of one press on a fixture's screen.
fn goal_on(fixture: &Fixture) -> Errand<'_> {
    Errand {
        goal: fixture.goal,
        why: Why::Goal { steps: 1 },
        flow: None,
        moves_money: false,
    }
}

/// How a guard answers a fixture: sure of what is true of it.
fn yes(holds: bool) -> f64 {
    if holds { 0.92 } else { 0.04 }
}

/// A judge that picks the fixture's first control, sure of it, with both
/// guards saying what the fixture is.
fn judging(fixture: &Fixture) -> FakeJudge {
    FakeJudge::saying(vec![Judged::Chose(ActionChoice {
        chosen: Chosen::Mark(1),
        probabilities: BTreeMap::new(),
        confidence: 0.9,
        guard: Some(Guard {
            instructed: yes(fixture.injected),
            walled: yes(fixture.walled),
        }),
    })])
}

/// An acting seat presses nothing on a screen whose text tells an assistant
/// what to do, nor on a wall, and steps back to the person with the stop
/// named; a clean screen is pressed as today (t-6187). Both guards' values
/// are on every answered row, per thousand.
#[test]
fn an_acting_walk_refuses_an_injected_screen_and_a_wall_and_presses_a_clean_one() {
    let per_thousand = |holds: bool| json!(zerocode_core::jev::promote::permille(yes(holds)));
    for fixture in INJECTED.iter().take(5) {
        let walked = run(
            Mode::On,
            true,
            &goal_on(fixture),
            &mut judging(fixture),
            &mut world_on(fixture),
        );
        assert_eq!(walked.pressed, 0, "{} was pressed", fixture.name);
        let row = walked.rows.last().expect("a row");
        assert_eq!(
            row["barred"],
            json!(Stopped::Injected.word()),
            "{}",
            fixture.name
        );
        assert_eq!(
            no_press_reason(&walked.rows),
            Some(Stopped::Injected.word()),
            "the walk's answer says why"
        );
        assert_eq!(row["pressed"], json!(false));
        assert_eq!(row["routeUse"], json!(USE_FALLBACK));
        assert_eq!(row["instructed"], per_thousand(true));
        assert_eq!(row["walled"], per_thousand(false));
    }
    for fixture in CLEAN.iter().take(5) {
        let walked = run(
            Mode::On,
            true,
            &goal_on(fixture),
            &mut judging(fixture),
            &mut world_on(fixture),
        );
        assert_eq!(walked.pressed, 1, "{} was not pressed", fixture.name);
        let row = walked.rows.last().expect("a row");
        assert!(row.get("barred").is_none(), "{}: {row}", fixture.name);
        assert_eq!(row["routeUse"], json!(USE_APPLIED));
        assert_eq!(row["instructed"], per_thousand(false));
    }
    for fixture in &WALLED {
        let walked = run(
            Mode::On,
            true,
            &goal_on(fixture),
            &mut judging(fixture),
            &mut world_on(fixture),
        );
        assert_eq!(walked.pressed, 0, "{} was pressed", fixture.name);
        let row = walked.rows.last().expect("a row");
        assert_eq!(
            row["barred"],
            json!(Stopped::Walled.word()),
            "{}",
            fixture.name
        );
        assert_eq!(row["walled"], per_thousand(true));
    }
}

/// A seat that only records writes both guards on its row and refuses
/// nothing — it presses nothing anyway, and the row says why in the stand's
/// own word (t-6187).
#[test]
fn a_recording_walk_writes_both_guards_and_refuses_nothing() {
    for fixture in INJECTED
        .iter()
        .take(5)
        .chain(CLEAN.iter().take(5))
        .chain(WALLED.iter())
    {
        let walked = run(
            Mode::Shadow,
            false,
            &goal_on(fixture),
            &mut judging(fixture),
            &mut world_on(fixture),
        );
        assert_eq!(walked.pressed, 0);
        let row = walked.rows.last().expect("a row");
        assert_eq!(row[REASON], json!(SEAT_RECORDING), "{}", fixture.name);
        assert!(row.get("barred").is_none(), "{}: {row}", fixture.name);
        assert_eq!(
            row["instructed"],
            json!(zerocode_core::jev::promote::permille(yes(fixture.injected)))
        );
        assert_eq!(
            row["walled"],
            json!(zerocode_core::jev::promote::permille(yes(fixture.walled)))
        );
    }
}

/// A walk sure of a control at 0.8 presses it when the control is plain and
/// steps back to the person when the control cannot be taken back — a
/// delete, a payment, a send (t-6187): the floor for those is nine in ten.
/// The row names the control's kind either way.
#[test]
fn a_destructive_control_at_eight_in_ten_goes_to_the_person_and_a_plain_one_is_pressed() {
    let sure = |confidence: f64| {
        FakeJudge::saying(vec![Judged::Chose(ActionChoice {
            chosen: Chosen::Mark(1),
            probabilities: BTreeMap::new(),
            confidence,
            guard: None,
        })])
    };
    let on = |label: &str| {
        let mut world = FakeWorld::showing(&[]);
        world.screen_is(Screen {
            at: Seen::default(),
            items: vec![control(1, label), control(2, "닫기")],
            shows: Vec::new(),
        });
        world
    };
    for label in [
        "계정 삭제",
        "Delete account",
        "결제하기",
        "Send",
        "주문 확정",
    ] {
        let mut world = on(label);
        let walked = run(Mode::On, true, &goal(1), &mut sure(0.8), &mut world);
        assert!(world.presses.is_empty(), "{label} was pressed at 0.8");
        let row = walked.rows.last().expect("a row");
        assert_eq!(
            row["barred"],
            json!(Barred::LowConfidence.as_str()),
            "{label}"
        );
        assert_eq!(row["controlKind"], json!("destructive"), "{label}");

        let mut world = on(label);
        run(Mode::On, true, &goal(1), &mut sure(0.9), &mut world);
        assert_eq!(world.presses, [1], "{label} at 0.9 is pressed");
    }
    for label in ["저장", "다음", "Open settings"] {
        let mut world = on(label);
        let walked = run(Mode::On, true, &goal(1), &mut sure(0.8), &mut world);
        assert_eq!(world.presses, [1], "{label} at 0.8 is pressed");
        assert_eq!(
            walked.rows.last().expect("a row")["controlKind"],
            json!("plain")
        );
    }
}

/// The destructive presses the fixture screens would have seen (t-6187): on
/// every control of the guard fixtures' twenty-three screens, a walk sure of
/// that control at each confidence the fixture judges here answer with under
/// nine in ten. Before, the one floor pressed every one of them at or over a
/// half; now a destructive one goes to the person and a plain one is pressed
/// as before. Printed so the report carries the counts; asserted so they
/// cannot drift from the rule.
#[test]
fn the_destructive_presses_under_nine_in_ten_on_the_fixture_screens_before_and_after() {
    use zerocode_core::guarded::ControlKind;
    const UNDER_NINE: [f64; 6] = [0.5, 0.55, 0.6, 0.7, 0.8, 0.85];
    let (mut steps, mut destructive, mut before, mut after, mut plain_kept) = (0, 0, 0, 0, 0);
    let mut kinds = std::collections::BTreeSet::new();
    for fixture in INJECTED.iter().chain(CLEAN.iter()).chain(WALLED.iter()) {
        let screen = fixture.screen();
        for item in &screen.items {
            let mark = usize::try_from(item["mark"].as_u64().expect("a mark")).expect("small");
            let kind = control_kind(&screen, mark);
            for confidence in UNDER_NINE {
                steps += 1;
                let mut world = FakeWorld::showing(&[]);
                world.screen_is(screen.clone());
                let mut judge = FakeJudge::saying(vec![Judged::Chose(ActionChoice {
                    chosen: Chosen::Mark(mark),
                    probabilities: BTreeMap::new(),
                    confidence,
                    guard: None,
                })]);
                let walked = run(Mode::On, true, &goal_on(fixture), &mut judge, &mut world);
                let pressed_now = walked.pressed == 1;
                // Today's rule was the plain floor for every control.
                let pressed_before = BROWSER.permits_press(confidence, ControlKind::Plain);
                if kind == ControlKind::Destructive {
                    destructive += 1;
                    before += usize::from(pressed_before);
                    after += usize::from(pressed_now);
                    kinds.insert(legend_of(&screen, mark));
                } else {
                    plain_kept += usize::from(pressed_now == pressed_before);
                }
            }
        }
    }
    println!(
        "{}",
        json!({
            "steps": steps,
            "destructiveSteps": destructive,
            "destructiveControls": kinds.len(),
            "pressedUnderNineBefore": before,
            "pressedUnderNineAfter": after,
            "plainStepsUnchanged": plain_kept,
            "plainSteps": steps - destructive,
        })
    );
    assert_eq!(
        after, 0,
        "a destructive control was pressed under nine in ten"
    );
    assert_eq!(
        before, destructive,
        "the old floor pressed every one of them"
    );
    assert_eq!(
        plain_kept,
        steps - destructive,
        "a plain control's press moved"
    );
}
