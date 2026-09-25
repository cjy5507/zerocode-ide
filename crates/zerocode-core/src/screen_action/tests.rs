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
        },
        // The two guards, answered as a clean screen's would be.
        "instructed": { "type": "noul", "noul": 0.02 },
        "walled": { "type": "noul", "noul": 0.03 },
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
    assert_eq!(
        asked.read(&wrong),
        Err(ActionRefusal::Choice(ChoiceRefusal::UnknownOption))
    );
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
    assert_eq!(
        asked.read(&missing),
        Err(ActionRefusal::Choice(ChoiceRefusal::NoAnswer))
    );

    let mut wrong_kind = good.clone();
    wrong_kind["action"]["type"] = json!("score");
    assert_eq!(
        asked.read(&wrong_kind),
        Err(ActionRefusal::Choice(ChoiceRefusal::NotAChoice))
    );

    let mut invented = good.clone();
    invented["action"]["choice"] = json!("mark:99");
    assert_eq!(
        asked.read(&invented),
        Err(ActionRefusal::Choice(ChoiceRefusal::UnknownOption))
    );

    let mut not_a_mark = good.clone();
    not_a_mark["action"]["choice"] = json!("click the blue one");
    assert_eq!(
        asked.read(&not_a_mark),
        Err(ActionRefusal::Choice(ChoiceRefusal::UnknownOption))
    );

    let mut short_keys = good.clone();
    short_keys["action"]["probabilities"] = json!({ "mark:1": 1.0 });
    assert_eq!(
        asked.read(&short_keys),
        Err(ActionRefusal::Choice(ChoiceRefusal::Keys))
    );

    let mut stranger_key = good.clone();
    stranger_key["action"]["probabilities"] = json!({ "mark:1": 0.5, "mark:4": 0.5 });
    assert_eq!(
        asked.read(&stranger_key),
        Err(ActionRefusal::Choice(ChoiceRefusal::Keys))
    );

    let mut unbalanced = good.clone();
    unbalanced["action"]["probabilities"] = json!({ "mark:1": 0.2, GIVE_UP: 0.2 });
    assert_eq!(
        asked.read(&unbalanced),
        Err(ActionRefusal::Choice(ChoiceRefusal::NotOne))
    );

    let mut out_of_range = good.clone();
    out_of_range["action"]["probabilities"] = json!({ "mark:1": 1.4, GIVE_UP: -0.4 });
    assert_eq!(
        asked.read(&out_of_range),
        Err(ActionRefusal::Choice(ChoiceRefusal::OutOfRange))
    );

    let mut not_a_number = good.clone();
    not_a_number["action"]["probabilities"] = json!({ "mark:1": "많이", GIVE_UP: 0.0 });
    assert_eq!(
        asked.read(&not_a_number),
        Err(ActionRefusal::Choice(ChoiceRefusal::OutOfRange))
    );

    let mut no_confidence = good.clone();
    no_confidence["action"]
        .as_object_mut()
        .unwrap()
        .remove("confidence");
    assert_eq!(
        asked.read(&no_confidence),
        Err(ActionRefusal::Choice(ChoiceRefusal::OutOfRange))
    );

    let mut wild_confidence = good;
    wild_confidence["action"]["confidence"] = json!(1.5);
    assert_eq!(
        asked.read(&wild_confidence),
        Err(ActionRefusal::Choice(ChoiceRefusal::OutOfRange))
    );
}

#[test]
fn the_version_is_pinned_to_the_words() {
    // Changing a word of the question without bumping the version turns this
    // red: a judgment read under one wording is not evidence about another.
    assert_eq!(SCREEN_ACTION_RUBRIC_VERSION, 6);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "31af4b5fa6fb9b34"
    );
}

/// A text field as a page's look numbers it.
fn textbox(mark: usize, label: &str) -> Value {
    json!({
        "mark": mark,
        "role": "textbox",
        "tag": "input",
        "label": label,
        "selector": format!("#field-{mark}"),
        "centerX": 200.0,
        "centerY": 40.0 * mark as f64,
    })
}

