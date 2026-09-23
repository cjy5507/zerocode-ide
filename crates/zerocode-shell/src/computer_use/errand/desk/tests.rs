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
                "mobile-look",
                "--text",
                "완료"
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

/// A press that waited for its screen to stop changing (an iOS click,
/// t-6385) is checked in the look the next step takes: one `marks --text`
/// counts the caller's words by the check's own rule and is kept, the next
/// look takes no road, and the next press goes out against the kept look.
#[test]
fn a_settled_press_is_checked_in_the_look_the_next_step_takes() {
    let settle = json!({ "ms": 1_080, "reads": 18, "settle": "still" });
    let answer_settle = settle.clone();
    let road = Road::new(move |verb| match verb {
        "marks" => ok(&json!({"ok": true, "result": {
            "lookId": "after-press", "count": 0,
            "items": [{"mark": 1, "role": "button", "label": "정보"}]
        }})
        .to_string()),
        "click" => ok(&json!({"ok": true, "result": {
            "performed": true, "settle": answer_settle
        }})
        .to_string()),
        _ => refused(),
    });
    let mut send = road.road();
    let aim = Aim::Phone {
        platform: EmulatorPlatform::Ios,
        device: "phone".into(),
    };
    let mut world = GoalWorld::new(
        &mut send,
        aim,
        Seen::default(),
        Some("iOS 버전".into()),
        60_000,
        0,
    );
    world.look().unwrap();
    assert!(world.press(1));
    assert_eq!(
        world.settled(),
        Some(Settled {
            note: settle,
            screen: None
        })
    );
    assert_eq!(world.reached(), Some(false));
    let calls = || road.said.borrow().len();
    assert_eq!(
        road.argv(2),
        [
            "marks",
            "--platform",
            "ios",
            "--device",
            "phone",
            "--json",
            "--text",
            "iOS 버전"
        ],
        "the reach check is the next look, counting"
    );
    let next = world.look().unwrap();
    assert_eq!(calls(), 3, "the next look took no road");
    assert_eq!(next.items[0]["label"], json!("정보"));
    assert!(world.press(1));
    let press = road.argv(3);
    let look = press.iter().position(|word| word == "--look").unwrap();
    assert_eq!(
        press[look + 1],
        "after-press",
        "the press goes out against the kept look"
    );
    assert_eq!(press[press.len() - 2..], ["--text", "iOS 버전"]);
}

/// A press that counted the caller's words in the tree it settled on ends the
/// walk on that count: nothing is looked at again (t-6385).
#[test]
fn a_settled_press_that_counted_the_words_ends_the_walk_without_a_look() {
    let road = Road::new(|verb| match verb {
        "click" => ok(&json!({"ok": true, "result": {
            "settle": { "ms": 900, "reads": 15, "settle": "still" }, "count": 1
        }})
        .to_string()),
        "marks" => ok(&json!({"ok": true, "result": {
            "lookId": "first", "items": [{"mark": 1, "role": "button", "label": "정보"}]
        }})
        .to_string()),
        _ => refused(),
    });
    let mut send = road.road();
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Phone {
            platform: EmulatorPlatform::Ios,
            device: "phone".into(),
        },
        Seen::default(),
        Some("iOS 버전".into()),
        60_000,
        0,
    );
    world.look().unwrap();
    assert!(world.press(1));
    assert_eq!(world.reached(), Some(true));
    assert_eq!(
        road.said.borrow().len(),
        2,
        "a look and a press, nothing more"
    );
}

/// A press asked for a preview (a walk asking ahead, t-6385) hands back the
/// screen it settled on, numbered as a look of it would be; a phone's world
/// never asks ahead of its press, a page's does as before.
#[test]
fn a_press_asked_for_a_preview_hands_back_the_screen_it_settled_on() {
    let road = Road::new(|verb| match verb {
        "marks" => ok(&json!({"ok": true, "result": {
            "lookId": "first", "items": [{"mark": 3, "role": "button", "label": "일반"}]
        }})
        .to_string()),
        "click" => ok(&json!({"ok": true, "result": {
            "settle": { "ms": 740, "reads": 3, "settle": "still" },
            "preview": {
                "items": [{"mark": 2, "role": "button", "label": "정보"}],
                "legend": "2 button 정보 @201,396"
            }
        }})
        .to_string()),
        _ => refused(),
    });
    let mut send = road.road();
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Phone {
            platform: EmulatorPlatform::Ios,
            device: "phone".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .previewing(true);
    assert!(!world.asks_ahead_of_the_press());
    world.look().unwrap();
    assert!(world.press(3));
    assert_eq!(road.argv(1).last().map(String::as_str), Some("--preview"));
    let screen = world.settled().and_then(|settled| settled.screen).unwrap();
    assert_eq!(
        screen.at,
        Seen::Phone {
            platform: EmulatorPlatform::Ios,
            device: "phone".into()
        }
    );
    assert_eq!(screen.items[0]["label"], json!("정보"));

    let road = Road::new(|_| refused());
    let mut send = road.road();
    let page = GoalWorld::new(
        &mut send,
        Aim::Pane {
            label: "main".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .previewing(true);
    assert!(page.asks_ahead_of_the_press());
}

/// The count a look answers decides the check as `find`'s did.
#[test]
fn a_settled_press_that_reached_the_words_ends_on_the_looks_own_count() {
    let road = Road::new(|verb| match verb {
        "marks" => ok(&json!({"ok": true, "result": {
            "lookId": "about", "count": 1, "items": []
        }})
        .to_string()),
        "click" => ok(&json!({"ok": true, "result": {
            "settle": { "ms": 900, "reads": 15, "settle": "still" }
        }})
        .to_string()),
        _ => refused(),
    });
    let mut send = road.road();
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Phone {
            platform: EmulatorPlatform::Ios,
            device: "phone".into(),
        },
        Seen::default(),
        Some("iOS 버전".into()),
        60_000,
        0,
    );
    world.look().unwrap();
    assert!(world.press(1));
    assert_eq!(world.reached(), Some(true));
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
