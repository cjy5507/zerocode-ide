//! What the question promises and what an answer must be to be one.

use super::*;

/// One marks item as the browser door and the desktop look both speak it.
fn item(mark: usize, role: &str, label: &str) -> Value {
    json!({
        "mark": mark,
        "role": role,
        "tag": "button",
        "label": label,
        "selector": "#save > button.primary",
        "x": 400.0,
        "y": 80.0,
        "width": 24.0,
        "height": 16.0,
        "centerX": 412.0,
        "centerY": 88.0,
    })
}

fn a_look<'a>(items: &'a [Value], tried: &'a [usize]) -> ActionLook<'a> {
    ActionLook {
        goal: "checkout smoke",
        errand: Errand::Clear {
            stopped: "step_failed",
            step: "click",
            refusal: "the selector matched nothing",
        },
        at: Where::Page {
            host: "shop.example",
            path: "/cart",
        },
        tried,
        items,
        pressed: &[],
        shows: &[],
    }
}

/// The same screen, asked about by a goal walk on the desktop.
fn a_goal<'a>(items: &'a [Value], tried: &'a [usize]) -> ActionLook<'a> {
    ActionLook {
        goal: "홍길동에게 메시지 보내기",
        errand: Errand::Goal,
        at: Where::Desk {
            app: "카카오톡",
            window: "채팅",
        },
        tried,
        items,
        pressed: &[],
        shows: &[],
    }
}

/// An answer in the contract's shape, spreading the rest evenly.
fn answered(asked: &ActionAsk, choice: &str, confidence: f64) -> Value {
    let options = asked.options();
    let share = 1.0 / options.len() as f64;
    let mut probabilities = Map::new();
    for name in &options {
        probabilities.insert(name.clone(), json!(share));
    }
    // Make it sum to exactly one despite the division.
    let drift = 1.0 - share * options.len() as f64;
    probabilities.insert(options[0].clone(), json!(share + drift));
    json!({
        "action": {
            "type": "choice",
            "choice": choice,
            "probabilities": Value::Object(probabilities),
            "confidence": confidence,
        }
    })
}

#[test]
fn the_question_offers_the_numbers_the_look_saw_and_nothing_else() {
    let items = [item(1, "button", "저장"), item(2, "link", "취소")];
    let asked = ask(&a_look(&items, &[])).expect("a look with controls asks");

    assert_eq!(asked.marks(), [1, 2]);
    assert_eq!(asked.options(), ["mark:1", "mark:2", GIVE_UP]);

    let criteria = asked.questions["action"]["criteria"]
        .as_object()
        .expect("criteria is an object");
    let mut named: Vec<&String> = criteria.keys().collect();
    named.sort();
    assert_eq!(named, [GIVE_UP, "mark:1", "mark:2"]);
    assert_eq!(asked.questions["action"]["type"], json!("choice"));
}

#[test]
fn a_look_with_no_control_asks_nothing() {
    assert!(ask(&a_look(&[], &[])).is_none());
    // Items the legend cannot render are not options either.
    let unreadable = [json!({ "role": "button" })];
    assert!(ask(&a_look(&unreadable, &[])).is_none());
}

#[test]
fn a_number_this_recovery_already_spent_is_not_offered_again() {
    let items = [item(1, "button", "저장"), item(2, "link", "취소")];
    let asked = ask(&a_look(&items, &[1])).expect("one control is left");

    assert_eq!(asked.marks(), [2]);
    assert_eq!(asked.options(), ["mark:2", GIVE_UP]);
    assert_eq!(asked.state["alreadyTried"], json!([1]));

    // Every number spent: there is nothing left to ask about.
    assert!(ask(&a_look(&items, &[1, 2])).is_none());
}

