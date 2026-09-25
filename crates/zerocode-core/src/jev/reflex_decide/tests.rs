use super::*;

/// A helper status on `plan` and `owner`, capture `capture`, with the ball's
/// newest sighting `value` and `done` actions so far.
fn status(plan: u64, owner: u64, capture: u64, value: i64, done: u64) -> Value {
    json!({
        "runId": "rx-1",
        "state": "running",
        "planHash": "an-app-name-free-hash",
        "sightings": [
            { "detector": "ball", "value": value, "unknown": null, "track": 5, "ageNs": 3_000_000 },
        ],
        "outcomes": { "done": done },
        "scene": { "stream": 1, "geometry": 1, "owner": owner, "plan": plan },
        "lastCapture": capture,
        "lastCaptureAgeNs": 3_000_000,
    })
}

/// The endpoint's body answering `word`.
fn answering(word: &str) -> Result<String, String> {
    let probabilities: Map<String, Value> = REFLEX_DECIDE_OPTIONS
        .iter()
        .map(|(option, _)| {
            (
                (*option).to_string(),
                json!(if *option == word { 0.8 } else { 0.1 }),
            )
        })
        .collect();
    Ok(json!({ "answers": { REFLEX_DECIDE_QUESTION: {
        "type": "choice", "choice": word, "probabilities": probabilities, "confidence": 0.7
    } }, "model": "jev-1.13.0" })
    .to_string())
}

fn sent(answer: Result<String, String>) -> Wired {
    Wired {
        answer,
        attempts: 1,
        request_bytes: 420,
        rtt_ms: 180,
    }
}

/// An answer that lands after the run moved to another plan is the teacher's
/// word on the state it was asked about — its state, its provenance — and says
/// that state is no longer the run's; the readings that came meanwhile are
/// merged or asked on their own, and none of them borrows the old answer.
#[test]
fn a_late_decision_cannot_rewrite_a_new_plan() {
    let mut decider = Decider::new();
    let Offer::Ask(asked) = decider.offer(snapshot_of(&status(1, 7, 10, 1, 2))) else {
        panic!("the first reading is asked");
    };
    // While it is in flight the run's plan changes, and two readings come.
    let Offer::Waiting { coalesced: None } = decider.offer(snapshot_of(&status(2, 7, 11, 0, 2)))
    else {
        panic!("the first reading behind a question waits");
    };
    let Offer::Waiting {
        coalesced: Some(merged),
    } = decider.offer(snapshot_of(&status(2, 7, 12, 1, 3)))
    else {
        panic!("a newer reading replaces the one waiting");
    };
    let now = snapshot_of(&status(2, 7, 12, 1, 3)).scene;
    let row = asked_row(
        "rx-1",
        &asked,
        &sent(answering("pause")),
        now.as_ref(),
        true,
        false,
    );
    assert_eq!(row["decision"], json!(asked.id));
    assert_eq!(
        row["state"], asked.snapshot.state,
        "the state asked, not the newest"
    );
    assert_eq!(row["provenance"]["plan"], json!(1));
    assert_eq!(row["provenance"]["capture"], json!(10));
    assert_eq!(row["chosen"], json!("pause"));
    assert_eq!(
        row["staleForCurrent"],
        json!(true),
        "the run's plan moved under it"
    );
    assert_eq!(row["applied"], json!(false));
    assert_eq!(row["labelSource"], json!("teacher"));
    // The newest reading is asked next, as itself.
    let next = decider
        .settled(asked.id)
        .expect("the reading waiting is asked next");
    assert_eq!(
        next.snapshot.state,
        snapshot_of(&status(2, 7, 12, 1, 3)).state
    );
    assert_ne!(next.snapshot.state, asked.snapshot.state);
    let merged_row = coalesced_row("rx-1", &merged);
    assert_eq!(merged_row["road"], json!(ROAD_COALESCED));
    assert!(
        merged_row.get("state").is_none() && merged_row.get("chosen").is_none(),
        "merged readings carry no answer"
    );
    // Every decision is one row: asked, merged, asked.
    assert_eq!((asked.id, merged.id, next.id), (1, 2, 3));
    // A reading waiting when the run ends leaves merged, never asked.
    decider.offer(snapshot_of(&status(2, 7, 13, 0, 4)));
    assert!(decider.close().is_some());
}