/// A field the same look read beside its controls ([`snapshot::FIELDS_KEY`]).
fn field(mark: usize, kind: &str, secret: bool) -> Value {
    json!({
        (snapshot::FIELD_MARK_KEY): mark,
        (snapshot::FIELD_KIND_KEY): kind,
        (snapshot::FIELD_SECRET_KEY): secret,
        (snapshot::LABEL_KEY): "",
        (snapshot::FIELD_VALUE_KEY): "",
    })
}

/// Every head of an answer answered — the action as `action`, the field as
/// `type_target`, each observation head as named — with the rest of each
/// head's options sharing what is left, and the guards clean.
fn answered_heads(asked: &ActionAsk, chosen: &[(&str, &str)]) -> Value {
    let mut answers = answered(asked, GIVE_UP, 0.9);
    for (head, choice) in chosen {
        let options: Vec<String> = asked.questions[*head]["criteria"]
            .as_object()
            .expect("the head was asked")
            .keys()
            .cloned()
            .collect();
        let mut probabilities = Map::new();
        let share = 0.2 / (options.len().max(2) - 1) as f64;
        for option in &options {
            probabilities.insert(option.clone(), json!(share));
        }
        probabilities.insert((*choice).to_string(), json!(0.8));
        if !options.iter().any(|option| option == choice) {
            // An answer naming something the head never offered: keep the
            // keys the head offered so only the choice itself is wrong.
            probabilities.remove(*choice);
            probabilities.insert(options[0].clone(), json!(0.8));
        }
        let total: f64 = probabilities.values().filter_map(Value::as_f64).sum();
        let first = options[0].clone();
        let lead = probabilities[&first].as_f64().unwrap_or(0.0) + (1.0 - total);
        probabilities.insert(first, json!(lead));
        answers[*head] = json!({
            "type": "choice",
            "choice": choice,
            "probabilities": Value::Object(probabilities),
            "confidence": if *head == "action" { 0.9 } else { 0.6 },
        });
    }
    answers
}

