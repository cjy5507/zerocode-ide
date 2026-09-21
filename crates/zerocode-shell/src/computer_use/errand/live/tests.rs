//! The browser judge over the window's wire: a deadline that bounds the whole
//! call, a body read only through the question that was asked, and the Jev
//! door before any socket. Every case here crosses a real socket or none at
//! all.

use std::net::TcpListener;
use std::time::Duration;

use serde_json::json;
use zerocode_core::computer_recipe::RecipeStop;
use zerocode_core::jev::choice::ChoiceRefusal;
use zerocode_core::jev::door::{REQUESTS_KEY, Refused};
use zerocode_core::screen_action::{ActionLook, Chosen, Errand as Asked, Where, ask};

use super::*;
use crate::api_routers::HeldKeys;
use crate::computer_use::errand::tests::{FakeWorld, stopped};
use crate::computer_use::errand::{ActionJudge, Mode, run};
use crate::systemone::tests::Endpoint;
use crate::systemone::{
    INVALID_REQUEST, RATE_LIMITED, SYSTEMONE_MODEL, SYSTEMONE_PATH, TIMEOUT, TRANSPORT,
    UNAUTHORIZED,
};

/// The question every case here asks.
fn asked() -> ActionAsk {
    asked_about("settings smoke", "다시 시도")
}

/// A question about `goal` whose first control reads `label`.
fn asked_about(goal: &str, label: &str) -> ActionAsk {
    let items = vec![
        json!({ "mark": 1, "role": "button", "label": label, "centerX": 10.0, "centerY": 20.0 }),
        json!({ "mark": 2, "role": "button", "label": "닫기", "centerX": 30.0, "centerY": 20.0 }),
    ];
    ask(&ActionLook {
        goal,
        errand: Asked::Clear {
            stopped: "step_failed",
            step: "click",
            refusal: "nothing matched",
        },
        at: Where::Page {
            host: "app.local",
            path: "/settings",
        },
        tried: &[],
        items: &items,
        pressed: &[],
        shows: &[],
    })
    .expect("a screen with controls asks")
}

/// A door the window may send through: zo's settings in a folder of the
/// case's own, consenting `consented` for a walk that runs in `work`.
fn door_in(home: &tempfile::TempDir, consented: &str) -> Doorway {
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("a workspace");
    let settings = home.path().join("settings.json");
    let root = home.path().join(consented);
    std::fs::write(
        &settings,
        json!({ "smart": { "jev": { "workspaces": [root.display().to_string()] } } }).to_string(),
    )
    .expect("zo's settings");
    Doorway {
        settings: Some(settings),
        workspace: Some(work),
        ..Doorway::default()
    }
}

/// A judge at `base` with a key, behind a door that consents the walk's
/// workspace. The home outlives the judge's calls.
fn consented_judge(base: &str) -> (tempfile::TempDir, LiveJudge) {
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_in(&home, "work");
    (home, LiveJudge::at(base, "test-key", door))
}

/// A body the endpoint would answer with.
fn body_choosing(choice: &str) -> String {
    json!({
        "model": SYSTEMONE_MODEL,
        "answers": {
            "action": {
                "type": "choice",
                "choice": choice,
                "probabilities": { "mark:1": 0.7, "mark:2": 0.2, "give_up": 0.1 },
                "confidence": 0.7,
            }
        },
        "usage": { "input_tokens": 120, "output_tokens": 0 },
    })
    .to_string()
}

#[test]
fn a_body_is_read_only_through_the_question_that_was_asked() {
    let asked = asked();

    let Judged::Chose(choice) = read_body(&asked, &body_choosing("mark:1")) else {
        panic!("a well formed body is a choice");
    };
    assert_eq!(choice.chosen, Chosen::Mark(1));

    // A body nothing could read is the wire's own word; an answer that broke
    // a rule of the closed choice names THAT rule, so a row can tell a sum
    // the wire's rounding moved from a number nobody offered.
    for (bad, refused) in [
        ("not json at all".to_string(), SCHEMA),
        ("{}".to_string(), SCHEMA),
        (
            json!({ "answers": {} }).to_string(),
            ChoiceRefusal::NoAnswer.token(),
        ),
        // A number the question never offered, in an otherwise perfect body.
        (
            body_choosing("mark:9"),
            ChoiceRefusal::UnknownOption.token(),
        ),
    ] {
        assert_eq!(
            read_body(&asked, &bad),
            Judged::Refused(refused.to_string()),
            "this body should have said nothing: {bad}"
        );
    }
}

