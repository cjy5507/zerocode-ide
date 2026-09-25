//! A recovery reaches the browser only through the roads a step reaches it
//! by: the pane's `marks`, `click --mark <n>`, and the caller's own round.

use std::cell::RefCell;

use serde_json::json;
use zerocode_hookd::TeamAnswer;

use super::*;

fn ok(stdout: &str) -> TeamAnswer {
    TeamAnswer {
        exit_code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
    }
}

fn refused() -> TeamAnswer {
    TeamAnswer {
        exit_code: 1,
        stdout: String::new(),
        stderr: "the pin no longer holds".to_string(),
    }
}

/// What a `marks --json` answer looks like on the wire.
fn marks_answer() -> String {
    json!({
        "items": [
            { "mark": 1, "role": "button", "label": "다시 시도", "centerX": 10.0, "centerY": 20.0 },
            { "mark": 2, "role": "button", "label": "닫기", "centerX": 30.0, "centerY": 20.0 },
        ],
        "count": 2,
    })
    .to_string()
}

/// A road that answers by verb and remembers every argv it was handed.
struct Road {
    sent: RefCell<Vec<Vec<String>>>,
    marks: TeamAnswer,
    click: TeamAnswer,
}

impl Road {
    fn new() -> Self {
        Self {
            sent: RefCell::new(Vec::new()),
            marks: ok(&marks_answer()),
            click: ok("{}"),
        }
    }
    fn drive(&self) -> impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer + '_ {
        move |_tool, argv, _logged| {
            self.sent.borrow_mut().push(argv.to_vec());
            match argv.first().map(String::as_str) {
                Some("marks") => self.marks.clone(),
                _ => self.click.clone(),
            }
        }
    }
    fn sent(&self) -> Vec<Vec<String>> {
        self.sent.borrow().clone()
    }
}

/// What a desk answers for `pages()`.
fn pages(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(label, url)| ((*label).to_string(), (*url).to_string()))
        .collect()
}

#[test]
fn the_page_address_is_the_desks_and_never_carries_a_query() {
    let desk = pages(&[
        ("app", "https://shop.example/cart?token=secret&id=9"),
        ("other", "https://elsewhere.test/x"),
    ]);

    let (host, path) = page_of(&desk, "app");
    assert_eq!((host.as_str(), path.as_str()), ("shop.example", "/cart"));

    // A pane the desk does not know, and a url it cannot read, are both blank
    // rather than a guess: the address is context, and a wrong one is worse.
    assert_eq!(page_of(&desk, "missing"), (String::new(), String::new()));
    let bad = pages(&[("app", "not a url")]);
    assert_eq!(page_of(&bad, "app"), (String::new(), String::new()));
}

#[test]
fn a_look_is_the_panes_own_marks_and_carries_the_address_it_was_given() {
    let road = Road::new();
    let mut drive = road.drive();
    let mut walk = |_: &mut _, _: u64, _: usize| None;
    let mut world = WalkWorld::new(
        &mut drive,
        &mut walk,
        "app",
        ("shop.example".into(), "/cart".into()),
        60_000,
        0,
    );

    let screen = world.look().expect("the pane answered its marks");

    assert_eq!(screen.items.len(), 2);
    assert_eq!(screen.items[0]["mark"], json!(1));
    assert_eq!(
        screen.at,
        Seen::Page {
            host: "shop.example".into(),
            path: "/cart".into()
        }
    );
    assert_eq!(road.sent(), [["marks", "app", "--json"]]);
}

#[test]
fn a_press_names_a_number_and_never_a_selector() {
    let road = Road::new();
    let mut drive = road.drive();
    let mut walk = |_: &mut _, _: u64, _: usize| None;
    let mut world = WalkWorld::new(
        &mut drive,
        &mut walk,
        "app",
        (String::new(), String::new()),
        60_000,
        0,
    );

    assert!(world.press(7));

    let sent = road.sent();
    assert_eq!(sent, [["click", "app", "--mark", "7"]]);
    assert!(
        !sent[0]
            .iter()
            .any(|word| word.contains('#') || word.contains('.')),
        "a recovery presses by number, so no selector can ride with it: {sent:?}"
    );
}