/// Only a field the look read, of a kind a keyboard types and not a secret,
/// is somewhere a walk may type (t-6720): the operation is offered beside
/// the presses, the field is asked in its own head, and an answer naming a
/// button, a secret, a field the look never read, a selector or a command is
/// refused whole. A walk that cannot type, and a stopped walk, ask exactly
/// what they asked before.
#[test]
fn type_text_is_a_closed_observed_operation_with_a_compatible_target() {
    let items = [
        item(1, "button", "Search"),
        textbox(2, "Destination"),
        textbox(3, "Password"),
        textbox(4, "Notes"),
        json!({ "mark": 5, "role": "combobox", "tag": "select", "label": "Cabin",
                "selector": "#cabin", "centerX": 10.0, "centerY": 10.0 }),
        textbox(6, "PIN"),
        textbox(7, "Code"),
    ];
    // Mark 4 is a textbox the look never read as a field; 6 is a text field
    // the page calls secret (a current password in a text box); 7 is one
    // whose entry says nothing of secrecy at all.
    let mut unsaid = field(7, "text", false);
    unsaid
        .as_object_mut()
        .expect("a field")
        .remove(snapshot::FIELD_SECRET_KEY);
    let fields = [
        field(2, "text", false),
        field(3, "password", true),
        field(5, "select", false),
        field(6, "text", true),
        unsaid,
    ];
    let typing = Beside {
        types: true,
        fields: &fields,
        ..Beside::default()
    };
    let asked = ask_with(&a_goal(&items, &[]), &typing).expect("a screen with controls asks");

    assert_eq!(
        asked.options(),
        [
            "mark:1", "mark:2", "mark:3", "mark:4", "mark:5", "mark:6", "mark:7", TYPE_TEXT,
            GIVE_UP, DONE
        ]
    );
    assert_eq!(asked.typing(), [2], "only the read, plain text field");
    let targets: Vec<&String> = asked.questions["type_target"]["criteria"]
        .as_object()
        .expect("the field is asked in a head of its own")
        .keys()
        .collect();
    assert_eq!(targets, ["mark:2"]);
    assert_eq!(asked.questions["type_target"]["type"], json!("choice"));
    assert!(
        asked.questions["action"]["criteria"][TYPE_TEXT].is_string(),
        "the operation says what it means"
    );

    let typed = asked
        .read_all(&answered_heads(
            &asked,
            &[("action", TYPE_TEXT), ("type_target", "mark:2")],
        ))
        .expect("a whole answer");
    assert_eq!(typed.choice.chosen, Chosen::Type(2));
    assert!(
        (typed.choice.confidence - 0.6).abs() < 1e-9,
        "the lesser of the two heads' confidences"
    );
    assert_eq!(
        typed.typed.as_ref().map(|head| head.chosen.as_str()),
        Some("mark:2")
    );

    for wrong in [
        "mark:1",
        "mark:3",
        "mark:4",
        "mark:5",
        "mark:6",
        "mark:7",
        "#field-2",
        "document.querySelector('#field-2').value = 'x'",
    ] {
        assert_eq!(
            asked.read_all(&answered_heads(
                &asked,
                &[("action", TYPE_TEXT), ("type_target", wrong)]
            )),
            Err(ActionRefusal::Choice(ChoiceRefusal::UnknownOption)),
            "{wrong} is not a field this look read"
        );
    }

    // A second reader answers one option and names no field: it may not type.
    assert_eq!(
        asked.choice_of(TYPE_TEXT, 0.9),
        Err(ActionRefusal::Choice(ChoiceRefusal::UnknownOption))
    );
    assert!(
        !asked
            .press_options()
            .iter()
            .any(|option| option == TYPE_TEXT)
    );

    // A world with no road for a value, a stopped walk, a spent field and a
    // look with no fields all ask exactly what a press-only look asks.
    let plain = ask(&a_goal(&items, &[])).expect("asks");
    assert!(plain.questions.get("type_target").is_none());
    for (look, beside) in [
        (
            a_goal(&items, &[]),
            Beside {
                types: false,
                fields: &fields,
                ..Beside::default()
            },
        ),
        (a_goal(&items, &[]), Beside::default()),
    ] {
        assert_eq!(ask_with(&look, &beside), Some(plain.clone()));
    }
    let stopped = ask_with(&a_look(&items, &[]), &typing).expect("asks");
    assert_eq!(Some(stopped), ask(&a_look(&items, &[])));
    let spent = ask_with(&a_goal(&items, &[2]), &typing).expect("asks");
    assert!(spent.typing().is_empty() && !spent.options().iter().any(|o| o == TYPE_TEXT));
}