#[test]
fn the_slice_that_is_cut_is_the_slice_that_may_be_chosen() {
    let items: Vec<Value> = (1..=MAX_ACTION_CANDIDATES + 5)
        .map(|mark| item(mark, "button", "행"))
        .collect();
    let asked = ask(&a_look(&items, &[])).expect("a long look still asks");

    assert_eq!(asked.marks().len(), MAX_ACTION_CANDIDATES);
    assert_eq!(asked.marks().last(), Some(&MAX_ACTION_CANDIDATES));
    let criteria = asked.questions["action"]["criteria"].as_object().unwrap();
    assert_eq!(criteria.len(), MAX_ACTION_CANDIDATES + 1);
    assert_eq!(
        asked.state["candidates"].as_array().unwrap().len(),
        MAX_ACTION_CANDIDATES
    );
    // A number past the cut is refused, not quietly honoured.
    let past = answered(&asked, GIVE_UP, 0.5);
    let mut wrong = past.clone();
    wrong["action"]["choice"] = json!(format!("mark:{}", MAX_ACTION_CANDIDATES + 1));
    assert_eq!(asked.read(&wrong), Err(ActionRefusal::UnknownOption));
}

#[test]
fn the_state_carries_the_page_but_never_a_selector_or_a_typed_value() {
    let items = [item(1, "button", "저장")];
    let asked = ask(&a_look(&items, &[])).expect("asks");
    let sent = format!("{}{}", asked.state, asked.questions);

    assert!(sent.contains("shop.example") && sent.contains("/cart"));
    assert!(sent.contains("저장"), "the label a person reads is state");
    for tool_only in [
        "#save",
        "button.primary",
        "elementIndex",
        "windowId",
        "\"tag\"",
    ] {
        assert!(
            !sent.contains(tool_only),
            "`{tool_only}` is the tool's, not the model's: {sent}"
        );
    }
    // The step reaches the model as its verb alone.
    assert_eq!(asked.state["step"], json!("click"));
}

#[test]
fn a_well_formed_answer_reads_as_the_number_it_chose() {
    let items = [item(1, "button", "저장"), item(7, "button", "다시 시도")];
    let asked = ask(&a_look(&items, &[])).expect("asks");

    let read = asked
        .read(&answered(&asked, "mark:7", 0.62))
        .expect("valid");
    assert_eq!(read.chosen, Chosen::Mark(7));
    assert!((read.confidence - 0.62).abs() < f64::EPSILON);
    assert_eq!(read.probabilities.len(), 3);

    let gave_up = asked.read(&answered(&asked, GIVE_UP, 0.4)).expect("valid");
    assert_eq!(gave_up.chosen, Chosen::GiveUp);
}

#[test]
fn every_broken_rule_discards_the_answer_whole() {
    let items = [item(1, "button", "저장")];
    let asked = ask(&a_look(&items, &[])).expect("asks");
    let good = answered(&asked, "mark:1", 0.5);

    let mut missing = good.clone();
    missing.as_object_mut().unwrap().remove("action");
    assert_eq!(asked.read(&missing), Err(ActionRefusal::NoAnswer));

    let mut wrong_kind = good.clone();
    wrong_kind["action"]["type"] = json!("score");
    assert_eq!(asked.read(&wrong_kind), Err(ActionRefusal::NotAChoice));

    let mut invented = good.clone();
    invented["action"]["choice"] = json!("mark:99");
    assert_eq!(asked.read(&invented), Err(ActionRefusal::UnknownOption));

    let mut not_a_mark = good.clone();
    not_a_mark["action"]["choice"] = json!("click the blue one");
    assert_eq!(asked.read(&not_a_mark), Err(ActionRefusal::UnknownOption));

    let mut short_keys = good.clone();
    short_keys["action"]["probabilities"] = json!({ "mark:1": 1.0 });
    assert_eq!(asked.read(&short_keys), Err(ActionRefusal::Keys));

    let mut stranger_key = good.clone();
    stranger_key["action"]["probabilities"] = json!({ "mark:1": 0.5, "mark:4": 0.5 });
    assert_eq!(asked.read(&stranger_key), Err(ActionRefusal::Keys));

    let mut unbalanced = good.clone();
    unbalanced["action"]["probabilities"] = json!({ "mark:1": 0.2, GIVE_UP: 0.2 });
    assert_eq!(asked.read(&unbalanced), Err(ActionRefusal::NotOne));

    let mut out_of_range = good.clone();
    out_of_range["action"]["probabilities"] = json!({ "mark:1": 1.4, GIVE_UP: -0.4 });
    assert_eq!(asked.read(&out_of_range), Err(ActionRefusal::OutOfRange));

    let mut not_a_number = good.clone();
    not_a_number["action"]["probabilities"] = json!({ "mark:1": "많이", GIVE_UP: 0.0 });
    assert_eq!(asked.read(&not_a_number), Err(ActionRefusal::OutOfRange));

    let mut no_confidence = good.clone();
    no_confidence["action"]
        .as_object_mut()
        .unwrap()
        .remove("confidence");
    assert_eq!(asked.read(&no_confidence), Err(ActionRefusal::OutOfRange));

    let mut wild_confidence = good;
    wild_confidence["action"]["confidence"] = json!(1.5);
    assert_eq!(asked.read(&wild_confidence), Err(ActionRefusal::OutOfRange));
}