/// A newer capture of the same scene — the same run, stream, geometry, hold and
/// plan — keeps the answer: it is not stale, and every reading is not made late
/// by the frames that follow it.
#[test]
fn a_compatible_newer_frame_keeps_the_answer() {
    let mut decider = Decider::new();
    let Offer::Ask(asked) = decider.offer(snapshot_of(&status(1, 7, 10, 1, 2))) else {
        panic!("asked");
    };
    let later = snapshot_of(&status(1, 7, 25, 1, 5));
    let row = asked_row(
        "rx-1",
        &asked,
        &sent(answering("continue")),
        later.scene.as_ref(),
        true,
        false,
    );
    assert_eq!(row["outcome"], json!(ANSWERED));
    assert_eq!(row["staleForCurrent"], json!(false));
    // Another hold on the hand is another scene.
    let taken = snapshot_of(&status(1, 8, 26, 1, 5));
    let stale = asked_row(
        "rx-1",
        &asked,
        &sent(answering("continue")),
        taken.scene.as_ref(),
        true,
        false,
    );
    assert_eq!(stale["staleForCurrent"], json!(true));
}

/// A question that left counts whatever became of it — a wire failure, a late
/// run, a malformed answer or one naming no option — with its bytes and its
/// round trip kept; one the door refused never left.
#[test]
fn every_decision_leaves_one_row_and_a_sent_one_keeps_its_cost() {
    let mut decider = Decider::new();
    let Offer::Ask(asked) = decider.offer(snapshot_of(&status(1, 7, 10, 1, 2))) else {
        panic!("asked");
    };
    for (answer, outcome) in [
        (Err("timeout".to_string()), "timeout"),
        (Err("http_503".to_string()), "http_503"),
        (Ok("{".to_string()), "schema_no_answer"),
        (answering("retreat"), "schema_unknown_option"),
    ] {
        let row = asked_row("rx-1", &asked, &sent(answer), None, true, false);
        assert_eq!(row["outcome"], json!(outcome));
        assert_eq!(row["road"], json!(ROAD_JEV));
        assert_eq!(row["attempts"], json!(1));
        assert_eq!(row["requestBytes"], json!(420));
        assert_eq!(row["rttMs"], json!(180));
        assert!(row.get("chosen").is_none());
    }
    let refused = asked_row(
        "rx-1",
        &asked,
        &Wired {
            answer: Err("not_consented".into()),
            attempts: 0,
            request_bytes: 0,
            rtt_ms: 0,
        },
        None,
        true,
        false,
    );
    assert_eq!(refused["road"], json!(ROAD_DOOR));
    assert_eq!(refused["attempts"], json!(0));
    // Switched off after it left: counted, its answer not kept.
    let withdrawn = asked_row(
        "rx-1",
        &asked,
        &sent(answering("pause")),
        None,
        false,
        false,
    );
    assert_eq!(withdrawn["outcome"], json!(WITHDRAWN));
    assert_eq!(withdrawn["attempts"], json!(1));
    assert!(withdrawn.get("chosen").is_none() && withdrawn.get("probabilities").is_none());
    // An answer after the run ended is kept, and says so.
    let late = asked_row("rx-1", &asked, &sent(answering("replan")), None, true, true);
    assert_eq!(late["late"], json!(true));
    assert_eq!(late["chosen"], json!("replan"));
    // The same reading again is no decision.
    assert_eq!(
        decider.offer(snapshot_of(&status(1, 7, 11, 1, 2))),
        Offer::Same
    );
}

/// The state the question carries is the typed state alone: the table's
/// detectors at most, each named by its plan id — a name that is not an id is
/// left out whole — and the outcome counts; the status's other fields stay
/// home.
#[test]
fn the_state_is_the_sightings_and_the_outcomes_alone() {
    let mut raw = status(1, 7, 10, 1, 2);
    let many: Vec<Value> = (0..LIMITS.max_detectors + 3)
        .map(|at| json!({ "detector": format!("d{at}"), "value": 1, "unknown": null, "track": at, "ageNs": 1 }))
        .collect();
    raw["sightings"] = json!(many);
    raw["sightings"][0]["detector"] = json!("Finder window: invoices.pdf");
    raw["outcomes"]["not an id"] = json!(3);
    let snapshot = snapshot_of(&raw);
    // As a set: a build decides whether a map keeps its insertion order.
    let keys: BTreeSet<&str> = snapshot
        .state
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, REFLEX_DECIDE_STATE_KEYS.into_iter().collect());
    let sightings = snapshot.state["sightings"].as_array().expect("sightings");
    assert_eq!(sightings.len() as u64, LIMITS.max_detectors);
    assert_ne!(
        sightings[0]["detector"],
        json!("Finder window: invoices.pdf"),
        "a name that is not an id is left out"
    );
    assert_eq!(snapshot.state["outcomes"], json!({ "done": 2 }));
    let text = snapshot.state.to_string();
    assert!(!text.contains("planHash") && !text.contains("an-app-name-free-hash"));
    assert_eq!(
        questions()[REFLEX_DECIDE_QUESTION]["criteria"]
            .as_object()
            .map(Map::len),
        Some(3)
    );
}