/// The observation heads (t-4692) ask which of the containers, images and
/// rows the look read the goal is about, by the look's own numbers, in the
/// same request as the action. A candidate the look could not describe is
/// not offered, no head carries a selector the page wrote, and an answer
/// naming another head's candidate, a number the look did not hold or a
/// selector is refused whole.
#[test]
fn observation_heads_select_only_the_observed_container_image_and_row() {
    let items = [item(1, "button", "More")];
    let containers = [
        json!({ "label": "Search results", "role": "list", "count": 36, "selector": "#product-list" }),
        json!({ "label": "Recommended", "role": "list", "count": 8, "selector": "#carousel" }),
    ];
    let images = [
        json!({ "alt": "iPhone 16 Pro", "width": 230, "height": 230, "selector": "#p1 img" }),
        json!({ "selector": "#p2 img" }),
        json!({ "alt": "갈비탕", "width": 230, "height": 230, "selector": "#ad img" }),
    ];
    let rows = [
        json!({ "text": "iPhone 16 Pro 256GB", "selector": "#p1" }),
        json!({ "text": "구운란 30구", "selector": "#ad" }),
    ];
    let beside = Beside {
        containers: &containers,
        images: &images,
        rows: &rows,
        ..Beside::default()
    };
    let asked = ask_with(&a_goal(&items, &[]), &beside).expect("asks");

    // Whatever order the map keeps its keys in, each head offers exactly
    // these.
    let offered = |head: &str| -> BTreeSet<String> {
        asked.questions[head]["criteria"]
            .as_object()
            .unwrap_or_else(|| panic!("{head} is asked"))
            .keys()
            .cloned()
            .collect()
    };
    let set = |options: &[&str]| -> BTreeSet<String> {
        options.iter().map(|option| (*option).to_string()).collect()
    };
    assert_eq!(
        offered("container"),
        set(&["container:1", "container:2", NONE])
    );
    assert_eq!(
        offered("image"),
        set(&["image:1", "image:3", NONE]),
        "image 2 says nothing"
    );
    assert_eq!(offered("row"), set(&["row:1", "row:2", NONE]));
    // The same lines stand in the state, under the head's own key, so the
    // judgment reads the page's candidates as it reads its controls.
    for head in Observe::ALL {
        let lines: BTreeSet<String> = asked.state[head.key()]
            .as_array()
            .unwrap_or_else(|| panic!("{} stands in the state", head.key()))
            .iter()
            .filter_map(|line| line.as_str().map(str::to_string))
            .collect();
        let options: BTreeSet<String> = asked.questions[head.head()]["criteria"]
            .as_object()
            .expect("criteria")
            .iter()
            .filter(|(option, _)| option.as_str() != NONE)
            .filter_map(|(_, line)| line.as_str().map(str::to_string))
            .collect();
        assert_eq!(lines, options, "{}", head.head());
    }
    let sent = format!("{}{}", asked.state, asked.questions);
    for selector in [
        "#product-list",
        "#carousel",
        "#p1 img",
        "#ad img",
        "\"#p1\"",
    ] {
        assert!(
            !sent.contains(selector),
            "{selector} is the look's, not the model's"
        );
    }
    for head in Observe::ALL {
        assert!(
            asked.questions[head.head()]["criteria"][NONE].is_string(),
            "{} says what none means",
            head.head()
        );
    }

    let read = asked
        .read_all(&answered_heads(
            &asked,
            &[
                ("action", "mark:1"),
                ("container", "container:1"),
                ("image", "image:3"),
                ("row", NONE),
            ],
        ))
        .expect("a whole answer");
    assert_eq!(read.choice.chosen, Chosen::Mark(1));
    let chose: Vec<(Observe, Option<usize>)> = read
        .observed
        .iter()
        .map(|observed| (observed.head, observed.chosen))
        .collect();
    assert_eq!(
        chose,
        [
            (Observe::Container, Some(1)),
            (Observe::Image, Some(3)),
            (Observe::Row, None)
        ]
    );

    // Every other head answered well, and one answered wrongly.
    for (head, wrong) in [
        ("container", "image:1"),
        ("image", "container:1"),
        ("image", "image:2"),
        ("row", "row:9"),
        ("container", "#product-list"),
    ] {
        let mut chosen = vec![
            ("action", "mark:1"),
            ("container", "container:1"),
            ("image", "image:3"),
            ("row", NONE),
        ];
        for answer in &mut chosen {
            if answer.0 == head {
                answer.1 = wrong;
            }
        }
        assert_eq!(
            asked.read_all(&answered_heads(&asked, &chosen)),
            Err(ActionRefusal::Choice(ChoiceRefusal::UnknownOption)),
            "{head} answered {wrong}"
        );
    }

    // A stopped walk asks no observation head, and a look that read nothing
    // beside its controls asks what it always asked.
    let stopped = ask_with(&a_look(&items, &[]), &beside).expect("asks");
    for head in Observe::ALL {
        assert!(stopped.questions.get(head.head()).is_none());
    }
    assert_eq!(
        ask_with(&a_goal(&items, &[]), &Beside::default()),
        ask(&a_goal(&items, &[]))
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
    assert_eq!(
        clear.read(&said),
        Err(ActionRefusal::Choice(ChoiceRefusal::UnknownOption))
    );
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
        ActionRefusal::Choice(ChoiceRefusal::UnknownOption)
    );
    assert_eq!(
        asked.choice_of("", 0.9).unwrap_err(),
        ActionRefusal::Choice(ChoiceRefusal::UnknownOption)
    );
    for not_a_share in [-0.1, 1.01, f64::NAN] {
        assert_eq!(
            asked.choice_of("mark:3", not_a_share).unwrap_err(),
            ActionRefusal::Choice(ChoiceRefusal::NotOne),
            "{not_a_share}"
        );
    }
}

