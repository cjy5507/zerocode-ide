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

/// A forked step's question, as the walk would build one.
fn compared_ask() -> zerocode_core::branching::BranchAsk {
    use zerocode_core::branching::{BranchLook, Candidate, Outcome, ask as ask_branch};
    let candidates = [
        Candidate {
            mark: 2,
            action: "2 button 설정 @102,40".into(),
            result: Some(Outcome {
                moved: false,
                controls: vec![],
                count: 2,
            }),
        },
        Candidate {
            mark: 1,
            action: "1 button Wi-Fi @101,40".into(),
            result: Some(Outcome {
                moved: true,
                controls: vec!["3 button 연결됨 @103,40".into()],
                count: 3,
            }),
        },
    ];
    ask_branch(&BranchLook {
        goal: "Wi-Fi 설정을 열어라",
        at: zerocode_core::screen_action::Where::Phone {
            platform: "android",
            device: "Pixel_6",
        },
        before: &[],
        candidates: &candidates,
    })
    .expect("two candidates ask")
}

/// A body the endpoint would answer a comparison with.
fn body_comparing(choice: &str) -> String {
    json!({
        "model": SYSTEMONE_MODEL,
        "answers": {
            "best": {
                "type": "choice",
                "choice": choice,
                "probabilities": { "mark:2": 0.2, "mark:1": 0.8 },
                "confidence": 0.8,
            }
        },
        "usage": { "input_tokens": 140, "output_tokens": 0 },
    })
    .to_string()
}

/// The comparison goes down the same wire under the branching seat's own
/// row (t-6044): the door reads that row's consent, the request carries the
/// fork's state, and the answer is read by the fork's own question — a
/// refusal by its word, a body the contract answered as a choice.
#[test]
fn a_comparison_is_asked_under_the_branching_row_and_read_by_its_own_question() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_comparing("mark:1"), 0);
    let (_home, mut judge) = consented_judge(&endpoint.base());

    let Compared::Chose(choice) = judge.compare(&compared_ask()) else {
        panic!("the contract's own answer is a choice");
    };
    assert_eq!(choice.mark, 1);
    assert_eq!(choice.confidence, 0.8);
    assert_eq!(
        judge.spent(),
        Some(Spent {
            requests: 1,
            redacted_lines: 0
        })
    );
    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one call, not a retry");
    assert!(heard[0].contains(&format!("POST {SYSTEMONE_PATH}")));
    assert!(heard[0].contains("Wi-Fi"), "the fork's state went with it");
    assert!(
        heard[0].contains("\\\"best\\\"") || heard[0].contains("\"best\""),
        "the fork's own question was asked:\n{}",
        heard[0]
    );

    // A number the question never offered is refused by the rule's word.
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_comparing("mark:9"), 0);
    let (_home, mut judge) = consented_judge(&endpoint.base());
    assert_eq!(
        judge.compare(&compared_ask()),
        Compared::Refused(ChoiceRefusal::UnknownOption.token().to_string())
    );

    // A shut door sends nothing: no key is the door's first question.
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_comparing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let mut keyless = LiveJudge::at(&endpoint.base(), "", door_in(&home, "work"));
    assert_eq!(
        keyless.compare(&compared_ask()),
        Compared::Refused(Refused::NoKey.token().to_string())
    );
    assert!(endpoint.asked().is_empty(), "no key, no socket");
}

// ---- the judgment cache (t-6132) ------------------------------------------

/// A body the endpoint would answer a GOAL walk's question with: the goal
/// offers `done` beside the marks and `give_up`, and the closed choice's
/// rules want every offered option named.
fn goal_body_choosing(choice: &str) -> String {
    json!({
        "model": SYSTEMONE_MODEL,
        "answers": {
            "action": {
                "type": "choice",
                "choice": choice,
                "probabilities": { "mark:1": 0.7, "mark:2": 0.1, "give_up": 0.1, "done": 0.1 },
                "confidence": 0.7,
            }
        },
        "usage": { "input_tokens": 120, "output_tokens": 0 },
    })
    .to_string()
}

