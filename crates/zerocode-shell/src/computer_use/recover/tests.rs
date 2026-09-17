//! What a stopped walk may do, and what it may not.

use std::collections::BTreeMap;

use super::*;
use zerocode_core::browser_action::GIVE_UP;
use zerocode_core::computer_flow::{Confirm, EvidenceLevel, Fingerprint, Money};

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
    press_takes: bool,
    walked_from: Vec<usize>,
    /// The step each re-walk says it stopped at; `None` means it finished.
    walks: Vec<Option<usize>>,
    left_ms: u64,
}

impl FakeWorld {
    pub(super) fn showing(marks: &[usize]) -> Self {
        Self {
            screen: Some(Screen {
                host: "app.local".into(),
                path: "/settings".into(),
                items: marks.iter().map(|mark| control(*mark, "저장")).collect(),
            }),
            presses: Vec::new(),
            press_takes: true,
            walked_from: Vec::new(),
            walks: vec![None],
            left_ms: 60_000,
        }
    }
}

impl Recovery for FakeWorld {
    fn look(&mut self) -> Option<Screen> {
        self.screen.clone()
    }
    fn press(&mut self, mark: usize) -> bool {
        self.presses.push(mark);
        self.press_takes
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

pub(super) fn stopped(stop: RecipeStop) -> Stopped<'static> {
    Stopped {
        goal: "settings smoke",
        stop,
        step: "click",
        refusal: "the selector matched nothing",
        next: 4,
        flow: None,
        moves_money: false,
    }
}

#[test]
fn off_asks_nothing_presses_nothing_and_writes_nothing() {
    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[1, 2]);

    let recovered = recover(
        Mode::Off,
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
    // `auto` records until something promotes it, and nothing promotes a
    // press: it is held to every word `shadow` is.
    for mode in [Mode::Shadow, Mode::Auto] {
        let mut judge = FakeJudge::chose(&[2]);
        let mut world = FakeWorld::showing(&[1, 2]);

        let recovered = recover(
            mode,
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

    let recovered = recover(
        Mode::Shadow,
        &stopped(RecipeStop::CheckFailed),
        &mut judge,
        &mut world,
    );

    assert_eq!(recovered.rows[0][REQUESTS_KEY], json!(1));
    assert_eq!(recovered.rows[0][REDACTED_LINES_KEY], json!(2));

    let mut quiet = FakeJudge::chose(&[2]);
    let recovered = recover(
        Mode::Shadow,
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

    let recovered = recover(
        Mode::On,
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

    let recovered = recover(
        Mode::On,
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

    let recovered = recover(
        Mode::On,
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

    let recovered = recover(
        Mode::On,
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

        let recovered = recover(
            Mode::On,
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

    let recovered = recover(
        Mode::On,
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

        recover(Mode::On, &stopped(stop), &mut judge, &mut world);

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
    let recovered = recover(Mode::On, &moneyed, &mut judge, &mut world);

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
    let recovered = recover(Mode::On, &guarded, &mut judge, &mut world);
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
    let deadline = u64::try_from(BROWSER_ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    assert_eq!(
        barred(Mode::On, &stopped(RecipeStop::StepFailed), deadline),
        Some(Barred::NoBudget)
    );
    assert!(barred(Mode::On, &stopped(RecipeStop::StepFailed), deadline + 1).is_none());

    let mut judge = FakeJudge::chose(&[1]);
    let mut world = FakeWorld::showing(&[1]);
    world.left_ms = deadline;
    let recovered = recover(
        Mode::On,
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
    let recovered = recover(
        Mode::On,
        &stopped(RecipeStop::StepFailed),
        &mut judge,
        &mut blind,
    );
    assert_eq!(recovered.rows[0]["outcome"], json!("no_look"));
    assert!(judge.asked.is_empty() && blind.presses.is_empty());

    let mut judge = FakeJudge::chose(&[1]);
    let mut bare = FakeWorld::showing(&[]);
    let recovered = recover(
        Mode::On,
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
    assert!(
        !Mode::Auto.applies(),
        "nothing promotes a press: auto never presses"
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
