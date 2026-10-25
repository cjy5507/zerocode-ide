use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::self_surface::{Facts, OwnApp};

use super::stand_in;
use crate::computer_use::{ComputerUseError, call, test_helper};

fn one_hand() -> std::sync::MutexGuard<'static, ()> {
    crate::tests::computer_desktop_wait::ONE_HAND
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn own() -> OwnApp {
    OwnApp {
        pid: std::process::id(),
        bundle_id: "dev.zerocode.app".into(),
        names: vec!["ZeroCode".into()],
    }
}

/// One control of the stood-in page.
#[derive(Clone)]
struct Control {
    role: &'static str,
    name: &'static str,
    value: Option<&'static str>,
    facts: Facts,
}

fn control(role: &'static str, name: &'static str) -> Control {
    Control {
        role,
        name,
        value: None,
        facts: Facts::default(),
    }
}

/// A page of controls that answers the window's three requests and writes
/// down every press it was asked to make.
#[derive(Default)]
struct Page {
    controls: Vec<Control>,
    pressed: Vec<(usize, String, Value)>,
}

fn page_answer(page: &Mutex<Page>, request: &Value) -> Result<Value, ComputerUseError> {
    let mut page = page.lock().unwrap();
    let id = request
        .get("id")
        .and_then(Value::as_u64)
        .map(|id| id as usize);
    match request["kind"].as_str() {
        Some("look") => Ok(json!({
            "lookId": 7,
            "title": "ZeroCode",
            "controls": page.controls.iter().enumerate().map(|(at, control)| json!({
                "id": at,
                "role": control.role,
                "name": control.name,
                "value": control.value,
                "facts": control.facts,
            })).collect::<Vec<_>>(),
        })),
        Some("facts") => {
            let control = &page.controls[id.unwrap()];
            Ok(json!({ "facts": control.facts, "role": control.role, "name": control.name }))
        }
        Some("do") => {
            let act = request["act"].as_str().unwrap_or_default().to_string();
            page.pressed.push((id.unwrap(), act, request.clone()));
            Ok(json!({ "done": true }))
        }
        other => panic!("the page was asked {other:?}"),
    }
}

/// The page, stood in: what it answers and what it was asked to press.
fn stand_page(controls: Vec<Control>) -> (stand_in::Standing, Arc<Mutex<Page>>) {
    let page = Arc::new(Mutex::new(Page {
        controls,
        pressed: Vec::new(),
    }));
    let answering = Arc::clone(&page);
    let standing = stand_in::stand(
        own(),
        Arc::new(move |request: &Value| page_answer(&answering, request)),
    );
    (standing, page)
}

/// The helper's acting verbs (`Provider.actingMethods` in `main.swift`) that
/// these tests ask.
const HELPER_ACTS: [&str; 7] = [
    "click",
    "setValue",
    "scroll",
    "typeText",
    "pressKey",
    "hotkey",
    "pasteText",
];

/// The helper, stood in as it answers today: a look at ZeroCode answers (an
/// empty one), and every act that names ZeroCode is refused in its words
/// (`refuseOwnTarget`). It counts what it was asked.
fn stand_helper() -> (test_helper::Standing, Arc<Mutex<Vec<String>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let counting = Arc::clone(&asked);
    let standing = test_helper::stand(move |method, params| {
        counting.lock().unwrap().push(method.to_string());
        if HELPER_ACTS.contains(&method)
            && params.get("app").and_then(Value::as_str) == Some("ZeroCode")
        {
            return Err(ComputerUseError::new(
                error_code::APP_BLOCKED,
                "'ZeroCode' is ZeroCode itself; the operator does not drive the app it lives in",
            ));
        }
        Ok(json!({}))
    });
    (standing, asked)
}

fn ordinary_page() -> Vec<Control> {
    vec![
        control("AXTab", "Board"),
        control("AXButton", "Split pane"),
        Control {
            value: Some(""),
            ..control("AXTextField", "Search files")
        },
    ]
}

