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
/// never asks ahead of its press, and neither does a page's that asks ahead
/// (t-9712): it asks on the page its press changed. A window's still does.
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
    assert!(!page.asks_ahead_of_the_press());
    drop(page);

    let road = Road::new(|_| refused());
    let mut send = road.road();
    let window = GoalWorld::new(
        &mut send,
        Aim::App {
            name: "계산기".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .previewing(true);
    assert!(window.asks_ahead_of_the_press());
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

// ---- Entering a written value into a field the look read (t-6720) ----

use std::sync::Arc;

use zerocode_core::agent_browser::TYPE_VALUE_FLAG;
use zerocode_core::screen_action::{ActionChoice, Chosen, Guard};

use super::super::tests::{FakeJudge, Pen, entry, goal, memory, pick};
use super::super::value::Written;
use super::super::{Judged, Mode, TYPED, Typed, ValueSource, run};

/// The one field a page's look read — a city box — and a button beside it,
/// with what the page read beside its numbers in the same pass (U4's
/// snapshot): the document, and the field's own words and value.
fn a_page_with_a_field(epoch: &str, now: &str, selector: &str) -> String {
    json!({
        "items": [
            { "mark": 1, "role": "textbox", "tag": "input", "label": "City",
              "selector": selector, "centerX": 120.0, "centerY": 40.0 },
            { "mark": 2, "role": "button", "tag": "button", "label": "Advance",
              "selector": "#advance", "centerX": 300.0, "centerY": 40.0 },
        ],
        "count": 2,
        (zerocode_core::screen_action::snapshot::EPOCH_KEY): epoch,
        (zerocode_core::screen_action::snapshot::FIELDS_KEY): [{
            "mark": 1, "kind": "text", "secret": false,
            "label": "Destination", "placeholder": "City",
            "near": "Travel search", "value": now,
        }],
    })
    .to_string()
}

/// One call a road was handed: the tool, the argv it ran, the argv it logged.
type Call = (RecipeTool, Vec<String>, Vec<String>);

/// A road that keeps what it was handed to run AND what it was handed to
/// log, and answers from what the test's page says now.
struct Kept {
    calls: RefCell<Vec<Call>>,
    page: RefCell<String>,
}

impl Kept {
    fn on(page: String) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            page: RefCell::new(page),
        }
    }
    fn road(&self) -> impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer + '_ {
        move |tool, argv, logged| {
            self.calls
                .borrow_mut()
                .push((tool, argv.to_vec(), logged.to_vec()));
            match argv.first().map(String::as_str) {
                Some("marks") => ok(&self.page.borrow()),
                Some("click" | "type") => ok("{}"),
                Some("find") => ok(r#"{"count":0}"#),
                _ => refused(),
            }
        }
    }
    fn verbs(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|(_, argv, _)| argv[0].clone())
            .collect()
    }
}

