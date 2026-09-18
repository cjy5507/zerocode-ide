//! A goal walk reaches a screen only through the roads a step reaches it by,
//! and it never reaches one it was not aimed at.

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

/// A road that answers by verb and remembers every argv it was handed.
struct Road {
    said: RefCell<Vec<(RecipeTool, Vec<String>)>>,
    answer: Box<dyn Fn(&str) -> TeamAnswer>,
}

impl Road {
    fn new(answer: impl Fn(&str) -> TeamAnswer + 'static) -> Self {
        Self {
            said: RefCell::new(Vec::new()),
            answer: Box::new(answer),
        }
    }
    fn road(&self) -> impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer + '_ {
        move |tool, argv, _| {
            self.said.borrow_mut().push((tool, argv.to_vec()));
            (self.answer)(argv.first().map_or("", String::as_str))
        }
    }
    fn argv(&self, at: usize) -> Vec<String> {
        self.said.borrow()[at].1.clone()
    }
    fn tool(&self, at: usize) -> RecipeTool {
        self.said.borrow()[at].0
    }
}

/// What a desktop `observe --marks --json` answers with, through the door's
/// envelope as the shim writes it.
fn desktop_answer() -> String {
    json!({
        "ok": true,
        "result": {
            "tree": { "window": { "title": "채팅" } },
            "marks": {
                "app": "카카오톡",
                "pid": 42,
                "windowId": 7,
                "lookId": "77249.1",
                "items": [
                    { "mark": 1, "role": "button", "label": "보내기", "centerX": 10.0, "centerY": 20.0 },
                ],
            },
        },
    })
    .to_string()
}

/// What a pane's `marks --json` answers with — the bare answer, no envelope.
fn pane_answer() -> String {
    json!({
        "items": [
            { "mark": 3, "role": "link", "label": "다음", "centerX": 30.0, "centerY": 40.0 },
        ],
        "count": 1,
    })
    .to_string()
}

#[test]
fn a_desktop_walk_looks_presses_and_checks_through_the_apps_own_door() {
    let road = Road::new(|verb| match verb {
        "observe" => ok(&desktop_answer()),
        "click" | "wait-for" => ok("{}"),
        _ => refused(),
    });
    let mut road_fn = road.road();
    let mut world = GoalWorld::new(
        &mut road_fn,
        Aim::App {
            name: "카카오톡".into(),
        },
        Seen::default(),
        Some("보냈습니다".into()),
        60_000,
        0,
    );

    let screen = world.look().expect("a marked look is a screen");
    assert_eq!(
        screen.at,
        Seen::Desk {
            app: "카카오톡".into(),
            window: "채팅".into()
        }
    );
    assert_eq!(screen.items.len(), 1);
    assert!(world.press(1));
    assert_eq!(world.reached(), Some(true));

    assert_eq!(road.tool(0), RecipeTool::Computer);
    assert_eq!(
        road.argv(0),
        [
            "observe",
            "--app",
            "카카오톡",
            "--marks",
            "--no-screenshot",
            "--json"
        ]
    );
    // By number, never by point: the pin re-measures what it was handed.
    assert_eq!(
        road.argv(1),
        ["click", "--mark", "1", "--look", "77249.1"],
        "a number is read against the look that drew it, and names no app of its own"
    );
    // The caller's own words, at no wait — the guarded path's own witness.
    assert_eq!(
        road.argv(2),
        [
            "wait-for",
            "--app",
            "카카오톡",
            "--text",
            "보냈습니다",
            "--timeout-ms",
            "0"
        ]
    );
}

#[test]
fn a_pane_walk_takes_the_browser_door_and_keeps_the_address_it_was_handed() {
    let road = Road::new(|verb| match verb {
        "marks" => ok(&pane_answer()),
        "click" | "find" => ok(""),
        _ => refused(),
    });
    let mut road_fn = road.road();
    let page = Seen::Page {
        host: "app.local".into(),
        path: "/inbox".into(),
    };
    let mut world = GoalWorld::new(
        &mut road_fn,
        Aim::Pane {
            label: "browser-5".into(),
        },
        page.clone(),
        Some("보관함".into()),
        60_000,
        0,
    );

    let screen = world.look().expect("a marked look is a screen");
    // A `marks` answer does not carry the address; the walk keeps the one it
    // was handed rather than spending a round trip on it.
    assert_eq!(screen.at, page);
    assert!(world.press(3));
    assert_eq!(world.reached(), Some(true));

    assert_eq!(road.tool(0), RecipeTool::Browser);
    assert_eq!(road.argv(0), ["marks", "browser-5", "--json"]);
    assert_eq!(road.argv(1), ["click", "browser-5", "--mark", "3"]);
    assert_eq!(road.argv(2), ["find", "browser-5", "보관함"]);
}

#[test]
fn a_caller_who_wrote_no_condition_is_asked_nothing_and_is_told_nothing() {
    let road = Road::new(|verb| match verb {
        "observe" => ok(&desktop_answer()),
        _ => ok("{}"),
    });
    let mut road_fn = road.road();
    let mut world = GoalWorld::new(
        &mut road_fn,
        Aim::App { name: "x".into() },
        Seen::default(),
        None,
        60_000,
        0,
    );

    assert_eq!(world.reached(), None, "no condition, no verdict");
    assert!(
        road.said.borrow().is_empty(),
        "and no check verb was asked either"
    );
}

#[test]
fn a_screen_that_will_not_answer_is_not_a_screen() {
    for answer in [refused(), ok("not json"), ok("{}")] {
        let road = Road::new(move |_| answer.clone());
        let mut road_fn = road.road();
        let mut world = GoalWorld::new(
            &mut road_fn,
            Aim::App { name: "x".into() },
            Seen::default(),
            None,
            60_000,
            0,
        );
        assert!(world.look().is_none());
    }
    // A door that refuses the press is a press that did not happen.
    let road = Road::new(|_| refused());
    let mut road_fn = road.road();
    let mut world = GoalWorld::new(
        &mut road_fn,
        Aim::App { name: "x".into() },
        Seen::default(),
        Some("x".into()),
        60_000,
        0,
    );
    assert!(!world.press(1));
    assert_eq!(world.reached(), Some(false));
}

#[test]
fn what_is_left_of_the_calls_clock_counts_what_was_spent_before_the_walk() {
    let road = Road::new(|_| ok("{}"));
    let mut road_fn = road.road();
    let mut world = GoalWorld::new(
        &mut road_fn,
        Aim::App { name: "x".into() },
        Seen::default(),
        None,
        10_000,
        9_000,
    );
    assert!(world.left_ms() <= 1_000);

    let mut road_fn = road.road();
    let mut spent = GoalWorld::new(
        &mut road_fn,
        Aim::App { name: "x".into() },
        Seen::default(),
        None,
        1_000,
        9_000,
    );
    assert_eq!(spent.left_ms(), 0, "a clock past its deadline has nothing");
}