/// A door whose settings also set the judgment cache to `cache`, consenting
/// the walk's own workspace; `None` leaves the cache key out — off, as any
/// settings file written before the seat existed.
fn door_caching(home: &tempfile::TempDir, cache: Option<&str>) -> Doorway {
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("a workspace");
    let settings = home.path().join("settings.json");
    let mut smart = json!({ "jev": { "workspaces": [work.display().to_string()] } });
    if let Some(cache) = cache {
        smart[zerocode_core::jev::JUDGMENT_CACHE.setting] = json!(cache);
    }
    std::fs::write(
        &settings,
        json!({ zerocode_core::jev::SMART_SETTINGS_KEY: smart }).to_string(),
    )
    .expect("zo's settings");
    Doorway {
        settings: Some(settings),
        workspace: Some(work),
        ..Doorway::default()
    }
}

fn memo_file(home: &tempfile::TempDir) -> std::path::PathBuf {
    home.path()
        .join(zerocode_core::jev::count::REQUESTS_DIR)
        .join(zerocode_core::jev::memo::MEMO_FILE)
}

fn cache_ledger(home: &tempfile::TempDir) -> std::path::PathBuf {
    home.path()
        .join(zerocode_core::jev::count::REQUESTS_DIR)
        .join(zerocode_core::jev::JUDGMENT_CACHE.ledger)
}

/// The day's count under `home`, whichever day the wire counted it in.
fn day_count(home: &tempfile::TempDir) -> u64 {
    std::fs::read_dir(home.path().join(zerocode_core::jev::count::REQUESTS_DIR))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("requests-"))
                .map(|entry| zerocode_core::jev::count::sent(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// Stand the judgment cache's `auto` up, the way its own judge would: one
/// rise written into its ledger.
fn raise_the_cache(home: &tempfile::TempDir) {
    crate::systemone::append_rows(
        &cache_ledger(home),
        &[json!({
            zerocode_core::jev::summary::AT.canonical: 1,
            zerocode_core::jev::summary::TRANSITION.canonical: zerocode_core::jev::promote::ROSE,
        })],
    );
}

/// The cache seat's rows a judge wrote, read back from its ledger.
fn cache_rows(judge: &mut LiveJudge, home: &tempfile::TempDir) -> Vec<serde_json::Value> {
    judge.write_memo_rows(None, 5);
    crate::systemone::read_rows(&cache_ledger(home))
        .into_iter()
        .filter(|row| zerocode_core::jev::summary::TRANSITION.read(row).is_none())
        .collect()
}

#[test]
fn a_cache_that_is_off_leaves_the_wire_every_byte_it_had() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door_caching(&home, None));
    let asked = asked();

    for _ in 0..2 {
        let Judged::Chose(choice) = judge.choose(&asked) else {
            panic!("the wire answers");
        };
        assert_eq!(choice.chosen, Chosen::Mark(1));
        assert!(!judge.cached());
        assert_eq!(judge.spent().map(|spent| spent.requests), Some(1));
    }
    assert_eq!(endpoint.asked().len(), 2, "every question went out");
    assert!(!memo_file(&home).exists(), "no memo file is written");
    assert!(
        cache_rows(&mut judge, &home).is_empty(),
        "no row of the cache's own"
    );
    assert_eq!(day_count(&home), 2);
}