#[test]
fn the_request_carries_the_questions_own_state_and_the_vendors_model() {
    let asked = asked();
    let sent = request_of(&asked);

    assert_eq!(sent["model"], json!(SYSTEMONE_MODEL));
    assert_eq!(sent["state"], asked.state);
    assert_eq!(sent["questions"], asked.questions);
}

#[test]
fn without_a_key_nothing_is_sent() {
    let keys = HeldKeys::default();
    let mut judge = LiveJudge::new(&keys, None, &zerocode_core::jev::BROWSER);
    assert!(!judge.armed());

    // A server that would answer, and is never reached.
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let mut pointed = LiveJudge::at(&endpoint.base(), "", door_in(&home, "work"));

    let no_key = || Judged::Refused(Refused::NoKey.token().to_string());
    assert_eq!(judge.choose(&asked()), no_key());
    assert_eq!(pointed.choose(&asked()), no_key());
    assert!(endpoint.asked().is_empty(), "no key, no socket");
}

#[test]
fn a_key_the_pane_kept_is_the_key_the_wire_presents() {
    let keys = HeldKeys::default();
    crate::typesafe_settings::save_key("secret-key", &keys).expect("the fake keychain takes it");

    let judge = LiveJudge::new(&keys, None, &zerocode_core::jev::BROWSER);
    assert!(judge.armed(), "the pane's key is the wire's key");
}

#[test]
fn a_real_socket_answering_the_contract_is_a_choice() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:2"), 0);
    let (_home, mut judge) = consented_judge(&endpoint.base());

    let Judged::Chose(choice) = judge.choose(&asked()) else {
        panic!("the contract's own answer is a choice");
    };
    assert_eq!(choice.chosen, Chosen::Mark(2));

    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one call, not a retry");
    assert!(heard[0].contains(&format!("POST {SYSTEMONE_PATH}")));
    assert!(heard[0].contains("authorization: Bearer test-key"));
    assert!(heard[0].contains("app.local"), "the state went with it");
}

#[test]
fn a_refused_status_is_one_call_and_its_word() {
    for (status, token) in [
        ("HTTP/1.1 401 Unauthorized", UNAUTHORIZED.to_string()),
        (
            "HTTP/1.1 422 Unprocessable Entity",
            INVALID_REQUEST.to_string(),
        ),
        ("HTTP/1.1 429 Too Many Requests", RATE_LIMITED.to_string()),
        ("HTTP/1.1 503 Service Unavailable", "http_503".to_string()),
    ] {
        let endpoint = Endpoint::serving(status, "{}".to_string(), 0);
        let (_home, mut judge) = consented_judge(&endpoint.base());

        assert_eq!(judge.choose(&asked()), Judged::Refused(token.clone()));
        assert_eq!(
            endpoint.asked().len(),
            1,
            "a recovery inside a stopped walk's budget does not wait twice ({token})"
        );
    }
}

#[test]
fn a_server_slower_than_the_deadline_is_a_timeout_not_a_hang() {
    let past = u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(0) + 500;
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), past);
    let (_home, mut judge) = consented_judge(&endpoint.base());

    let began = std::time::Instant::now();
    assert_eq!(judge.choose(&asked()), Judged::Refused(TIMEOUT.to_string()));
    let elapsed = began.elapsed();
    println!("judgment call elapsed: {elapsed:?}");
    assert!(
        elapsed < ACTION_DEADLINE + Duration::from_millis(400),
        "the deadline bounds the whole call: {elapsed:?}"
    );
}

#[test]
fn a_socket_that_answers_nothing_is_transport() {
    // A seat nobody is sitting in: the connection is refused at once.
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback seat");
    let addr = listener.local_addr().expect("its address");
    drop(listener);
    let (_home, mut judge) = consented_judge(&format!("http://{addr}"));

    assert_eq!(
        judge.choose(&asked()),
        Judged::Refused(TRANSPORT.to_string())
    );
}