#[test]
fn the_version_is_pinned_to_the_words() {
    // Changing a word of the question without bumping the version turns this
    // red: a judgment read under one wording is not evidence about another.
    assert_eq!(SCREEN_ACTION_RUBRIC_VERSION, 4);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "24ffa8989582fd7e"
    );
}

#[test]
fn a_goal_walk_may_say_it_is_there_and_a_stopped_walk_may_not() {
    let items = [item(1, "button", "보내기"), item(2, "link", "취소")];

    let goal = ask(&a_goal(&items, &[])).expect("a screen with controls asks");
    assert_eq!(goal.options(), ["mark:1", "mark:2", GIVE_UP, DONE]);
    let criteria = goal.questions["action"]["criteria"]
        .as_object()
        .expect("criteria is an object");
    assert!(criteria.contains_key(DONE), "a goal may answer `done`");
    assert_eq!(
        goal.read(&answered(&goal, DONE, 0.8))
            .expect("a read")
            .chosen,
        Chosen::Done
    );

    // The errand that did not offer it cannot be answered with it: the
    // document's own re-walk, not a judgment, says whether a stopped flow is
    // finished.
    let clear = ask(&a_look(&items, &[])).expect("a screen with controls asks");
    assert_eq!(clear.options(), ["mark:1", "mark:2", GIVE_UP]);
    assert!(
        !clear.questions["action"]["criteria"]
            .as_object()
            .expect("criteria is an object")
            .contains_key(DONE)
    );
    let mut said = answered(&clear, GIVE_UP, 0.8);
    said["action"]["choice"] = json!(DONE);
    assert_eq!(clear.read(&said), Err(ActionRefusal::UnknownOption));
}

#[test]
fn a_goal_walk_says_where_it_is_by_the_surfaces_own_two_words() {
    let items = [item(1, "button", "보내기")];

    let desk = ask(&a_goal(&items, &[])).expect("a screen with controls asks");
    assert_eq!(
        desk.state["where"],
        json!({ "app": "카카오톡", "window": "채팅" })
    );
    assert_eq!(desk.state["goal"], json!("홍길동에게 메시지 보내기"));
    // Nothing failed, so the stopped walk's three keys are absent — a key a
    // request does not carry sends nothing.
    for key in ["stopped", "step", "refusal"] {
        assert!(desk.state.get(key).is_none(), "a goal walk sends no {key}");
    }

    let page = ask(&a_look(&items, &[])).expect("a screen with controls asks");
    assert_eq!(
        page.state["where"],
        json!({ "host": "shop.example", "path": "/cart" })
    );
    assert_eq!(page.state["stopped"], json!("step_failed"));
}