#[test]
fn shadow_asks_the_wire_as_today_and_labels_the_memo_against_it() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let mut judge = LiveJudge::at(
        &endpoint.base(),
        "test-key",
        door_caching(&home, Some(zerocode_core::jev::JevMode::Shadow.key())),
    );
    let asked = asked();

    // The first question: a miss, remembered.
    assert!(matches!(judge.choose(&asked), Judged::Chose(_)));
    assert_eq!(zerocode_core::jev::memo::rows(&memo_file(&home)), 1);
    // The second: a hit the wire is still asked for, labeled by agreement.
    assert!(matches!(judge.choose(&asked), Judged::Chose(_)));
    assert!(!judge.cached(), "shadow never answers from the memo");
    assert_eq!(judge.spent().map(|spent| spent.requests), Some(1));
    assert_eq!(endpoint.asked().len(), 2);
    assert_eq!(day_count(&home), 2, "both requests left and were counted");

    let rows = cache_rows(&mut judge, &home);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["miss"], json!(true));
    assert!(
        rows[0].get("outcome").is_none(),
        "a miss is not an ask of the cache"
    );
    assert_eq!(rows[1]["outcome"], json!("answered"));
    assert_eq!(rows[1]["routeUse"], json!("shadow"));
    assert_eq!(
        rows[1][zerocode_core::jev::summary::AGREED.canonical],
        json!(true)
    );
    assert_eq!(rows[1][REQUESTS_KEY], json!(0));
    assert_eq!(rows[1]["seat"], json!(zerocode_core::jev::BROWSER.id));
    assert!(rows[1]["key"].as_str().is_some_and(|key| key.len() == 16));
}

#[test]
fn a_risen_auto_answers_the_same_bytes_from_the_memo_and_sends_nothing() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_caching(&home, Some(zerocode_core::jev::JevMode::Auto.key()));
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door.clone());
    let asked = asked();

    // An auto nobody has raised compares, like shadow.
    assert!(matches!(judge.choose(&asked), Judged::Chose(_)));
    assert!(matches!(judge.choose(&asked), Judged::Chose(_)));
    assert!(!judge.cached());
    assert_eq!(endpoint.asked().len(), 2);

    // Raised, the memo answers: nothing leaves, nothing is counted.
    raise_the_cache(&home);
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door);
    let Judged::Chose(choice) = judge.choose(&asked) else {
        panic!("the memo answers");
    };
    assert_eq!(choice.chosen, Chosen::Mark(1));
    assert!(judge.cached());
    assert_eq!(
        judge.spent(),
        Some(Spent {
            requests: 0,
            redacted_lines: 0
        })
    );
    assert_eq!(endpoint.asked().len(), 2, "the third question never left");
    assert_eq!(day_count(&home), 2, "and took no place in the day");
    let rows = cache_rows(&mut judge, &home);
    let hit = rows.last().expect("a row for the hit");
    assert_eq!(hit["outcome"], json!("answered"));
    assert_eq!(
        hit["routeUse"],
        json!(zerocode_core::jev::ROUTE_USE_APPLIED)
    );
    assert!(
        hit.get(zerocode_core::jev::summary::AGREED.canonical)
            .is_none(),
        "nothing to compare"
    );

    // A question with other bytes is a miss, and goes.
    let other = asked_about("settings smoke", "저장");
    assert!(matches!(judge.choose(&other), Judged::Chose(_)));
    assert!(!judge.cached());
    assert_eq!(endpoint.asked().len(), 3);
}

#[test]
fn a_memo_hit_still_passes_the_door_first() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_caching(&home, Some(zerocode_core::jev::JevMode::Auto.key()));
    let asked = asked();
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door.clone());
    assert!(matches!(judge.choose(&asked), Judged::Chose(_)));
    raise_the_cache(&home);

    // No key: refused before the memo, as ever.
    let mut keyless = LiveJudge::at(&endpoint.base(), "", door.clone());
    assert_eq!(
        keyless.choose(&asked),
        Judged::Refused(Refused::NoKey.token().to_string())
    );
    assert!(!keyless.cached());
    // A workspace nobody consented to: refused, memo or no memo.
    let elsewhere = Doorway {
        workspace: Some(home.path().join("elsewhere")),
        ..door
    };
    let mut stranger = LiveJudge::at(&endpoint.base(), "test-key", elsewhere);
    assert_eq!(
        stranger.choose(&asked),
        Judged::Refused(Refused::NotConsented.token().to_string())
    );
    assert!(
        cache_rows(&mut stranger, &home).is_empty(),
        "a refusal writes no cache row"
    );
    assert_eq!(endpoint.asked().len(), 1);
}