/// A walk in a workspace nobody consented to asks nothing: the door refuses
/// before a socket opens, and the judge says it sent nothing.
#[test]
fn a_workspace_nobody_consented_to_is_refused_before_a_socket() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door_in(&home, "elsewhere"));

    assert_eq!(
        judge.choose(&asked()),
        Judged::Refused(Refused::NotConsented.token().to_string())
    );
    assert!(endpoint.asked().is_empty(), "nothing left the door");
    assert_eq!(
        judge.spent(),
        Some(Spent {
            requests: 0,
            redacted_lines: 0
        })
    );

    let mut nowhere = LiveJudge::at(&endpoint.base(), "test-key", Doorway::default());
    assert_eq!(
        nowhere.choose(&asked()),
        Judged::Refused(Refused::NotConsented.token().to_string()),
        "a walk no settings or workspace can be found for consents to nothing"
    );
}

/// The directory a walk was asked from is the workspace the door consents by.
/// Read back from the header the Computer Use door writes, a folder under a
/// consented root is judged — one request leaves, the number is pressed and
/// the walk goes on — and a folder beside it is not: no socket opens, nothing
/// is pressed, and the ledger's row says `not_consented` with nothing sent.
#[test]
fn the_folder_a_walk_was_asked_from_is_the_workspace_the_door_consents_by() {
    let home = tempfile::tempdir().expect("a zo home");
    let consented = door_in(&home, "work");
    let inside = home.path().join("work").join("화면 복구");
    let beside = home.path().join("elsewhere");
    for folder in [&inside, &beside] {
        std::fs::create_dir_all(folder).expect("a folder to ask from");
    }
    // What the bridge hands the walk: the door's header, read back.
    let asked_from = |folder: &Path| {
        let spelled =
            zerocode_core::computer_use::cwd_header_value(folder.to_str().expect("a UTF-8 folder"));
        zerocode_core::computer_use::cwd_from_header(&spelled).map(PathBuf::from)
    };
    let at = stopped(RecipeStop::CheckFailed);

    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let mut judge = LiveJudge::at(
        &endpoint.base(),
        "test-key",
        Doorway {
            workspace: asked_from(&inside),
            ..consented.clone()
        },
    );
    let mut world = FakeWorld::showing(&[1, 2]);
    let recovered = run(Mode::On, true, &at, &mut judge, &mut world);
    assert_eq!(endpoint.asked().len(), 1, "one request left the door");
    assert_eq!(world.presses, [1]);
    assert_eq!(recovered.rows.len(), 1, "{:?}", recovered.rows);
    assert_eq!(recovered.rows[0]["outcome"], json!("answered"));
    assert_eq!(recovered.rows[0][REQUESTS_KEY], json!(1));
    assert_eq!(recovered.rows[0]["recheck"], json!(true));
    assert!(recovered.report.is_some(), "the walk went on");

    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let mut judge = LiveJudge::at(
        &endpoint.base(),
        "test-key",
        Doorway {
            workspace: asked_from(&beside),
            ..consented
        },
    );
    let mut world = FakeWorld::showing(&[1, 2]);
    let recovered = run(Mode::On, true, &at, &mut judge, &mut world);
    assert!(endpoint.asked().is_empty(), "nothing left the door");
    assert!(world.presses.is_empty(), "nothing was pressed");
    assert_eq!(recovered.rows.len(), 1, "{:?}", recovered.rows);
    assert_eq!(
        recovered.rows[0]["outcome"],
        json!(Refused::NotConsented.token())
    );
    assert_eq!(recovered.rows[0][REQUESTS_KEY], json!(0));
    assert!(
        recovered.report.is_none(),
        "the walk stops where it stopped"
    );
}

/// A credential on the screen — in the goal, in a control's label — never
/// reaches the wire, and the judge counts the lines the door withheld.
#[test]
fn a_credential_on_the_screen_never_reaches_the_wire() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let (_home, mut judge) = consented_judge(&endpoint.base());

    let question = asked_about(
        "sign in\npassword: hunter2-SENTINEL",
        "token=sk-live-SENTINEL",
    );
    let Judged::Chose(choice) = judge.choose(&question) else {
        panic!("the contract's own answer is a choice");
    };
    assert_eq!(choice.chosen, Chosen::Mark(1));

    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1);
    assert!(
        !heard[0].contains("SENTINEL"),
        "a credential reached the wire: {}",
        heard[0]
    );
    // The goal's line, and the label as a candidate and as its option.
    assert_eq!(
        judge.spent(),
        Some(Spent {
            requests: 1,
            redacted_lines: 3
        })
    );
}