/// Beside its choice a screen question asks two Nouls over the same state
/// (t-6187): whether the screen's text instructs an assistant what to do,
/// and whether the screen is a wall in front of the page the goal expects.
/// One request, the state charged once; each Noul says what yes and no mean.
#[test]
fn a_screen_question_asks_its_two_guards_beside_the_choice() {
    let items = [item(1, "button", "저장"), item(2, "link", "취소")];
    let asked = ask(&a_goal(&items, &[])).expect("a question");
    let questions = asked.questions.as_object().expect("questions");
    let mut names: Vec<&str> = questions.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["action", "instructed", "walled"]);
    for guard in ["instructed", "walled"] {
        assert_eq!(questions[guard]["type"], "noul", "{guard}");
        assert!(
            questions[guard]["instructions"]
                .as_str()
                .is_some_and(|words| !words.is_empty()),
            "{guard}"
        );
        assert!(
            questions[guard]["criteria"]["true"].is_string()
                && questions[guard]["criteria"]["false"].is_string(),
            "{guard}: a Noul says what yes and no mean"
        );
    }
}

/// The guards are read with the choice, each a probability of yes; a broken
/// guard discards the answer whole, by the guard's own word (t-6187). A
/// second reader's closed choice answers no guard at all.
#[test]
fn the_guards_are_read_with_the_choice_and_a_broken_one_refuses_the_answer() {
    let items = [item(1, "button", "저장"), item(2, "link", "취소")];
    let asked = ask(&a_goal(&items, &[])).expect("a question");
    let mut answers = answered(&asked, "mark:1", 0.8);
    answers["instructed"]["noul"] = json!(0.91);
    let read = asked.read(&answers).expect("a whole answer");
    assert_eq!(read.chosen, Chosen::Mark(1));
    assert_eq!(
        read.guard,
        Some(Guard {
            instructed: 0.91,
            walled: 0.03
        })
    );
    let mut blind = answers.clone();
    blind.as_object_mut().expect("answers").remove("walled");
    assert_eq!(
        asked.read(&blind),
        Err(ActionRefusal::Guard(NoulRefusal::NoAnswer))
    );
    let mut wild = answers.clone();
    wild["instructed"]["noul"] = json!(7);
    assert_eq!(
        asked.read(&wild),
        Err(ActionRefusal::Guard(NoulRefusal::OutOfRange))
    );
    assert_eq!(
        ActionRefusal::Guard(NoulRefusal::OutOfRange).token(),
        NoulRefusal::OutOfRange.token()
    );
    assert_eq!(
        asked.choice_of("mark:1", 0.9).expect("offered").guard,
        None,
        "a second reader answers no guard"
    );
}

/// A guard at its floor stops a press and one under it does not; an
/// instruction is named before a wall; a row keeps both per thousand under
/// the guards' own names (t-6187).
#[test]
fn a_guard_at_its_floor_stops_the_press_and_an_instruction_is_named_first() {
    let floor = f64::from(crate::jev::SCREEN_INSTRUCTED_FLOOR_PERMILLE) / 1000.0;
    let under = floor - crate::jev::ANSWER_STEP;
    assert_eq!(
        Guard {
            instructed: under,
            walled: under
        }
        .stops(),
        None
    );
    assert_eq!(
        Guard {
            instructed: floor,
            walled: 0.0
        }
        .stops(),
        Some(Stopped::Injected)
    );
    assert_eq!(
        Guard {
            instructed: 0.0,
            walled: floor
        }
        .stops(),
        Some(Stopped::Walled)
    );
    assert_eq!(
        Guard {
            instructed: 0.95,
            walled: 0.99
        }
        .stops(),
        Some(Stopped::Injected),
        "obeying the screen is the worse press"
    );
    assert_eq!(
        (Stopped::Injected.word(), Stopped::Walled.word()),
        ("injected", "walled")
    );
    assert_eq!(
        Guard {
            instructed: 0.91,
            walled: 0.03
        }
        .permille(),
        [("instructed", 910), ("walled", 30)]
    );
}
