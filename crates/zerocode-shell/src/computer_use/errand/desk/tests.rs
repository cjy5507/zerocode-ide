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
        // The window's own text beside its controls (t-5497): what the
        // question reads as `shows`, blank lines and all.
        "read" => ok(r#"{"source":"accessibility","text":"채팅\n\n홍길동\n"}"#),
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
    assert_eq!(
        screen.shows,
        ["채팅", "홍길동"],
        "a desktop look reads the window's text beside its controls"
    );
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
    // The look's second call: the window's text, by the same app name.
    assert_eq!(road.argv(1), ["read", "--app", "카카오톡", "--json"]);
    // By number, never by point: the pin re-measures what it was handed.
    assert_eq!(
        road.argv(2),
        ["click", "--mark", "1", "--look", "77249.1"],
        "a number is read against the look that drew it, and names no app of its own"
    );
    // The caller's own words, at no wait — the guarded path's own witness.
    assert_eq!(
        road.argv(3),
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
        "click" => ok(""),
        "find" => ok(r#"{"count":1}"#),
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
fn a_goal_check_does_not_confuse_zero_matches_with_success() {
    let road = Road::new(|_| ok(r#"{"count":0}"#));
    let mut send = road.road();
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Pane {
            label: "page".into(),
        },
        Seen::default(),
        Some("Done".into()),
        60_000,
        0,
    );
    assert_eq!(world.reached(), Some(false));
}

#[test]
fn mobile_goal_uses_the_same_device_and_its_look_for_every_road() {
    for platform in [EmulatorPlatform::Ios, EmulatorPlatform::Android] {
        let road = Road::new(|verb| match verb {
            "marks" => ok(&json!({"ok": true, "result": {
                "lookId": "mobile-look", "items": [{"mark": 2, "role": "button", "label": "일반"}]
            }})
            .to_string()),
            "click" => ok(r#"{"ok":true}"#),
            "find" => ok(r#"{"ok":true,"result":{"count":0}}"#),
            _ => refused(),
        });
        let mut send = road.road();
        let aim = Aim::Phone {
            platform,
            device: "phone".into(),
        };
        let mut world = GoalWorld::new(
            &mut send,
            aim.clone(),
            Seen::default(),
            Some("완료".into()),
            60_000,
            0,
        );
        let screen = world.look().unwrap();
        assert_eq!(
            screen.at,
            Seen::Phone {
                platform,
                device: "phone".into()
            }
        );
        assert_eq!(super::super::seat_of(aim.surface()).id, "emulator");
        assert!(world.press(2));
        assert_eq!(world.reached(), Some(false));
        assert_eq!(road.tool(0), RecipeTool::Emulator);
        assert_eq!(
            road.argv(0),
            [
                "marks",
                "--platform",
                platform.as_str(),
                "--device",
                "phone",
                "--json"
            ]
        );
        assert_eq!(
            road.argv(1),
            [
                "click",
                "--platform",
                platform.as_str(),
                "--device",
                "phone",
                "--json",
                "--mark",
                "2",
                "--look",
                "mobile-look"
            ]
        );
        assert_eq!(
            road.argv(2),
            [
                "find",
                "--platform",
                platform.as_str(),
                "--device",
                "phone",
                "--json",
                "--text",
                "완료"
            ]
        );
        assert!(screen_of(&aim, &json!({"items": []})).is_none());
    }
}

#[test]
fn a_press_cannot_outlive_the_goal_budget_after_its_look() {
    let road = Road::new(|verb| match verb {
        "observe" => ok(&desktop_answer()),
        _ => ok("{}"),
    });
    let mut send = road.road();
    let mut world = GoalWorld::new(
        &mut send,
        Aim::App {
            name: "Settings".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    );
    assert!(world.look().is_some());
    world.spent_ms = world.deadline_ms;
    assert!(!world.press(1));
    assert_eq!(
        road.said.borrow().len(),
        2,
        "an expired goal sends no input — the look's two reads and nothing after"
    );
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