/// What one screen question costs and chooses against the real endpoint.
///
/// Not part of the gate — it spends a person's key and crosses the internet.
/// It reads a real marked look from `ZEROCODE_JEV_BENCH_LOOK` (a
/// `zerocode-computer observe --marks --json` answer, or a browser pane's
/// `marks --json`), the goal from `ZEROCODE_JEV_BENCH_GOAL`, the key from
/// `ZEROCODE_JEV_BENCH_KEY`, and how many rounds from
/// `ZEROCODE_JEV_BENCH_ROUNDS`. It prints each round's milliseconds and what
/// it chose, and asserts nothing about the answer: what a judgment picks on
/// our own screens is the thing being measured, not a thing to assume.
#[test]
#[ignore = "spends a real key on the real endpoint"]
fn what_one_screen_question_costs_against_the_real_endpoint() {
    let look = std::env::var("ZEROCODE_JEV_BENCH_LOOK").expect("a marked look's json");
    let goal = std::env::var("ZEROCODE_JEV_BENCH_GOAL").expect("a goal sentence");
    let key = std::env::var("ZEROCODE_JEV_BENCH_KEY").expect("a TypeSafe key");
    let rounds: usize = std::env::var("ZEROCODE_JEV_BENCH_ROUNDS")
        .ok()
        .and_then(|rounds| rounds.parse().ok())
        .unwrap_or(5);
    let cap: usize = std::env::var("ZEROCODE_JEV_BENCH_CANDIDATES")
        .ok()
        .and_then(|cap| cap.parse().ok())
        .unwrap_or(0);

    let said: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&look).expect("the look")).expect("json");
    let said = said.get("result").unwrap_or(&said);
    let (items, app, window) = match said.pointer("/marks/items") {
        Some(items) => (
            items.as_array().expect("items").clone(),
            said.pointer("/marks/app")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            said.pointer("/tree/window/title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        None => (
            said["items"].as_array().expect("items").clone(),
            String::new(),
            String::new(),
        ),
    };
    // A cap of the caller's own, so the cost of a wider question can be read
    // off the same screen as the cost of today's.
    let items = if cap > 0 && cap < items.len() {
        items[..cap].to_vec()
    } else {
        items
    };

    let home = tempfile::tempdir().expect("a zo home");
    let door = door_in(&home, "work");
    let asked = ask(&ActionLook {
        goal: &goal,
        errand: zerocode_core::screen_action::Errand::Goal,
        at: Where::Desk {
            app: &app,
            window: &window,
        },
        tried: &[],
        items: &items,
        pressed: &[],
        shows: &[],
    })
    .expect("a screen with controls asks");
    println!(
        "screen: {} items, question offers {} + give_up + done",
        items.len(),
        asked.marks().len()
    );

    let base = std::env::var("ZO_SYSTEMONE_BASE_URL")
        .unwrap_or_else(|_| "https://api.typesafe.ai".to_string());
    // The wire itself rather than the judge, so a refusal can say WHICH rule
    // the answer broke: the judge's ledger token is `schema` for all of them.
    let wire = crate::systemone::Wire::at(&base, &key, door.settings.clone());
    let mut millis = Vec::new();
    for round in 1..=rounds {
        let began = std::time::Instant::now();
        let sent = wire.ask(
            &zerocode_core::jev::DESKTOP,
            door.workspace.as_deref(),
            request_of(&asked),
            ACTION_DEADLINE,
        );
        let took = began.elapsed().as_secs_f64() * 1_000.0;
        millis.push(took);
        let said = match &sent.answer {
            Err(token) => format!("refused {token}"),
            Ok(body) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
                match asked.read(parsed.get("answers").unwrap_or(&serde_json::Value::Null)) {
                    Ok(choice) => {
                        format!("{:?} confidence {:.3}", choice.chosen, choice.confidence)
                    }
                    Err(why) => format!("schema · {} · {body}", why.reason()),
                }
            }
        };
        println!("round {round}: {took:.0} ms · {said}");
    }
    millis.sort_by(f64::total_cmp);
    println!(
        "{} rounds · min {:.0} ms · p50 {:.0} ms · max {:.0} ms",
        millis.len(),
        millis.first().copied().unwrap_or_default(),
        millis[millis.len() / 2],
        millis.last().copied().unwrap_or_default(),
    );
}