#[test]
fn a_number_already_spent_on_this_screen_is_not_offered_again() {
    let items = [
        item(1, "button", "보내기"),
        item(2, "link", "취소"),
        item(3, "button", "닫기"),
    ];
    let asked = ask(&a_goal(&items, &[2])).expect("a screen with controls asks");
    assert_eq!(asked.marks(), [1, 3]);
    assert_eq!(asked.state["alreadyTried"], json!([2]));

    // Every number spent and the question is not worth asking at all.
    assert!(ask(&a_goal(&items, &[1, 2, 3])).is_none());
}

/// The question carries what the walk already pressed and what the screen
/// shows, and cuts the screen's words by whole lines to the caps — so a
/// calculator's display reaches the judgment (t-5497) and a long page
/// cannot swell the request past what the deadline bounds.
#[test]
fn the_state_carries_what_was_pressed_and_what_the_screen_shows_cut_to_the_caps() {
    let items = vec![item(9, "button", "7"), item(12, "button", "×")];
    let pressed = vec!["button 7 @ 40,300".to_string()];
    let shows: Vec<String> = (0..SHOWS_LINE_CAP + 5)
        .map(|n| format!("line {n}"))
        .collect();
    let look = ActionLook {
        pressed: &pressed,
        shows: &shows,
        ..a_goal(&items, &[])
    };
    let asked = ask(&look).expect("a question");
    assert_eq!(asked.state["pressed"], json!(pressed));
    let carried = asked.state["shows"].as_array().expect("lines");
    assert_eq!(carried.len(), SHOWS_LINE_CAP, "cut by lines");
    assert_eq!(carried[0], json!("line 0"), "top of the screen first");

    let wide = vec![
        "x".repeat(SHOWS_CHAR_CAP - 10),
        "y".repeat(20),
        "z".to_string(),
    ];
    let cut = shows_cut(&wide);
    assert_eq!(
        cut.len(),
        1,
        "a line that would cross the character cap ends the cut"
    );
    let blank = vec!["  ".to_string(), "42".to_string()];
    assert_eq!(shows_cut(&blank), vec!["42"], "blank lines are not lines");
}

/// A second reader's answer — one option, one confidence — is judged by the
/// closed choice's own rules: only an offered option, only a share.
#[test]
fn a_second_readers_answer_is_judged_against_the_offered_set() {
    let items = vec![
        json!({ "mark": 3, "role": "button", "label": "저장", "centerX": 10.0, "centerY": 20.0 }),
        json!({ "mark": 5, "role": "link", "label": "닫기", "centerX": 30.0, "centerY": 20.0 }),
    ];
    let asked = ask(&ActionLook {
        goal: "채팅방 열기",
        errand: Errand::Goal,
        at: Where::Page {
            host: "app.local",
            path: "/",
        },
        tried: &[],
        items: &items,
        pressed: &[],
        shows: &[],
    })
    .expect("asks");
    let chosen = asked.choice_of(" mark:5 ", 0.8).expect("an offered number");
    assert_eq!(chosen.chosen, Chosen::Mark(5));
    assert_eq!(chosen.confidence, 0.8);
    assert_eq!(chosen.probabilities, [("mark:5".to_string(), 0.8)].into());
    assert_eq!(
        asked.choice_of(GIVE_UP, 0.5).expect("offered").chosen,
        Chosen::GiveUp
    );
    assert_eq!(
        asked.choice_of(DONE, 1.0).expect("offered").chosen,
        Chosen::Done
    );
    assert_eq!(
        asked.choice_of("mark:4", 0.9).unwrap_err(),
        ActionRefusal::UnknownOption
    );
    assert_eq!(
        asked.choice_of("", 0.9).unwrap_err(),
        ActionRefusal::UnknownOption
    );
    for not_a_share in [-0.1, 1.01, f64::NAN] {
        assert_eq!(
            asked.choice_of("mark:3", not_a_share).unwrap_err(),
            ActionRefusal::NotOne,
            "{not_a_share}"
        );
    }
}