#[test]
fn a_door_that_refuses_is_a_press_that_did_not_land_and_a_look_that_saw_nothing() {
    let mut road = Road::new();
    road.click = refused();
    road.marks = refused();
    let mut drive = road.drive();
    let mut walk = |_: &mut _, _: u64, _: usize| None;
    let mut world = WalkWorld::new(
        &mut drive,
        &mut walk,
        "app",
        (String::new(), String::new()),
        60_000,
        0,
    );

    assert!(!world.press(3), "a refused press is false, not a panic");
    assert!(
        world.look().is_none(),
        "a refused look is nothing to choose from"
    );
}

#[test]
fn a_marks_answer_that_is_not_one_is_nothing_to_choose_from() {
    for said in ["not json", "{}", r#"{"items":"lots"}"#] {
        let mut road = Road::new();
        road.marks = ok(said);
        let mut drive = road.drive();
        let mut walk = |_: &mut _, _: u64, _: usize| None;
        let mut world = WalkWorld::new(
            &mut drive,
            &mut walk,
            "app",
            (String::new(), String::new()),
            60_000,
            0,
        );

        assert!(world.look().is_none(), "this answer is not a look: {said}");
    }
}

#[test]
fn walking_again_hands_the_round_the_step_and_what_is_left_of_the_call() {
    let road = Road::new();
    let mut drive = road.drive();
    let asked = RefCell::new(Vec::new());
    let mut walk = |_: &mut _, left: u64, step: usize| {
        asked.borrow_mut().push((left, step));
        Some(json!({ "done": true }))
    };
    // The first walk already spent 40 s of the call's 60 s.
    let mut world = WalkWorld::new(
        &mut drive,
        &mut walk,
        "app",
        (String::new(), String::new()),
        60_000,
        40_000,
    );

    let report = world.walk_from(4).expect("the round answered");

    assert_eq!(report["done"], json!(true));
    let asked = asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, 4, "the step is the report's own next");
    assert!(
        asked[0].0 <= 20_000 && asked[0].0 > 19_000,
        "the round gets what is left of the call, not the whole of it: {} ms",
        asked[0].0
    );
}

#[test]
fn a_call_whose_clock_is_gone_has_nothing_left() {
    let road = Road::new();
    let mut drive = road.drive();
    let mut walk = |_: &mut _, _: u64, _: usize| None;
    let mut world = WalkWorld::new(
        &mut drive,
        &mut walk,
        "app",
        (String::new(), String::new()),
        10_000,
        10_000,
    );

    assert_eq!(
        world.left_ms(),
        0,
        "spent past the deadline is zero, never a wrap"
    );
}

