//! What a walk by judgment may do, and what it may not.

use std::collections::BTreeMap;

use super::*;
use zerocode_core::computer_flow::{Confirm, EvidenceLevel, Fingerprint, Money};
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

/// A judge that answers what the test says, and counts what it was asked.
struct FakeJudge {
    answers: Vec<Judged>,
    asked: Vec<Vec<usize>>,
}

impl FakeJudge {
    fn chose(marks: &[usize]) -> Self {
        Self {
            answers: marks.iter().map(|mark| pick(*mark)).collect(),
            asked: Vec::new(),
        }
    }
    fn saying(answers: Vec<Judged>) -> Self {
        Self {
            answers,
            asked: Vec::new(),
        }
    }
}

/// A validated choice of `mark`, as the pure module would have read one.
fn pick(mark: usize) -> Judged {
    Judged::Chose(ActionChoice {
        chosen: Chosen::Mark(mark),
        probabilities: BTreeMap::new(),
        confidence: 0.7,
    })
}

impl ActionJudge for FakeJudge {
    fn choose(&mut self, ask: &ActionAsk) -> Judged {
        self.asked.push(ask.marks().to_vec());
        if self.answers.is_empty() {
            Judged::Refused("timeout".to_string())
        } else {
            self.answers.remove(0)
        }
    }
}

/// A world that answers what the test says and remembers what was done to it —
/// shared with the wire's tests, which put the live judge in front of it.
pub(super) struct FakeWorld {
    screen: Option<Screen>,
    pub(super) presses: Vec<usize>,
    observed: Vec<Option<Value>>,
    press_takes: bool,
    walked_from: Vec<usize>,
    /// The step each re-walk says it stopped at; `None` means it finished.
    walks: Vec<Option<usize>>,
    left_ms: u64,
    /// Whether every press renumbers the screen — a world that moves.
    moves: bool,
    /// What the caller's own condition answers, press by press; a world whose
    /// caller wrote no condition has none.
    reached: Vec<bool>,
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
            reached: Vec::new(),
        }
    }

    /// The same world, whose screen shows different words after every press.
    fn that_moves(marks: &[usize]) -> Self {
        Self {
            moves: true,
            ..Self::showing(marks)
        }
    }
}

impl World for FakeWorld {
    fn look(&mut self) -> Option<Screen> {
        self.screen.clone()
    }
    fn press(&mut self, mark: usize) -> bool {
        self.observed.push(crate::run_evidence::observation());
        self.presses.push(mark);
        if self.press_takes
            && self.moves
            && let Some(screen) = self.screen.as_mut()
        {
            // A screen that moved: same numbers, different words.
            let round = self.presses.len();
            for item in &mut screen.items {
                item["label"] = json!(format!("저장 {round}"));
            }
        }
        self.press_takes
    }

    fn reached(&mut self) -> Option<bool> {
        if self.reached.is_empty() {
            return None;
        }
        Some(self.reached.remove(0))
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

    let mut quiet = FakeJudge::chose(&[2]);
    let recovered = run(
        Mode::Shadow,
        false,
        &stopped(RecipeStop::CheckFailed),
        &mut quiet,
        &mut FakeWorld::showing(&[1, 2]),
    );
    assert!(recovered.rows[0].get(REQUESTS_KEY).is_none());
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
        assert!(
            seat.promotes,
            "{}: a screen seat's auto rises on its own ledger",
            seat.id
        );
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
    let wanted = zerocode_core::jev::promote::window_wanted_for(seat).expect("a screen seat rises");
    let rows: Vec<Value> = (0..wanted as i64)
        .map(|n| answered_press(1_000 + n, true))
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