#[test]
fn a_remembered_answer_that_no_longer_reads_is_asked_afresh() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_caching(&home, Some(zerocode_core::jev::JevMode::Auto.key()));
    let asked = asked();
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door.clone());
    assert!(matches!(judge.choose(&asked), Judged::Chose(_)));
    raise_the_cache(&home);

    // The memo now holds an answer naming a number the question never offered.
    let memo = memo_file(&home);
    let key = crate::systemone::read_rows(&memo)[0]["key"]
        .as_str()
        .expect("the key")
        .to_string();
    std::fs::remove_file(&memo).expect("a fresh memo");
    zerocode_core::jev::memo::remember(
        &memo,
        &zerocode_core::jev::BROWSER,
        &key,
        &body_choosing("mark:9"),
        1,
    )
    .expect("a stale memory");

    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door);
    let Judged::Chose(choice) = judge.choose(&asked) else {
        panic!("the wire answers after the memo failed to read");
    };
    assert_eq!(choice.chosen, Chosen::Mark(1));
    assert!(!judge.cached());
    assert_eq!(judge.spent().map(|spent| spent.requests), Some(1));
    assert_eq!(endpoint.asked().len(), 2);
    let rows = cache_rows(&mut judge, &home);
    let stale = rows.last().expect("the stale hit's row");
    assert_eq!(
        stale["outcome"],
        json!(ChoiceRefusal::UnknownOption.token())
    );
    assert_eq!(
        stale["routeUse"],
        json!(zerocode_core::jev::ROUTE_USE_FALLBACK)
    );
    // And the fresh answer is what the memo holds now.
    assert_eq!(
        zerocode_core::jev::memo::recall(&memo, &key).map(|r| r.answer),
        Some(body_choosing("mark:1"))
    );
}

/// The walk's own row says the memo answered: `cached`, and no request.
#[test]
fn a_walk_answered_from_the_memo_says_so_on_its_row() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", goal_body_choosing("mark:1"), 0);
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_caching(&home, Some(zerocode_core::jev::JevMode::Auto.key()));
    let goal = crate::computer_use::errand::tests::goal(1);

    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door.clone());
    let mut world = FakeWorld::showing(&[1, 2]);
    let first = run(Mode::On, true, &goal, &mut judge, &mut world);
    assert_eq!(first.pressed, 1, "{:?}", first.rows);
    assert!(
        first.rows[0]
            .get(zerocode_core::jev::summary::CACHED.canonical)
            .is_none()
    );
    assert_eq!(first.rows[0][REQUESTS_KEY], json!(1));

    raise_the_cache(&home);
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door);
    let mut world = FakeWorld::showing(&[1, 2]);
    let second = run(Mode::On, true, &goal, &mut judge, &mut world);
    assert_eq!(second.pressed, 1, "the memo's number is pressed");
    assert_eq!(
        second.rows[0][zerocode_core::jev::summary::CACHED.canonical],
        json!(true)
    );
    assert_eq!(second.rows[0][REQUESTS_KEY], json!(0));
    assert_eq!(second.rows[0]["outcome"], json!("answered"));
    assert_eq!(endpoint.asked().len(), 1);
}