/// The value seat is asked only when the walk's judgment chose to TYPE
/// and the walk goes on to type: a press, `give_up` and `done` write
/// nothing — there is no scroll operation to ask about — and neither does
/// a seat that only records, a guard that stops the step, or an entry under
/// the press floor. An entry writes once.
#[test]
fn only_type_text_calls_the_value_generator() {
    let walk = |answer: Judged, acting: bool| -> (usize, Vec<String>) {
        let road = Kept::on(a_page_with_a_field("doc-1", "", "#destination"));
        let mut send = road.road();
        let (pen, writes, _) = Pen::writing("London");
        let mut world = GoalWorld::new(
            &mut send,
            Aim::Pane {
                label: "browser-9".into(),
            },
            Seen::default(),
            None,
            60_000,
            0,
        )
        .writing(Box::new(pen))
        .remembering(memory());
        let mut judge = FakeJudge::saying(vec![answer]);
        let _ = run(Mode::On, acting, &goal(1), &mut judge, &mut world);
        drop(world);
        (writes.get(), road.verbs())
    };
    let ended = |chosen: Chosen| {
        Judged::Chose(
            ActionChoice {
                chosen,
                probabilities: std::collections::BTreeMap::new(),
                confidence: 0.9,
                guard: None,
            }
            .into(),
        )
    };
    for (name, answer) in [
        ("click", pick(2)),
        ("give_up", ended(Chosen::GiveUp)),
        ("done", ended(Chosen::Done)),
    ] {
        let (writes, verbs) = walk(answer, true);
        assert_eq!(writes, 0, "{name} wrote a value");
        assert!(!verbs.iter().any(|verb| verb == "type"), "{name} typed");
    }
    let (writes, verbs) = walk(entry(1), true);
    assert_eq!(writes, 1, "an entry writes once");
    assert_eq!(verbs, ["marks", "click", "type"]);

    // A seat that only records, an entry under the floor and an entry on a
    // screen whose guard stops it write nothing and type nothing.
    let (writes, verbs) = walk(entry(1), false);
    assert_eq!((writes, verbs.len()), (0, 1), "a recording seat");
    let mut unsure = entry(1);
    if let Judged::Chose(read) = &mut unsure {
        read.choice.confidence = 0.29;
    }
    let (writes, verbs) = walk(unsure, true);
    assert_eq!((writes, verbs.len()), (0, 1), "under the floor");
    let mut walled = entry(1);
    if let Judged::Chose(read) = &mut walled {
        read.choice.guard = Some(Guard {
            instructed: 0.0,
            walled: 0.95,
        });
    }
    let (writes, verbs) = walk(walled, true);
    assert_eq!((writes, verbs.len()), (0, 1), "a wall");
}