/// The person's words: an agent works in ZeroCode's own window like dots
/// does — a tab, a pane's button — and the helper is never asked, so no OS
/// event takes the person's pointer, focus or keys.
#[test]
fn zerocode_ordinary_button_is_pressed_by_the_self_road() {
    let _hand = one_hand();
    let (_helper, helper_asked) = stand_helper();
    let (_page, page) = stand_page(ordinary_page());

    let look = call("getAppState", json!({ "app": "ZeroCode" }))
        .unwrap_or_else(|error| panic!("the look was refused: {error:?}"));
    let tree = look["snapshot"]["treeText"].as_str().unwrap_or_default();
    assert!(tree.contains("1 tab Board"), "{tree}");
    assert!(tree.contains("2 button Split pane"), "{tree}");

    let pressed = call("click", json!({ "app": "ZeroCode", "elementIndex": 2 }))
        .unwrap_or_else(|error| panic!("the press was refused: {error:?}"));
    assert_eq!(pressed["action"]["path"], "self", "{pressed}");
    assert_eq!(
        pressed["action"]["background"]["foregroundUnchanged"], true,
        "{pressed}"
    );

    let filled = call(
        "setValue",
        json!({ "app": "ZeroCode", "elementIndex": 3, "value": "main.rs" }),
    )
    .unwrap_or_else(|error| panic!("the field was refused: {error:?}"));
    assert_eq!(filled["action"]["path"], "self", "{filled}");

    let page = page.lock().unwrap();
    let presses: Vec<(usize, &str)> = page
        .pressed
        .iter()
        .map(|(id, act, _)| (*id, act.as_str()))
        .collect();
    assert_eq!(presses, vec![(1, "click"), (2, "setValue")]);
    assert_eq!(page.pressed[1].2["value"], "main.rs");
    assert!(
        helper_asked.lock().unwrap().is_empty(),
        "the helper was asked {:?}",
        helper_asked.lock().unwrap()
    );
}

/// The marks that keep a control the person's still refuse it — and the
/// page is never asked to press it.
#[test]
fn a_marked_control_of_zerocodes_window_is_refused_and_not_pressed() {
    let _hand = one_hand();
    let (_helper, helper_asked) = stand_helper();
    let marked = [
        Facts {
            asking: true,
            ..Facts::default()
        },
        Facts {
            secret: true,
            ..Facts::default()
        },
        Facts {
            beside_secret: true,
            ..Facts::default()
        },
        Facts {
            terminal: true,
            ..Facts::default()
        },
        Facts {
            person_only: true,
            ..Facts::default()
        },
        Facts {
            setting: true,
            ..Facts::default()
        },
    ];
    let controls = marked
        .iter()
        .map(|facts| Control {
            facts: facts.clone(),
            ..control("AXButton", "Allow")
        })
        .collect();
    let (_page, page) = stand_page(controls);
    call("getAppState", json!({ "app": "ZeroCode" })).unwrap();
    for index in 1..=marked.len() {
        let refused = call("click", json!({ "app": "ZeroCode", "elementIndex": index }))
            .expect_err("a marked control was pressed");
        assert_eq!(refused.code, error_code::SELF_SENSITIVE, "{index}");
    }
    assert!(page.lock().unwrap().pressed.is_empty());
    assert!(helper_asked.lock().unwrap().is_empty());
}

/// The field the person is typing in is left to them.
#[test]
fn the_field_the_person_types_in_is_left_to_them() {
    let _hand = one_hand();
    let (_helper, _) = stand_helper();
    let (_page, page) = stand_page(vec![Control {
        facts: Facts {
            person_focus: true,
            ..Facts::default()
        },
        ..control("AXTextArea", "Message")
    }]);
    call("getAppState", json!({ "app": "ZeroCode" })).unwrap();
    let refused = call(
        "setValue",
        json!({ "app": "ZeroCode", "elementIndex": 1, "value": "hi" }),
    )
    .expect_err("the person's field was filled");
    assert_eq!(refused.code, error_code::PERSON_TYPING);
    assert!(page.lock().unwrap().pressed.is_empty());
}

/// Keys, typed text, a paste or a drag would be OS events in the window the
/// person works in: refused, and neither the page nor the helper is asked.
#[test]
fn keys_on_zerocodes_window_are_refused_without_asking_anyone() {
    let _hand = one_hand();
    let (_helper, helper_asked) = stand_helper();
    let (_page, page) = stand_page(ordinary_page());
    for method in ["typeText", "pressKey", "hotkey", "pasteText"] {
        let refused = call(
            method,
            json!({ "app": "ZeroCode", "text": "x", "key": "return" }),
        )
        .expect_err(method);
        assert_eq!(refused.code, error_code::UNSUPPORTED_CAPABILITY, "{method}");
    }
    assert!(page.lock().unwrap().pressed.is_empty());
    assert!(helper_asked.lock().unwrap().is_empty());
}

/// Another app is the helper's, as before.
#[test]
fn another_app_still_goes_to_the_helper() {
    let _hand = one_hand();
    let (_helper, helper_asked) = stand_helper();
    let (_page, page) = stand_page(ordinary_page());
    call("click", json!({ "app": "Calculator", "elementIndex": 2 })).unwrap();
    assert_eq!(*helper_asked.lock().unwrap(), vec!["click".to_string()]);
    assert!(page.lock().unwrap().pressed.is_empty());
}

/// An index from no look of the page, or past its end, presses nothing.
#[test]
fn an_index_the_page_never_showed_presses_nothing() {
    let _hand = one_hand();
    let (_helper, _) = stand_helper();
    let (_page, page) = stand_page(ordinary_page());
    call("getAppState", json!({ "app": "ZeroCode" })).unwrap();
    let refused = call("click", json!({ "app": "ZeroCode", "elementIndex": 9 }))
        .expect_err("an unknown index was pressed");
    assert_eq!(refused.code, error_code::ELEMENT_NOT_FOUND);
    assert!(page.lock().unwrap().pressed.is_empty());
}

