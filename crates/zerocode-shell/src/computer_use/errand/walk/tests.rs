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