/// What makes a second write the same write: the same goal, the same words
/// around the same field holding the same value, in the same document. A
/// retry and a replay of that write type the value from memory with no
/// model asked; a changed document, value, field or goal asks afresh, and a
/// look that named no document never reuses anything.
#[test]
fn identical_value_input_reuses_but_changed_document_or_value_does_not() {
    let road = Kept::on(a_page_with_a_field("doc-1", "", "#destination"));
    let mut send = road.road();
    let (pen, writes, _) = Pen::writing("London");
    let values = memory();
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Pane {
            label: "browser-9".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .writing(Box::new(pen))
    .remembering(Arc::clone(&values));
    let goal = "Set the destination to London";
    let enter = |world: &mut GoalWorld<'_, _>, page: String, goal: &str| {
        *road.page.borrow_mut() = page;
        world.look().expect("a look");
        world.type_into(1, goal)
    };

    let first = enter(
        &mut world,
        a_page_with_a_field("doc-1", "", "#destination"),
        goal,
    );
    assert!(
        matches!(
            &first,
            Typed::Typed {
                source: ValueSource::Written { .. },
                chars: 6
            }
        ),
        "{first:?}"
    );
    assert_eq!(writes.get(), 1);
    // The same input again — a retry: typed from memory, no write.
    let retried = enter(
        &mut world,
        a_page_with_a_field("doc-1", "", "#destination"),
        goal,
    );
    assert_eq!(
        retried,
        Typed::Typed {
            source: ValueSource::Reused,
            chars: 6
        }
    );
    assert_eq!(writes.get(), 1, "a retry of the same input writes nothing");

    // Anything that changed asks afresh.
    let changed = [
        (
            "another document",
            a_page_with_a_field("doc-2", "", "#destination"),
            goal,
        ),
        (
            "another value in the field",
            a_page_with_a_field("doc-2", "Lon", "#destination"),
            goal,
        ),
        (
            "another field",
            a_page_with_a_field("doc-2", "Lon", "#origin"),
            goal,
        ),
        (
            "another goal",
            a_page_with_a_field("doc-2", "Lon", "#origin"),
            "Set the destination to Paris",
        ),
    ];
    for (at, (why, page, goal)) in changed.into_iter().enumerate() {
        let typed = enter(&mut world, page, goal);
        assert!(
            matches!(
                typed,
                Typed::Typed {
                    source: ValueSource::Written { .. },
                    ..
                }
            ),
            "{why}: {typed:?}"
        );
        assert_eq!(writes.get(), 2 + at, "{why} is a new write");
    }
    // A look that named no document is never vouched for.
    for _ in 0..2 {
        let before = writes.get();
        let typed = enter(
            &mut world,
            a_page_with_a_field("", "", "#destination"),
            goal,
        );
        assert!(matches!(
            typed,
            Typed::Typed {
                source: ValueSource::Written { .. },
                ..
            }
        ));
        assert_eq!(writes.get(), before + 1, "no document, no reuse");
    }
    drop(world);

    // A replay — another walk, the same window's memory, the same input —
    // types the first write's value without asking.
    let (pen, rewrites, _) = Pen::writing("never asked");
    let replay_road = Kept::on(a_page_with_a_field("doc-1", "", "#destination"));
    let mut replay_send = replay_road.road();
    let mut replay = GoalWorld::new(
        &mut replay_send,
        Aim::Pane {
            label: "browser-9".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .writing(Box::new(pen))
    .remembering(values);
    replay.look().expect("a look");
    assert_eq!(
        replay.type_into(1, goal),
        Typed::Typed {
            source: ValueSource::Reused,
            chars: 6
        }
    );
    assert_eq!(
        rewrites.get(),
        0,
        "a replay of the same input writes nothing"
    );
    let typed_argv = replay_road
        .calls
        .borrow()
        .iter()
        .find(|(_, argv, _)| argv[0] == "type")
        .map(|(_, argv, _)| argv.clone())
        .expect("typed");
    assert_eq!(typed_argv[4], "London", "the value the first write wrote");
}

/// A written value reaches the page down the door's value road — `type
/// <pane> <selector> --value`, the shape the stdin road sends — into the
/// field the look itself named, pressed first by its pinned number. It is
/// never in what the log keeps (`[6 chars]` in its place), never on a
/// walk's row, never in what a writer's debug line prints, and the model is
/// asked about the words around the box, never the box's value.
#[test]
fn generated_value_reaches_stdin_and_is_never_in_argv_or_logs() {
    let road = Kept::on(a_page_with_a_field("doc-1", "Zur", "#destination"));
    let mut send = road.road();
    let (pen, writes, asked) = Pen::writing("London");
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Pane {
            label: "browser-9".into(),
        },
        Seen::default(),
        Some("Arrived".into()),
        60_000,
        0,
    )
    .writing(Box::new(pen))
    .remembering(memory());
    let mut judge = FakeJudge::saying(vec![entry(1)]);
    let walked = run(Mode::On, true, &goal(1), &mut judge, &mut world);
    drop(world);

    assert_eq!(writes.get(), 1);
    assert_eq!(walked.typed, 1);
    let calls = road.calls.borrow();
    let verbs: Vec<&str> = calls.iter().map(|(_, argv, _)| argv[0].as_str()).collect();
    assert_eq!(verbs, ["marks", "click", "type", "find"]);
    // The field is pressed by its pinned number, then typed by the look's
    // own selector for it — nothing composed.
    assert_eq!(calls[1].1, ["click", "browser-9", "--mark", "1"]);
    assert_eq!(
        calls[2].1,
        [
            "type",
            "browser-9",
            "#destination",
            TYPE_VALUE_FLAG,
            "London"
        ],
        "the value rides the door's value slot, the stdin road's shape"
    );
    assert_eq!(
        calls[2].2,
        [
            "type",
            "browser-9",
            "#destination",
            TYPE_VALUE_FLAG,
            "[6 chars]"
        ],
        "the log keeps the flag and hides the value"
    );
    for (_, _, logged) in calls.iter() {
        assert!(
            !logged.iter().any(|word| word.contains("London")),
            "a logged line holds the value: {logged:?}"
        );
    }
    let rows = serde_json::to_string(&walked.rows).expect("rows");
    assert!(!rows.contains("London"), "a row holds the value: {rows}");
    assert!(
        !rows.contains("Zur"),
        "a row holds the field's value: {rows}"
    );
    assert_eq!(walked.rows[0][TYPED]["source"], json!("written"));
    assert_eq!(walked.rows[0][TYPED]["chars"], json!(6));
    // The model was asked about the words around the box — never what the
    // box held.
    let question = asked.borrow().join("\n");
    assert!(question.contains("Destination") && question.contains("Travel search"));
    assert!(!question.contains("Zur"), "{question}");
    let written = Written {
        value: "London".into(),
        model: "m".into(),
        ms: 1,
    };
    assert!(!format!("{written:?}").contains("London"));
}

/// The value seat's road is opened once a walk's look has read a field — on
/// that first look, while the judgment is still to be asked — and only then:
/// a look of numbers alone, a world that cannot type and a second look warm
/// nothing.
#[test]
fn the_first_look_that_reads_a_field_opens_the_value_road_once() {
    struct Warmed(Rc<Cell<usize>>);
    impl super::super::value::ValueWriter for Warmed {
        fn row(&self) -> Option<&'static zerocode_core::type_value::ValueRow> {
            zerocode_core::type_value::chosen()
        }
        fn ready(&self) -> bool {
            true
        }
        fn write(
            &mut self,
            _: &zerocode_core::type_value::FieldLook<'_>,
            _: std::time::Duration,
        ) -> Result<Written, String> {
            Err("unused".to_string())
        }
        fn warm(&self) {
            self.0.set(self.0.get() + 1);
        }
    }
    use std::cell::Cell;
    use std::rc::Rc;

    let warm_after = |page: String, aim: Aim, looks: usize| {
        let road = Kept::on(page);
        let mut send = road.road();
        let warms = Rc::new(Cell::new(0));
        let mut world = GoalWorld::new(&mut send, aim, Seen::default(), None, 60_000, 0)
            .writing(Box::new(Warmed(Rc::clone(&warms))))
            .remembering(memory());
        for _ in 0..looks {
            let _ = world.look();
        }
        warms.get()
    };
    let pane = || Aim::Pane {
        label: "browser-9".into(),
    };
    assert_eq!(
        warm_after(a_page_with_a_field("doc-1", "", "#destination"), pane(), 3),
        1
    );
    assert_eq!(
        warm_after(pane_answer(), pane(), 2),
        0,
        "no field, no warm-up"
    );
    assert_eq!(
        warm_after(
            a_page_with_a_field("doc-1", "", "#destination"),
            Aim::App { name: "x".into() },
            1
        ),
        0,
        "a world that cannot type"
    );
}