/// The measurement the seat is for: the same Flow walked twice, the second
/// time under a risen cache. Printed, and held on the one number that is a
/// promise — the second walk sends nothing.
#[test]
fn the_second_walk_of_the_same_flow_sends_nothing() {
    const HOLD_MS: u64 = 250;
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", goal_body_choosing("mark:1"), HOLD_MS);
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_caching(&home, Some(zerocode_core::jev::JevMode::Auto.key()));
    let goal = crate::computer_use::errand::tests::goal(3);

    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door.clone());
    let mut world = FakeWorld::that_moves(&[1, 2]);
    let began = std::time::Instant::now();
    let first = run(Mode::On, true, &goal, &mut judge, &mut world);
    let first_ms = began.elapsed().as_millis();
    assert_eq!(first.pressed, 3, "{:?}", first.rows);
    assert_eq!(endpoint.asked().len(), 3);

    raise_the_cache(&home);
    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door);
    let mut world = FakeWorld::that_moves(&[1, 2]);
    let began = std::time::Instant::now();
    let second = run(Mode::On, true, &goal, &mut judge, &mut world);
    let second_ms = began.elapsed().as_millis();
    assert_eq!(second.pressed, 3);
    assert_eq!(
        endpoint.asked().len(),
        3,
        "the second walk asked the wire nothing"
    );
    assert!(second.rows.iter().all(|row| {
        row[zerocode_core::jev::summary::CACHED.canonical] == json!(true)
            && row[REQUESTS_KEY] == json!(0)
    }));
    println!(
        "measure: judgment-cache flow=3 presses wire_hold_ms={HOLD_MS} first_run_ms={first_ms} first_wire_calls=3 second_run_ms={second_ms} second_wire_calls=0"
    );
}

/// What the memo would have agreed with, against the real endpoint: the same
/// screen question asked `ZEROCODE_JEV_BENCH_PASSES` times, the first answer
/// standing for what a memo remembers and every later one for the fresh
/// answer `shadow` compares it with. Prints the pooled share and a 95%
/// Wilson lower bound; asserts nothing about the answer. Not part of the
/// gate — it spends a person's key and crosses the internet. The look and
/// the goal are read as the bench above reads them.
#[test]
#[ignore = "spends a real key on the real endpoint"]
fn what_the_memo_would_have_agreed_with_against_the_real_endpoint() {
    let look = std::env::var("ZEROCODE_JEV_BENCH_LOOK").expect("a marked look's json");
    let goal = std::env::var("ZEROCODE_JEV_BENCH_GOAL").expect("a goal sentence");
    let key = std::env::var("ZEROCODE_JEV_BENCH_KEY").expect("a TypeSafe key");
    let passes: usize = std::env::var("ZEROCODE_JEV_BENCH_PASSES")
        .ok()
        .and_then(|passes| passes.parse().ok())
        .unwrap_or(5);
    let said: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&look).expect("the look")).expect("json");
    let said = said.get("result").unwrap_or(&said);
    let items = said
        .pointer("/marks/items")
        .or_else(|| said.get("items"))
        .and_then(serde_json::Value::as_array)
        .expect("items")
        .clone();
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_in(&home, "work");
    let asked = ask(&ActionLook {
        goal: &goal,
        errand: zerocode_core::screen_action::Errand::Goal,
        at: Where::Page {
            host: "bench.local",
            path: "/",
        },
        tried: &[],
        items: &items,
        pressed: &[],
        shows: &[],
    })
    .expect("a screen with controls asks");
    let mut judge = LiveJudge::at(
        &std::env::var("ZO_SYSTEMONE_BASE_URL")
            .unwrap_or_else(|_| crate::systemone::SYSTEMONE_BASE_URL.to_string()),
        &key,
        door,
    );
    let mut answers = Vec::new();
    let mut waits = Vec::new();
    for pass in 1..=passes {
        let began = std::time::Instant::now();
        let judged = judge.choose(&asked);
        let ms = began.elapsed().as_millis();
        waits.push(ms);
        match judged {
            Judged::Chose(choice) => {
                println!(
                    "pass {pass}: {:?} confidence {} in {ms} ms",
                    choice.chosen, choice.confidence
                );
                answers.push(Some(choice.chosen));
            }
            Judged::Refused(token) => {
                println!("pass {pass}: refused {token} in {ms} ms");
                answers.push(None);
            }
        }
    }
    let remembered = answers.first().cloned().flatten();
    let compared: Vec<bool> = answers
        .iter()
        .skip(1)
        .filter_map(|fresh| Some(fresh.as_ref()? == remembered.as_ref()?))
        .collect();
    let agreed = compared.iter().filter(|a| **a).count();
    let lower = zerocode_core::jev::summary::wilson_lower(
        agreed,
        compared.len(),
        zerocode_core::jev::summary::WILSON_Z_95,
    );
    waits.sort_unstable();
    println!(
        "measure: memo-agreement passes={passes} compared={} agreed={agreed} share={:.3} wilson_lower={lower:.3} p50_ms={} max_ms={}",
        compared.len(),
        if compared.is_empty() {
            0.0
        } else {
            agreed as f64 / compared.len() as f64
        },
        waits[waits.len() / 2],
        waits[waits.len() - 1]
    );
}