/// The window's road to its page: an answer by id comes back, a page's
/// refusal keeps its code, and silence is reported unknown — never pressed.
#[test]
fn the_page_answers_by_id_and_silence_is_unknown() {
    use std::time::Duration;

    use super::{answered, ask_through};

    let answering = |answer: Value| {
        move |id: u64, _: &Value| {
            let answer = answer.clone();
            std::thread::spawn(move || assert!(answered(id, answer)));
        }
    };
    let said = ask_through(
        &answering(json!({ "ok": { "done": true } })),
        &json!({ "kind": "do" }),
        Duration::from_secs(5),
    );
    assert_eq!(said.unwrap(), json!({ "done": true }));

    let refused = ask_through(
        &answering(json!({ "error": { "code": "self_sensitive", "message": "kept" } })),
        &json!({ "kind": "do" }),
        Duration::from_secs(5),
    )
    .expect_err("a refusal answered ok");
    assert_eq!(refused.code, error_code::SELF_SENSITIVE);

    let silent = ask_through(
        &|_, _| {},
        &json!({ "kind": "do" }),
        Duration::from_millis(20),
    )
    .expect_err("silence answered ok");
    assert_eq!(silent.code, error_code::ACTION_TIMEOUT);
    assert!(silent.message.contains("unknown"), "{}", silent.message);
    assert!(
        !answered(u64::MAX, json!({})),
        "nothing waits for an id never asked"
    );
}

/// The second wall: while an agent's press in ZeroCode's own window is in
/// flight — and a moment after, for a handler that awaits first — no
/// person-only command runs, whatever the page's marks missed.
#[test]
fn an_agents_press_holds_every_person_only_command() {
    use std::time::{Duration, Instant};

    use zerocode_core::computer_use_protocol::self_surface::{
        PAGE_ANSWER_MS, PERSON_ONLY_GRACE_MS,
    };

    use super::{PERSON_ONLY_COMMANDS, person_only, person_only_at};

    let _hand = one_hand();
    let (_helper, _) = stand_helper();
    let during = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&during);
    let page = Arc::new(Mutex::new(Page {
        controls: ordinary_page(),
        pressed: Vec::new(),
    }));
    let answering = Arc::clone(&page);
    let _page = stand_in::stand(
        own(),
        Arc::new(move |request: &Value| {
            if request["kind"] == "do" {
                for command in PERSON_ONLY_COMMANDS {
                    seen.lock().unwrap().push((*command, person_only(command)));
                }
            }
            page_answer(&answering, request)
        }),
    );
    call("getAppState", json!({ "app": "ZeroCode" })).unwrap();
    call("click", json!({ "app": "ZeroCode", "elementIndex": 2 })).unwrap();
    let pressed = Instant::now();

    let during = during.lock().unwrap();
    assert_eq!(during.len(), PERSON_ONLY_COMMANDS.len());
    for (command, said) in during.iter() {
        assert!(said.is_err(), "{command} ran during an agent's press");
    }
    for command in PERSON_ONLY_COMMANDS {
        assert!(
            person_only(command).is_err(),
            "{command} ran right after an agent's press"
        );
        let later = pressed + Duration::from_millis(PERSON_ONLY_GRACE_MS + PAGE_ANSWER_MS);
        assert_eq!(person_only_at(command, later), Ok(()), "{command} later");
    }
}

/// The invoke handler's question: a command on the person-only list is
/// refused during an agent's press, any other command never is.
#[test]
fn the_invoke_gate_refuses_only_the_persons_commands_under_a_press() {
    use super::person_only_refusal;

    let _hand = one_hand();
    let (_helper, _) = stand_helper();
    let during = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&during);
    let page = Arc::new(Mutex::new(Page {
        controls: ordinary_page(),
        pressed: Vec::new(),
    }));
    let answering = Arc::clone(&page);
    let _page = stand_in::stand(
        own(),
        Arc::new(move |request: &Value| {
            if request["kind"] == "do" {
                for command in ["set_computer_input_policy", "jira_connect", "board_columns"] {
                    seen.lock()
                        .unwrap()
                        .push((command, person_only_refusal(command).is_some()));
                }
            }
            page_answer(&answering, request)
        }),
    );
    assert_eq!(person_only_refusal("set_computer_input_policy"), None);
    call("getAppState", json!({ "app": "ZeroCode" })).unwrap();
    call("click", json!({ "app": "ZeroCode", "elementIndex": 1 })).unwrap();
    assert_eq!(
        *during.lock().unwrap(),
        vec![
            ("set_computer_input_policy", true),
            ("jira_connect", true),
            ("board_columns", false),
        ]
    );
}