/// A field takes one entry in a walk: once a value went into it, the next
/// question offers no entry into that field again — the entry changed the
/// words the field is read by, so the screen reads as moved and would offer
/// it afresh, and a second guess at it is a loop, not a step — while its
/// press and every other control stay on offer.
#[test]
fn a_field_takes_one_entry_a_walk() {
    // The page as a real one answers: after the entry the field is read by
    // what it now holds.
    let typed = std::cell::Cell::new(false);
    let mut send = |_: RecipeTool, argv: &[String], _: &[String]| match argv[0].as_str() {
        "marks" if typed.get() => ok(&a_page_with_a_field("doc-1", "London", "#destination")
            .replace("\"label\":\"City\"", "\"label\":\"London\"")),
        "marks" => ok(&a_page_with_a_field("doc-1", "", "#destination")),
        "type" => {
            typed.set(true);
            ok("{}")
        }
        _ => ok("{}"),
    };
    let (pen, writes, _) = Pen::writing("London");
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Pane {
            label: "browser-9".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .writing(Box::new(pen))
    .remembering(memory());
    let mut judge = FakeJudge::saying(vec![entry(1), pick(2)]);
    let walked = run(Mode::On, true, &goal(2), &mut judge, &mut world);
    drop(world);
    assert_eq!((walked.typed, walked.pressed, writes.get()), (1, 2, 1));
    let [first, second] = judge.questions.as_slice() else {
        panic!("two questions: {:?}", judge.questions);
    };
    assert!(
        first.get("type_target").is_some(),
        "the first look may type"
    );
    assert!(first["action"]["criteria"].get("type_text").is_some());
    assert!(
        second["action"]["criteria"]["mark:1"]
            .as_str()
            .is_some_and(|line| line.contains("London")),
        "the screen moved: the field is read by what it holds, and its press is on offer"
    );
    assert!(second.get("type_target").is_none(), "the field was entered");
    assert!(second["action"]["criteria"].get("type_text").is_none());
    assert!(second["action"]["criteria"].get("mark:2").is_some());
}

/// A subscription login is on no request the value seat makes (t-6720, the
/// coordinator's decision m-9526): on a machine whose key store holds only a
/// subscription login, a look that read a field offers no entry — Type
/// candidates 0 — and the value seat asks nothing, even of a judgment that
/// answers with an entry regardless; with a key a person set, the same look
/// offers the entry and the value goes in down the value road, asked with
/// that key alone.
/// One walk over a page with a field, as the window walks one: a writer of
/// its own over `keys`, its value seat on an endpoint of its own, and a
/// judgment that answers with an entry whether or not one was offered — the
/// questions it was asked, what the value seat heard, the verbs the page was
/// sent, and the walk.
fn walk_a_field(
    keys: Box<dyn crate::api_routers::RouterKeys>,
    epoch: &str,
) -> (Vec<Value>, Vec<String>, Vec<String>, super::super::Walked) {
    use super::super::value::LiveWriter;
    use super::super::value::tests::wrote;
    use crate::systemone::tests::Endpoint;

    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let road = Kept::on(a_page_with_a_field(epoch, "", "#destination"));
    let mut send = road.road();
    let writer = LiveWriter::at(&format!("{}/v1/messages", endpoint.base()), keys);
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Pane {
            label: "browser-9".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .writing(Box::new(writer))
    .remembering(memory());
    let mut judge = FakeJudge::saying(vec![entry(1)]);
    let walked = run(Mode::On, true, &goal(1), &mut judge, &mut world);
    drop(world);
    (judge.questions, endpoint.asked(), road.verbs(), walked)
}

#[test]
fn a_subscription_login_never_rides_a_value_request() {
    use super::super::value::NO_KEY;
    use super::super::value::tests::{KEY, store};

    let walk = |with_key: bool| walk_a_field(store(with_key), "doc-1");

    let (questions, heard, verbs, walked) = walk(false);
    assert!(
        questions[0].get("type_target").is_none(),
        "an entry was offered"
    );
    assert!(
        questions[0]["action"]["criteria"]
            .get("type_text")
            .is_none(),
        "Type candidates 0"
    );
    assert!(heard.is_empty(), "the value seat asked: {heard:?}");
    assert!(!verbs.iter().any(|verb| verb == "type"));
    assert_eq!(walked.typed, 0);
    assert_eq!(walked.rows[0]["reason"], json!(NO_KEY));

    let (questions, heard, verbs, walked) = walk(true);
    assert!(
        questions[0].get("type_target").is_some(),
        "the entry is offered"
    );
    // One value, one request; the road's warm-up before it carries no key;
    // no request carries the subscription login.
    let (posts, warm_ups): (Vec<&String>, Vec<&String>) = heard
        .iter()
        .partition(|request| request.starts_with("POST"));
    assert_eq!(posts.len(), 1, "one value, one request: {heard:?}");
    assert!(
        posts[0]
            .to_ascii_lowercase()
            .contains(&format!("x-api-key: {KEY}")),
        "{}",
        posts[0]
    );
    for request in &warm_ups {
        assert!(
            !request.to_ascii_lowercase().contains("x-api-key"),
            "{request}"
        );
    }
    for request in &heard {
        assert!(!request.contains("a-subscription-login"), "{request}");
    }
    assert!(verbs.iter().any(|verb| verb == "type"));
    assert_eq!(walked.typed, 1);
}

// ---- A page's press that leaves its settle for the next look (t-9712) ----

use zerocode_core::agent_browser::{
    BROWSER_SETTLE_KEY, BROWSER_SETTLE_LATER_FLAG, Settle, SettleWhy, settle_said, settle_unheard,
};

/// A road that answers a page's calls in the order the test wrote them down,
/// and keeps every argv it was handed.
struct Script {
    said: RefCell<Vec<Vec<String>>>,
    answers: RefCell<std::collections::VecDeque<TeamAnswer>>,
}

impl Script {
    fn of(answers: Vec<TeamAnswer>) -> Self {
        Self {
            said: RefCell::new(Vec::new()),
            answers: RefCell::new(answers.into()),
        }
    }
    fn road(&self) -> impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer + '_ {
        move |_, argv, _| {
            self.said.borrow_mut().push(argv.to_vec());
            self.answers
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(refused)
        }
    }
    fn said(&self) -> Vec<Vec<String>> {
        self.said.borrow().clone()
    }
}

/// A page whose one button says `next`, as `marks --json` answers it — with
/// how the pane's last press settled, when this look finished one.
fn a_step_page(next: &str, settle: Option<Value>) -> String {
    let mut page = json!({
        "items": [
            { "mark": 1, "role": "button", "tag": "button", "label": next,
              "selector": "#next", "centerX": 120.0, "centerY": 40.0 },
        ],
        "count": 1,
        (zerocode_core::screen_action::snapshot::EPOCH_KEY): "doc-1",
    });
    if let Some(settle) = settle {
        page[BROWSER_SETTLE_KEY] = settle;
    }
    page.to_string()
}

/// A settle-later press's answer: the page as the press left it, with the
/// press's own sentence beside it.
fn a_press_that_left(next: &str) -> String {
    let mut answer: Value = serde_json::from_str(&a_step_page(next, None)).expect("json");
    answer[crate::cmd::browser::CLICK_SAID_KEY] = json!(format!(
        "{} (method=dom-activation)",
        crate::cmd::browser::CLICK_SAID
    ));
    answer.to_string()
}

/// A settle-later press's answer when the press left the legend it was made
/// on (t-9876): the page read after its settle, how it settled under the
/// look's own key, and the press's sentence beside it.
fn a_press_that_settled(next: &str, settle: &Value) -> String {
    let mut answer: Value = serde_json::from_str(&a_press_that_left(next)).expect("json");
    answer[BROWSER_SETTLE_KEY] = settle.clone();
    answer.to_string()
}

fn a_pane_walk<'a, R>(road: &'a mut R, previewing: bool) -> GoalWorld<'a, R>
where
    R: FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
{
    GoalWorld::new(
        road,
        Aim::Pane {
            label: "browser-9".into(),
        },
        Seen::Page {
            host: "app.local".into(),
            path: "/steps".into(),
        },
        Some("Step 4 of 4".into()),
        60_000,
        0,
    )
    .previewing(previewing)
}