/// A walk asks its second reader (t-6132 S3) only when its caller turned the
/// rung on, and only for a press: a normal walk — the rescue switch off —
/// never starts one, whatever its seat said; turned on, it asks one only for
/// a press under the seat's floor, hands it no more than the call has left,
/// and presses its answer only at that same floor. An entry under the floor
/// is never a second reader's to rescue (t-6720): the reader answers one
/// option and names no field. Where the reader must NOT start, it is the real
/// one over a zo that is a script — started or not, its own record says —
/// and the row carries no word from it either.
#[test]
fn a_normal_walk_does_not_start_the_second_reader() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use zerocode_core::computer_recipe::RecipeStop;
    use zerocode_core::screen_action::ActionAsk;

    use super::super::team::tests::{fake_zo, judge_over};
    use super::super::tests::{FakeJudge, entry, pick, stopped};
    use super::super::{ActionJudge, Branching, Judged, Mode, Options, RESCUE, Walked, run_with};

    /// A second reader that says what the test says and keeps the wall it
    /// was handed each time it was asked.
    struct Reader {
        says: Judged,
        walls: Rc<RefCell<Vec<Duration>>>,
    }
    impl ActionJudge for Reader {
        fn choose(&mut self, ask: &ActionAsk) -> Judged {
            self.choose_within(ask, Duration::MAX)
        }
        fn choose_within(&mut self, _: &ActionAsk, left: Duration) -> Judged {
            self.walls.borrow_mut().push(left);
            self.says.clone()
        }
    }

    let unsure = |mut judged: Judged| {
        if let Judged::Chose(read) = &mut judged {
            read.choice.confidence = 0.29;
        }
        judged
    };
    let at = |mut judged: Judged, confidence: f64| {
        if let Judged::Chose(read) = &mut judged {
            read.choice.confidence = confidence;
        }
        judged
    };
    // One stopped walk on the walk road, with `second` as its reader:
    // every argv the road was handed, and the walk.
    let walk_once = |rescue: bool, seat: Judged, second: &mut dyn ActionJudge, clock_ms: u64| {
        let road = Road::new();
        let mut drive = road.drive();
        let mut walk = |_: &mut _, _: u64, _: usize| None;
        let mut world = WalkWorld::new(
            &mut drive,
            &mut walk,
            "app",
            ("app.local".to_string(), "/cart".to_string()),
            clock_ms,
            0,
        );
        let mut judge = FakeJudge::saying(vec![seat]);
        let walked: Walked = run_with(
            Mode::On,
            true,
            Branching::OFF,
            &stopped(RecipeStop::StepFailed),
            &mut judge,
            &mut world,
            Options {
                overlap: false,
                rescue,
                act_line: None,
            },
            Some(second),
        );
        drop(world);
        (road.sent(), walked)
    };
    let clicks = |sent: &[Vec<String>], mark: &str| {
        sent.iter()
            .filter(|argv| argv[0] == "click" && argv.last().map(String::as_str) == Some(mark))
            .count()
    };

    // Where the reader must not start, the real one: its record stays empty.
    let root = tempfile::tempdir().expect("a root");
    let (program, _) = fake_zo(root.path(), "", "0");
    for (name, rescue, seat) in [
        ("a normal walk", false, unsure(pick(1))),
        ("a sure press", true, pick(1)),
        ("an entry under the floor", true, unsure(entry(1))),
    ] {
        let record = tempfile::tempdir().expect("a record");
        let mut team = judge_over(
            program.clone(),
            record.path(),
            r#"{"choice":"mark:2","confidence":0.95}"#,
            None,
        );
        let (sent, walked) = walk_once(rescue, seat, &mut team, 60_000);
        assert!(
            !record.path().join("argv").exists(),
            "{name} started the second reader"
        );
        assert!(
            walked.rows[0].get(RESCUE).is_none(),
            "{name} asked the second reader"
        );
        assert_eq!((walked.rescued, walked.rescue_failed), (0, 0), "{name}");
        if name == "a sure press" {
            assert_eq!(clicks(&sent, "1"), 1, "the seat's own press");
        } else {
            assert_eq!(clicks(&sent, "1") + clicks(&sent, "2"), 0, "{name} pressed");
            assert_eq!(walked.rows[0]["barred"], json!("low_confidence"), "{name}");
        }
    }

    // Turned on, an unsure press asks the reader once, with no more than the
    // call has left; its answer under the same floor presses nothing, and one
    // at the floor presses the reader's own number.
    let walls = Rc::new(RefCell::new(Vec::new()));
    let mut under = Reader {
        says: at(pick(2), 0.4),
        walls: Rc::clone(&walls),
    };
    let (sent, walked) = walk_once(true, unsure(pick(1)), &mut under, 3_000);
    assert_eq!(walls.borrow().len(), 1, "asked once");
    assert!(
        walls.borrow()[0] <= Duration::from_millis(3_000),
        "the reader was handed more than the call had: {:?}",
        walls.borrow()[0]
    );
    assert_eq!(clicks(&sent, "1") + clicks(&sent, "2"), 0);
    assert_eq!(walked.rows[0][RESCUE]["outcome"], json!("low_confidence"));
    assert_eq!(walked.rescue_failed, 1);
    let mut sure = Reader {
        says: at(pick(2), 0.95),
        walls: Rc::new(RefCell::new(Vec::new())),
    };
    let (sent, walked) = walk_once(true, unsure(pick(1)), &mut sure, 60_000);
    assert_eq!((clicks(&sent, "2"), walked.rescued), (1, 1));
}