// ---- asking ahead over the wire (t-6132 S2) ---------------------------------

/// A judgment begun ahead of the walk runs down the same wire on a thread of
/// its own, and what it would have said of itself — the door's account, the
/// memo's rows — comes back to the judge that writes the rows: a walk with
/// `overlap` on a screen that stays writes exactly the rows a walk in turn
/// would, with the second judgment marked as used ahead.
#[test]
fn a_judgment_begun_ahead_runs_down_the_wire_and_its_account_comes_back() {
    // The first question offers both numbers; the second, mark 1 spent,
    // offers mark 2 alone — and the closed choice wants every offered option
    // named, so the endpoint answers each question in its own shape.
    let endpoint = Endpoint::answering_each(
        "HTTP/1.1 200 OK",
        |request: &str| {
            if request.contains("\"mark:1\"") {
                goal_body_choosing("mark:1")
            } else {
                json!({
                    "model": SYSTEMONE_MODEL,
                    "answers": {
                        "action": {
                            "type": "choice",
                            "choice": "mark:2",
                            "probabilities": { "mark:2": 0.8, "give_up": 0.1, "done": 0.1 },
                            "confidence": 0.8,
                        }
                    },
                    "usage": { "input_tokens": 120, "output_tokens": 0 },
                })
                .to_string()
            }
        },
        40,
    );
    let home = tempfile::tempdir().expect("a zo home");
    let door = door_caching(&home, Some(zerocode_core::jev::JevMode::Shadow.key()));
    let goal = crate::computer_use::errand::tests::goal(3);

    let mut judge = LiveJudge::at(&endpoint.base(), "test-key", door);
    let mut world = FakeWorld::showing(&[1, 2]);
    let walked = crate::computer_use::errand::run_with(
        Mode::On,
        true,
        crate::computer_use::errand::Branching::OFF,
        &goal,
        &mut judge,
        &mut world,
        crate::computer_use::errand::Options {
            overlap: true,
            rescue: false,
        },
        None,
    );
    assert_eq!(world.presses, vec![1, 2]);
    assert_eq!((walked.overlapped, walked.discarded), (1, 0));
    assert_eq!(
        endpoint.asked().len(),
        2,
        "two questions, one of them ahead"
    );
    let second = &walked.rows[1];
    assert_eq!(
        second[crate::computer_use::errand::OVERLAP],
        json!(crate::computer_use::errand::OVERLAP_USED)
    );
    assert_eq!(
        second[REQUESTS_KEY],
        json!(1),
        "the door's account of the question asked ahead"
    );
    assert_eq!(second["outcome"], json!("answered"));
    assert_eq!(second["chosen"], json!("mark:2"));
    // The cache seat's rows of both questions land, the ahead one's through
    // `finish`: two misses (each screen's bytes differ by `tried`/`pressed`).
    let rows = cache_rows(&mut judge, &home);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|row| row["miss"] == json!(true)));
    assert_eq!(zerocode_core::jev::memo::rows(&memo_file(&home)), 2);
}