fn words(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_string()).collect()
}

/// A page walk that asks ahead presses with `--settle-later` (t-9712): the
/// press hands back the page it changed — at the walk's own address — once;
/// the pane's next look finishes the settle and says how, before the reach
/// check reads the page; and a page that settled is not read a third time:
/// its look is the next step's.
#[test]
fn a_page_press_that_settles_later_hands_back_the_page_it_left_and_its_next_look_settles_it() {
    let ready = settle_said(Settle::Ready, SettleWhy::Quiet, 52);
    let script = Script::of(vec![
        ok(&a_step_page("Next: 2", None)),
        ok(&a_press_that_left("Next: 3")),
        ok(&a_step_page("Next: 3", Some(ready.clone()))),
        ok(r#"{"count":0}"#),
    ]);
    let mut road = script.road();
    let mut world = a_pane_walk(&mut road, true);
    assert!(
        !world.asks_ahead_of_the_press(),
        "a page that settles later asks on the page its press changed"
    );
    world.look().expect("the first look");
    assert!(world.press(1));
    let left = world.unsettled().expect("the page the press changed");
    assert_eq!(
        left.at,
        Seen::Page {
            host: "app.local".into(),
            path: "/steps".into()
        }
    );
    assert_eq!(left.items[0]["label"], "Next: 3");
    assert!(world.unsettled().is_none(), "handed back once");
    let settled = world.settle().expect("the settle the press left");
    assert_eq!(settled.note, ready);
    assert!(world.settle().is_none(), "one settle a press");
    assert_eq!(world.reached(), Some(false));
    let next = world.look().expect("the settled page");
    assert_eq!(Some(next), settled.screen);
    drop(world);
    assert_eq!(
        script.said(),
        [
            words(&["marks", "browser-9", "--json"]),
            words(&[
                "click",
                "browser-9",
                "--mark",
                "1",
                BROWSER_SETTLE_LATER_FLAG
            ]),
            words(&["marks", "browser-9", "--json"]),
            words(&["find", "browser-9", "Step 4 of 4"]),
        ],
        "the settled page's look is the next step's: no third read of it"
    );
}

/// A settle that did not end `ready` keeps nothing: the next look reads the
/// page again (t-9712) — and a look that failed before it could say how the
/// settle ended says `unknown`, never a settle that went unheard in silence.
#[test]
fn a_page_that_did_not_settle_is_looked_at_again_and_an_unheard_settle_says_unknown() {
    let not_ready = settle_said(Settle::NotReady, SettleWhy::Moving, 250);
    for (settling, said) in [
        (
            ok(&a_step_page("Next: 2", Some(not_ready.clone()))),
            not_ready.clone(),
        ),
        (refused(), settle_unheard()),
    ] {
        let script = Script::of(vec![
            ok(&a_step_page("Next: 2", None)),
            ok(&a_press_that_left("Next: 2")),
            settling,
            ok(&a_step_page("Next: 3", None)),
        ]);
        let mut road = script.road();
        let mut world = a_pane_walk(&mut road, true);
        world.look().expect("the first look");
        assert!(world.press(1));
        let settled = world.settle().expect("a settle was left");
        assert_eq!(settled.note, said);
        let again = world.look().expect("the page read again");
        assert_eq!(again.items[0]["label"], "Next: 3", "{said}");
        drop(world);
        assert_eq!(script.said().len(), 4, "{said}");
        assert_eq!(script.said()[3], words(&["marks", "browser-9", "--json"]));
    }
}

/// A walk that does not ask ahead presses exactly as v1.1.27 did — `click
/// <pane> --mark <n>`, a press that settles before it answers — and has no
/// page to hand back and no settle to wait for; a press the door refused
/// leaves none either.
#[test]
fn a_page_walk_that_does_not_ask_ahead_presses_as_it_always_did() {
    let script = Script::of(vec![
        ok(&a_step_page("Next: 2", None)),
        ok("클릭 이벤트를 보냈습니다 (method=dom-activation, settle=ready)"),
    ]);
    let mut road = script.road();
    let mut world = a_pane_walk(&mut road, false);
    assert!(world.asks_ahead_of_the_press());
    world.look().expect("the first look");
    assert!(world.press(1));
    assert!(world.unsettled().is_none());
    assert!(world.settle().is_none());
    drop(world);
    assert_eq!(
        script.said()[1],
        words(&["click", "browser-9", "--mark", "1"])
    );

    let script = Script::of(vec![ok(&a_step_page("Next: 2", None)), refused()]);
    let mut road = script.road();
    let mut world = a_pane_walk(&mut road, true);
    world.look().expect("the first look");
    assert!(!world.press(1));
    assert!(world.unsettled().is_none());
    assert!(world.settle().is_none(), "a refused press left no settle");
}

/// A page press that left the legend it was made on settled before it
/// answered (t-9876), and its answer says how under the look's own key: the
/// world holds nothing — no page handed back to begin on, no settle left for
/// a later look — its settle is the press's own, the reach check asks `find`
/// as a page's always does, and the settled page the press answered is the
/// next step's look, read no second time. A press that changed the legend
/// still answers at once and holds its settle, as before.
#[test]
fn a_page_press_that_left_its_legend_settled_before_it_answered_and_holds_nothing() {
    let ready = settle_said(Settle::Ready, SettleWhy::Quiet, 52);
    let script = Script::of(vec![
        ok(&a_step_page("Next: 2", None)),
        ok(&a_press_that_settled("Next: 3", &ready)),
        ok(r#"{"count":0}"#),
        ok(&a_press_that_left("Next: 4")),
        ok(&a_step_page("Next: 4", Some(ready.clone()))),
    ]);
    let mut road = script.road();
    let mut world = a_pane_walk(&mut road, true);
    world.look().expect("the first look");
    assert!(world.press(1));
    let settled = world
        .settled()
        .expect("the press said how its page settled");
    assert_eq!(settled.note, ready);
    assert_eq!(
        settled.screen, None,
        "a page hands back no screen to begin on"
    );
    assert!(
        world.unsettled().is_none(),
        "the legend stood: nothing to begin on"
    );
    assert!(world.settle().is_none(), "nothing held for a later look");
    assert_eq!(world.reached(), Some(false));
    let next = world.look().expect("the page the press answered");
    assert_eq!(next.items[0]["label"], "Next: 3");
    assert_eq!(
        next.at,
        Seen::Page {
            host: "app.local".into(),
            path: "/steps".into()
        }
    );

    assert!(world.press(1));
    assert!(world.settled().is_none(), "its settle is still to come");
    let left = world.unsettled().expect("the page the press changed");
    assert_eq!(left.items[0]["label"], "Next: 4");
    assert_eq!(world.settle().expect("the settle it held").note, ready);
    drop(world);
    assert_eq!(
        script.said(),
        [
            words(&["marks", "browser-9", "--json"]),
            words(&[
                "click",
                "browser-9",
                "--mark",
                "1",
                BROWSER_SETTLE_LATER_FLAG
            ]),
            words(&["find", "browser-9", "Step 4 of 4"]),
            words(&[
                "click",
                "browser-9",
                "--mark",
                "1",
                BROWSER_SETTLE_LATER_FLAG
            ]),
            words(&["marks", "browser-9", "--json"]),
        ],
        "the page the first press answered is the next step's look: one read a step"
    );
}

/// An entry's own press settles before it answers even in a walk that asks
/// ahead (t-9712): the typing follows it at once, and no settle may be left
/// for a look that comes after the typing.
#[test]
fn an_entrys_own_press_settles_before_the_typing_even_when_the_walk_asks_ahead() {
    let road = Kept::on(a_page_with_a_field("doc-1", "", "#destination"));
    let mut send = road.road();
    let (pen, _, _) = Pen::writing("London");
    let mut world = GoalWorld::new(
        &mut send,
        Aim::Pane {
            label: "browser-9".into(),
        },
        Seen::default(),
        None,
        60_000,
        0,
    )
    .previewing(true)
    .writing(Box::new(pen))
    .remembering(memory());
    world.look().expect("a look with a field");
    assert!(matches!(
        world.type_into(1, "Search for London"),
        Typed::Typed { .. }
    ));
    assert!(world.settle().is_none(), "no settle was left for later");
    drop(world);
    let calls = road.calls.borrow();
    let pressed: Vec<&Vec<String>> = calls
        .iter()
        .map(|(_, argv, _)| argv)
        .filter(|argv| argv[0] == "click")
        .collect();
    assert_eq!(
        pressed,
        [&words(&["click", "browser-9", "--mark", "1"])],
        "the field is pressed as v1.1.27 pressed it"
    );
}

/// The pane's key turns typing on for the walks after it (t-9537), counted:
/// walks over pages with a field — each with a writer of its own over the one
/// key store, as the window makes one per walk — type into none of their
/// fields and say `value_no_key` before a person saves the key in the
/// Computer Use pane, and into every one after it, with no restart between.
/// `--nocapture` prints the count.
#[test]
fn the_panes_key_turns_typing_on_for_the_walks_after_it() {
    use super::super::value::NO_KEY;
    use super::super::value::tests::{KEY, OneStore, chosen_key, store};

    const WALKS: usize = 6;
    let keys = OneStore::over(store(false));
    let walks = |first: usize| -> Vec<super::super::Walked> {
        (first..first + WALKS)
            .map(|page| walk_a_field(Box::new(keys.clone()), &format!("doc-{page}")).3)
            .collect()
    };

    let before = walks(0);
    for walked in &before {
        assert_eq!(walked.rows[0]["reason"], json!(NO_KEY), "{walked:?}");
    }
    crate::type_value_keys::save(chosen_key(), KEY, &keys).expect("the pane keeps the key");
    let after = walks(WALKS);

    let typed = |walks: &[super::super::Walked]| walks.iter().map(|walked| walked.typed).sum();
    let (typed_before, typed_after): (usize, usize) = (typed(&before), typed(&after));
    println!(
        "type steps run on pages with a field: {typed_before}/{WALKS} before the pane's key, \
         {typed_after}/{WALKS} after"
    );
    assert_eq!((typed_before, typed_after), (0, WALKS));
}
