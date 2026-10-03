use super::*;
use crate::computer_use::{REFLEX_COLLECT_MS, REFLEX_MISSED_OUTCOMES, REFLEX_PRESSED_OUTCOME};

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
fn the_state_is_the_sightings_outcomes_activity_and_freshness_alone() {
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
    // A status carrying no receipts: nothing finished since the last reading.
    assert_eq!(
        snapshot.state["activity"],
        json!({ "done": 0, "missed": 0 }),
        "the activity of a reading with no receipts"
    );
    // Its capture is three milliseconds old against the table's own limit.
    assert_eq!(
        snapshot.state["freshness"],
        json!({
            "capture_age_ms": 3,
            "max_frame_age_ms": LIMITS.max_frame_age_ns / 1_000_000,
            "over_age": false,
        })
    );
    let text = snapshot.state.to_string();
    assert!(!text.contains("planHash") && !text.contains("an-app-name-free-hash"));
    assert_eq!(
        questions()[REFLEX_DECIDE_QUESTION]["criteria"]
            .as_object()
            .map(Map::len),
        Some(3)
    );
}

/// What the hand did since the last reading goes with the reading (t-22110):
/// the receipts a status carries — the ones not yet acknowledged, one
/// collect's worth — counted by the product's own outcome words, a landed
/// action `done` and a target the hand went for and lost `missed`; any other
/// outcome is neither. And the capture's age is held to the hand's own frame
/// limit, in whole milliseconds, with the limit itself beside it; a capture
/// nobody can date is not an old one — it is not measured.
#[test]
fn activity_counts_the_readings_receipts_and_freshness_holds_the_capture_to_the_hands_limit() {
    let mut raw = status(1, 7, 10, 1, 9);
    raw["receipts"] = json!([
        { "seq": 1, "actionId": "click1", "outcome": REFLEX_PRESSED_OUTCOME },
        { "seq": 2, "actionId": "move1", "outcome": REFLEX_MISSED_OUTCOMES[0] },
        { "seq": 3, "actionId": "move1", "outcome": REFLEX_MISSED_OUTCOMES[1] },
        { "seq": 4, "actionId": "move1", "outcome": REFLEX_MISSED_OUTCOMES[2] },
        { "seq": 5, "actionId": "move1", "outcome": REFLEX_MISSED_OUTCOMES[3] },
        { "seq": 6, "actionId": "click1", "outcome": REFLEX_PRESSED_OUTCOME },
        { "seq": 7, "actionId": "click1", "outcome": "stopped" },
        { "seq": 8, "actionId": "click1" },
    ]);
    raw["lastCaptureAgeNs"] = json!(LIMITS.max_frame_age_ns + 1_000_000);
    let snapshot = snapshot_of(&raw);
    assert_eq!(
        snapshot.state["activity"],
        json!({ "done": 2, "missed": 4 })
    );
    assert_eq!(
        snapshot.state["freshness"],
        json!({
            "capture_age_ms": LIMITS.max_frame_age_ns / 1_000_000 + 1,
            "max_frame_age_ms": LIMITS.max_frame_age_ns / 1_000_000,
            "over_age": true,
        })
    );
    // At the limit itself the capture is still young: the hand's own rule
    // (`observed <= now && now - observed <= max_frame_age_ns`).
    raw["lastCaptureAgeNs"] = json!(LIMITS.max_frame_age_ns);
    assert_eq!(
        snapshot_of(&raw).state["freshness"]["over_age"],
        json!(false)
    );
    // A capture nobody can date is not measured, and establishes nothing.
    raw.as_object_mut()
        .expect("a status")
        .remove("lastCaptureAgeNs");
    let undated = snapshot_of(&raw);
    assert_eq!(undated.state["freshness"]["capture_age_ms"], Value::Null);
    assert_eq!(undated.state["freshness"]["over_age"], Value::Null);
    assert_eq!(
        undated.state["freshness"]["max_frame_age_ms"],
        json!(LIMITS.max_frame_age_ns / 1_000_000)
    );
    // The receipts are no part of the state beyond their counts: no action
    // id and no sequence number goes to the model.
    let text = snapshot.state.to_string();
    assert!(!text.contains("click1") && !text.contains("\"seq\""));
    // Finding nothing is still the sightings' word alone.
    assert!(!finds_nothing(&snapshot.state));
}

// ---- what may be carried out, and what grades it (t-10223 §2.2) ---------------

/// The stamp a question's autopilot remembers of its run: epoch 3, plan `h3`,
/// the seat's own mode.
fn stamp() -> Stamp {
    Stamp {
        epoch: 3,
        plan_hash: "h3".into(),
        forced: false,
    }
}

/// An answer is carried out only about the run, epoch and plan running now,
/// on a reading no older than two collects, under a seat that applies — and
/// the grounds come before the seat's word, so a row under `shadow` still
/// says whether its answer would have been fit to carry out. A run that
/// ended, stopped or was re-planned is another epoch's; an undatable reading
/// is no young one.
#[test]
fn an_answer_is_carried_out_only_about_the_run_epoch_and_plan_running_on_a_young_reading() {
    use crate::computer_use::{REFLEX_APPLY_MAX_AGE_MS, REFLEX_COLLECT_MS};
    let running = Running {
        run: "rx-1",
        epoch: 3,
        plan_hash: "h3",
    };
    let young = Some(REFLEX_COLLECT_MS + 3);
    assert_eq!(
        verdict("rx-1", &stamp(), Some(running), young, false, true),
        Ok(())
    );
    assert_eq!(
        verdict(
            "rx-1",
            &stamp(),
            Some(running),
            Some(REFLEX_APPLY_MAX_AGE_MS),
            false,
            true
        ),
        Ok(()),
        "two collects is still the scene the hand acts in"
    );
    for (asked, running, age, applies, why) in [
        (
            "rx-1",
            Some(Running {
                epoch: 4,
                ..running
            }),
            young,
            true,
            Why::EpochMismatch,
        ),
        ("rx-0", Some(running), young, true, Why::EpochMismatch),
        ("rx-1", None, young, true, Why::EpochMismatch),
        (
            "rx-1",
            Some(Running {
                plan_hash: "another",
                ..running
            }),
            young,
            true,
            Why::PlanMismatch,
        ),
        (
            "rx-1",
            Some(running),
            Some(REFLEX_APPLY_MAX_AGE_MS + 1),
            true,
            Why::Stale,
        ),
        ("rx-1", Some(running), None, true, Why::Stale),
        ("rx-1", Some(running), young, false, Why::NotAuto),
        // The grounds before the seat's word.
        ("rx-0", Some(running), None, false, Why::EpochMismatch),
        ("rx-1", Some(running), None, false, Why::Stale),
    ] {
        assert_eq!(
            verdict(asked, &stamp(), running, age, false, applies),
            Err(why),
            "{asked} {running:?} {age:?} {applies}"
        );
    }
    let mut row = json!({ "applied": false });
    carried(&mut row, Ok(()));
    assert_eq!(row, json!({ "applied": true }));
    for why in Why::ALL {
        let mut row = json!({ "applied": true });
        carried(&mut row, Err(why));
        assert_eq!(row, json!({ "applied": false, "why": why.word() }));
    }
    assert_eq!(
        Why::ALL.map(Why::word),
        [
            "not_auto",
            "stale",
            "epoch_mismatch",
            "plan_mismatch",
            "idle"
        ]
    );
}

/// A pause about a hand with nothing to stop is never carried out: its
/// reading found nothing to press and the hand stood quiet. A pause about a
/// hand that sees a target or has been pressing is; so is any other answer.
/// Idleness is a ground, read before the seat's word, and after the run's.
#[test]
fn a_pause_about_a_hand_with_nothing_to_stop_is_idle() {
    let nothing = json!({ "sightings": [{ "detector": "red", "value": 0, "unknown": null }] });
    let blind =
        json!({ "sightings": [{ "detector": "red", "value": null, "unknown": "occluded" }] });
    let seen = json!({ "sightings": [{ "detector": "red", "value": 1, "unknown": null }] });
    assert!(idle(PAUSE, &nothing, true));
    assert!(idle(PAUSE, &blind, true));
    assert!(
        !idle(PAUSE, &seen, true),
        "a target in sight: the pause stops a hand"
    );
    assert!(
        !idle(PAUSE, &nothing, false),
        "a hand that was pressing is stopped"
    );
    assert!(!idle(CONTINUE, &nothing, true));
    assert!(!idle(REPLAN, &nothing, true));
    let running = Running {
        run: "rx-1",
        epoch: 3,
        plan_hash: "h3",
    };
    let young = Some(crate::computer_use::REFLEX_COLLECT_MS);
    assert_eq!(
        verdict("rx-1", &stamp(), Some(running), young, true, true),
        Err(Why::Idle)
    );
    assert_eq!(
        verdict("rx-1", &stamp(), Some(running), young, true, false),
        Err(Why::Idle),
        "a shadow row says the pause had nothing to stop"
    );
    assert_eq!(
        verdict("rx-0", &stamp(), Some(running), young, true, true),
        Err(Why::EpochMismatch)
    );
}

/// A reading's age is the time since the window read its status plus the age
/// its capture had then: an answer read one collect later about a capture 3 ms
/// old is 1,003 ms old. A reading nobody stamped, or whose capture's age the
/// status did not say, has no age.
#[test]
fn a_readings_age_is_the_time_since_it_was_read_plus_its_captures() {
    use crate::computer_use::REFLEX_COLLECT_MS;
    let mut snapshot = snapshot_of(&status(1, 7, 10, 1, 2));
    assert_eq!(snapshot.read_ms, None, "the reader stamps it");
    assert_eq!(age_at(&snapshot, 5_000), None);
    snapshot.read_ms = Some(5_000);
    assert_eq!(
        age_at(&snapshot, 5_000 + REFLEX_COLLECT_MS),
        Some(REFLEX_COLLECT_MS + 3)
    );
    snapshot.age_ns = None;
    assert_eq!(age_at(&snapshot, 6_000), None);
}

/// The autopilot's stamp rides every decision row's provenance and reads back
/// whole; a row that carries none has none.
#[test]
fn a_stamp_rides_the_rows_provenance() {
    let mut decider = Decider::new();
    let Offer::Ask(asked) = decider.offer(snapshot_of(&status(1, 7, 10, 1, 2))) else {
        panic!("asked");
    };
    let mut row = asked_row("rx-1", &asked, &sent(answering(PAUSE)), None, true, false);
    assert_eq!(Stamp::of(&row), None);
    let forced = Stamp {
        forced: true,
        ..stamp()
    };
    forced.stamp(&mut row);
    assert_eq!(Stamp::of(&row), Some(forced));
    assert_eq!(row["provenance"]["planHash"], json!("h3"));
    assert_eq!(row["provenance"]["capture"], json!(10), "the rest kept");
    assert_eq!(row["applied"], json!(false), "carried out by nobody yet");
}

/// Nothing is found when no detector reads a known value other than none:
/// every one unknown, reading no target, or no sighting at all.
#[test]
fn finding_nothing_is_every_reading_unknown_or_empty() {
    let state = |sightings: Value| json!({ "sightings": sightings, "outcomes": {} });
    assert!(finds_nothing(&state(json!([]))));
    assert!(finds_nothing(&json!({})));
    assert!(finds_nothing(&state(json!([
        { "detector": "a", "value": null, "unknown": "occluded" },
        { "detector": "b", "value": 0, "unknown": null },
        { "detector": "c", "value": 2, "unknown": "stale" },
    ]))));
    assert!(!finds_nothing(&state(json!([
        { "detector": "a", "value": null, "unknown": "occluded" },
        { "detector": "b", "value": 1, "unknown": null },
    ]))));
    assert_eq!([CONTINUE, PAUSE, REPLAN], ["continue", "pause", "replan"]);
}

fn screen(pressed: u64, missed: u64, blind: bool, live: bool) -> Window {
    Window {
        share: Share::Screen { pressed, missed },
        wrong: 0,
        blind,
        live,
    }
}

fn bench(hit: u64, offered: u64, wrong: u64, blind: bool) -> Window {
    Window {
        share: Share::Bench { hit, offered },
        wrong,
        blind,
        live: false,
    }
}

/// Every answer is graded on the window after it, whether or not it was
/// carried out (§2.2, D3): `continue` was right when the hand pressed some of
/// what it was offered and nothing else, and wrong when every reading found
/// nothing or it pressed none; `pause` was right when there was nothing to
/// press. A bench reads its share off its oracle — hits over the targets it
/// drew, wrong presses counted — and a screen off its receipts; a screen
/// shows a stopped hand nothing, a bench's oracle still counts what it drew.
#[test]
fn a_label_grades_the_window_after_an_answer_on_its_own_share() {
    // continue
    assert_eq!(graded(CONTINUE, &screen(3, 1, false, true)), Ok(true));
    assert_eq!(graded(CONTINUE, &screen(0, 4, false, true)), Ok(false));
    assert_eq!(
        graded(CONTINUE, &screen(0, 0, true, true)),
        Ok(false),
        "found nothing: pause was right"
    );
    assert_eq!(
        graded(CONTINUE, &screen(0, 0, false, true)),
        Err(NOTHING_OFFERED)
    );
    assert_eq!(graded(CONTINUE, &bench(9, 10, 0, false)), Ok(true));
    assert_eq!(
        graded(CONTINUE, &bench(9, 10, 1, false)),
        Ok(false),
        "a wrong press"
    );
    assert_eq!(
        graded(CONTINUE, &bench(0, 0, 0, false)),
        Err(NOTHING_OFFERED)
    );
    // pause
    assert_eq!(graded(PAUSE, &screen(0, 0, true, true)), Ok(true));
    assert_eq!(graded(PAUSE, &screen(2, 0, false, true)), Ok(false));
    assert_eq!(graded(PAUSE, &screen(0, 0, true, false)), Err(HAND_STOPPED));
    assert_eq!(
        graded(PAUSE, &bench(0, 0, 0, true)),
        Ok(true),
        "the oracle drew nothing"
    );
    assert_eq!(
        graded(PAUSE, &bench(0, 6, 0, false)),
        Ok(false),
        "a stopped hand, six targets drawn"
    );
    // a re-plan nobody carried out
    assert_eq!(
        graded(REPLAN, &screen(3, 1, false, true)),
        Err(NOT_REPLANNED)
    );
    // The baseline — `continue` — rides beside the seat's own mark, never alone.
    assert_eq!(
        marks_on(PAUSE, &screen(2, 0, false, true)),
        Ok((false, Some(true)))
    );
    assert_eq!(
        marks_on(PAUSE, &screen(0, 0, true, true)),
        Ok((true, Some(false)))
    );
    // Something was found and nothing pressed: pause was wrong, and continue
    // has no share to be graded on — no baseline mark.
    assert_eq!(
        marks_on(PAUSE, &screen(0, 0, false, true)),
        Ok((false, None))
    );
    assert_eq!(
        marks_on(REPLAN, &screen(3, 1, false, true)),
        Err(NOT_REPLANNED)
    );
    // A re-plan is graded across its two runs.
    let old = Share::Screen {
        pressed: 2,
        missed: 6,
    };
    let new = Share::Screen {
        pressed: 5,
        missed: 1,
    };
    assert_eq!(replan_graded(old, new), Ok(true));
    assert_eq!(replan_graded(new, old), Ok(false));
    assert_eq!(replan_marks(old, new), Ok((true, Some(false))));
    assert_eq!(
        replan_graded(
            old,
            Share::Screen {
                pressed: 0,
                missed: 0
            }
        ),
        Err(NOTHING_OFFERED)
    );
    // The two readings say which they are.
    assert_eq!(
        Share::Screen {
            pressed: 2,
            missed: 6
        }
        .json()["kind"],
        json!("screen")
    );
    assert_eq!(
        Share::Bench { hit: 2, offered: 6 }.json()["kind"],
        json!("bench")
    );
    assert_eq!(Share::Bench { hit: 3, offered: 4 }.ratio(), Some(0.75));
    assert_eq!(
        Share::Screen {
            pressed: 3,
            missed: 1
        }
        .ratio(),
        Some(0.75)
    );
}

/// A label names its request the way the seat's row says a request is named
/// — its run and its decision, and the time it was asked — and the judge
/// joins it there and nowhere else: two rows under one name are a replay
/// nothing tells apart, and a label naming another time grades nothing.
#[test]
fn a_label_names_its_request_by_run_decision_and_time_as_the_judge_joins_it() {
    use crate::jev::REFLEX_DECIDE;
    use crate::jev::promote::on_the_newest_version;
    let mut decider = Decider::new();
    let Offer::Ask(asked) = decider.offer(snapshot_of(&status(1, 7, 10, 1, 2))) else {
        panic!("asked");
    };
    let mut request = asked_row(
        "rx-9-1",
        &asked,
        &sent(answering(CONTINUE)),
        None,
        true,
        false,
    );
    stamp().stamp(&mut request);
    request["at"] = json!(1_000);
    let measured = Share::Screen {
        pressed: 4,
        missed: 0,
    }
    .json();
    let label = label_row(
        "rx-9-1",
        asked.id,
        1_000,
        measured.clone(),
        Ok((true, Some(true))),
    );
    assert_eq!(label["label"], json!(format!("rx-9-1:{}", asked.id)));
    assert_eq!(label["requestAt"], json!(1_000));
    assert_eq!(label["kind"], json!(LABEL_KIND));
    assert_eq!(label["share"], measured);
    assert!(label.get("outcome").is_none(), "a label is never a request");
    // The confidence of the answer it grades comes from its request.
    let graded = |rows: &[Value]| {
        on_the_newest_version(&REFLEX_DECIDE, rows)
            .graded()
            .into_iter()
            .map(|mark| (mark.agreed, mark.baseline, mark.confidence))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        graded(&[request.clone(), label.clone()]),
        [(true, Some(true), Some(0.7))],
        "the label grades its request"
    );
    // Another time names no request.
    let elsewhere = label_row(
        "rx-9-1",
        asked.id,
        999,
        measured.clone(),
        Ok((true, Some(true))),
    );
    assert!(graded(&[request.clone(), elsewhere]).is_empty());
    // Another run's decision of the same number is another request.
    let other = label_row(
        "rx-9-2",
        asked.id,
        1_000,
        measured.clone(),
        Ok((false, Some(false))),
    );
    assert!(graded(&[request.clone(), other]).is_empty());
    // Two rows under one name and one time: neither is graded.
    assert!(graded(&[request.clone(), request, label]).is_empty());
    // A label that cannot say carries the word, and no mark.
    let unsaid = label_row("rx-9-1", 2, 1_000, measured, Err(HAND_STOPPED));
    assert_eq!(unsaid["notCompared"], json!(HAND_STOPPED));
    assert!(unsaid.get("agreed").is_none() && unsaid.get("baselineAgreed").is_none());
}

/// A reading that differs from the one before it only by the capture's exact
/// age is the same reading (t-22110): the hand's captures jitter a few
/// milliseconds from collect to collect, and a run standing still would
/// otherwise ask every collect for nothing new. Past the hand's limit the
/// reading is new — `over_age` turned, which the words read.
#[test]
fn a_reading_that_differs_only_by_the_captures_exact_age_is_the_same_reading() {
    let mut decider = Decider::new();
    let mut raw = status(1, 7, 10, 1, 2);
    raw["lastCaptureAgeNs"] = json!(3_000_000);
    assert!(matches!(decider.offer(snapshot_of(&raw)), Offer::Ask(_)));
    raw["lastCaptureAgeNs"] = json!(7_000_000);
    assert_eq!(
        decider.offer(snapshot_of(&raw)),
        Offer::Same,
        "four milliseconds of jitter is no decision"
    );
    raw["lastCaptureAgeNs"] = json!(LIMITS.max_frame_age_ns + 1_000_000);
    assert!(
        matches!(
            decider.offer(snapshot_of(&raw)),
            Offer::Waiting { coalesced: None }
        ),
        "over the hand's limit the reading is new"
    );
}

// ---- asked ahead (t-32797) ---------------------------------------------------

/// A reading of two detectors on `plan` — `first` and `second` each a known
/// value or, for `None`, unknown — whose capture is `age_ms` old, with
/// `done` actions finished and `missed` lost since the reading before, and
/// `total` done so far.
fn reading(
    plan: u64,
    first: Option<i64>,
    second: Option<i64>,
    age_ms: u64,
    (done, missed, total): (u64, u64, u64),
) -> Snapshot {
    let sighting = |detector: &str, value: Option<i64>, track: u64| {
        json!({
            "detector": detector,
            "value": value,
            "unknown": if value.is_none() { json!("scene") } else { Value::Null },
            "track": value.filter(|value| *value != 0).map(|_| track),
            "ageNs": age_ms * 1_000_000,
        })
    };
    let receipts: Vec<Value> = (0..done)
        .map(|_| REFLEX_PRESSED_OUTCOME)
        .chain((0..missed).map(|_| REFLEX_MISSED_OUTCOMES[0]))
        .enumerate()
        .map(|(at, outcome)| json!({ "seq": at + 1, "outcome": outcome }))
        .collect();
    let mut snapshot = snapshot_of(&json!({
        "runId": "rx-1",
        "sightings": [sighting("red", first, total + 1), sighting("blue", second, total + 2)],
        "outcomes": { "done": total },
        "receipts": receipts,
        "scene": { "stream": 1, "geometry": 1, "owner": 7, "plan": plan },
        "lastCapture": total + 1,
        "lastCaptureAgeNs": age_ms * 1_000_000,
    }));
    snapshot.read_ms = Some(1_000);
    snapshot
}

/// The endpoint's body answering `next` with `own` and each branch named in
/// `branches` with its word.
fn answering_ahead(own: &str, branches: &[(Branch, &str)]) -> String {
    let choice = |word: &str| {
        let probabilities: Map<String, Value> = REFLEX_DECIDE_OPTIONS
            .iter()
            .map(|(option, _)| {
                (
                    (*option).to_string(),
                    json!(if *option == word { 0.8 } else { 0.1 }),
                )
            })
            .collect();
        json!({ "type": "choice", "choice": word, "probabilities": probabilities, "confidence": 0.7 })
    };
    let mut answers = Map::new();
    answers.insert(REFLEX_DECIDE_QUESTION.to_string(), choice(own));
    for (branch, word) in branches {
        answers.insert(branch.question(), choice(word));
    }
    json!({ "answers": answers, "model": "jev-1.13.0" }).to_string()
}

/// A healthy reading: the red detector reads a target, the blue none, the
/// hand finished three since the reading before.
fn healthy(plan: u64, total: u64) -> Snapshot {
    reading(plan, Some(1), Some(0), 4, (3, 0, total))
}

/// What a reading is, as the options are written: some detector reads a
/// target, some none, some unknown; the capture or a frame past the limit;
/// the hand finished or lost something since the reading before. A count, an
/// age under the limit, a track's number and why a reading is unknown are no
/// fact of it.
#[test]
fn a_readings_facts_are_what_the_options_are_written_in() {
    assert_eq!(
        facts_of(&healthy(1, 10).state),
        Facts {
            capture_over: Some(false),
            frame_over: false,
            target: true,
            none: true,
            unknown: false,
            done: true,
            missed: false,
        }
    );
    let blind = facts_of(&reading(1, None, None, 60, (0, 2, 10)).state);
    assert_eq!(
        blind,
        Facts {
            capture_over: Some(true),
            frame_over: true,
            target: false,
            none: false,
            unknown: true,
            done: false,
            missed: true,
        }
    );
    // Other counts, ages under the limit, other tracks: the same facts.
    assert_eq!(
        facts_of(&reading(1, Some(3), Some(0), 21, (8, 0, 77)).state),
        facts_of(&healthy(1, 10).state)
    );
    // Why a reading is unknown is not a fact of it.
    let mut occluded = reading(1, Some(1), None, 4, (3, 0, 10));
    occluded.state["sightings"][1]["unknown"] = json!("occluded");
    assert_eq!(
        facts_of(&occluded.state),
        facts_of(&reading(1, Some(1), None, 4, (3, 0, 10)).state)
    );
    // No sightings, no capture dated: nothing read, nothing past a limit.
    assert_eq!(facts_of(&snapshot_of(&json!({})).state).capture_over, None);
    assert!(finds_nothing(
        &reading(1, Some(0), None, 4, (0, 0, 1)).state
    ));
    assert!(!finds_nothing(&healthy(1, 1).state));
}

/// A reading is asked ahead of the branches it can take, each once, never of
/// itself and at most [`AHEAD_BRANCHES`]: a target can be taken only where
/// one is read, appear only where a detector reads none.
#[test]
fn a_reading_is_asked_ahead_of_the_branches_it_can_take() {
    let words = |snapshot: &Snapshot| -> Vec<&'static str> {
        branches_of(&snapshot.state)
            .into_iter()
            .map(|(branch, _)| branch.word())
            .collect()
    };
    assert_eq!(words(&healthy(1, 10)), ["appears", "taken", "missed"]);
    // Only unknowns: nothing to take, nowhere to appear.
    assert_eq!(words(&reading(1, None, None, 4, (2, 0, 9))), ["missed"]);
    // A hand already missing: that branch is the reading itself.
    assert_eq!(words(&reading(1, Some(1), None, 4, (0, 1, 9))), ["taken"]);
    let healthy_facts = facts_of(&healthy(1, 10).state);
    for (branch, facts) in branches_of(&healthy(1, 10).state) {
        assert_ne!(facts, healthy_facts, "{}", branch.word());
        assert_eq!(Some(facts), branch.edit(healthy_facts));
    }
    assert!(branches_of(&healthy(1, 10).state).len() <= AHEAD_BRANCHES);
}

/// The request carries the reading's own question as it always was, byte for
/// byte, and one question of the same choice per branch under its own name —
/// the state once, unchanged.
#[test]
fn the_request_asks_the_reading_as_it_is_and_each_branch_beside_it() {
    let state = healthy(1, 10).state;
    let asked = questions_for(&state);
    assert_eq!(
        asked[REFLEX_DECIDE_QUESTION],
        questions()[REFLEX_DECIDE_QUESTION]
    );
    let names: BTreeSet<String> = asked
        .as_object()
        .expect("questions")
        .keys()
        .cloned()
        .collect();
    let wanted: BTreeSet<String> = std::iter::once(REFLEX_DECIDE_QUESTION.to_string())
        .chain(Branch::ALL.map(Branch::question))
        .collect();
    assert_eq!(names, wanted);
    for (branch, supposes) in Branch::ALL
        .into_iter()
        .zip(REFLEX_DECIDE_BRANCHES.map(|(_, s)| s))
    {
        let question = &asked[branch.question()];
        assert_eq!(question["type"], json!("choice"));
        let words = question["instructions"].as_str().expect("instructions");
        assert!(words.contains(supposes), "{}", branch.word());
        assert_eq!(
            question["criteria"], asked[REFLEX_DECIDE_QUESTION]["criteria"],
            "the same three options"
        );
    }
    // A reading with no branch asks its own question alone.
    let lone = reading(1, None, None, 4, (0, 1, 9));
    assert_eq!(questions_for(&lone.state), questions());
}

/// One request's answers are held for the reading after it: its own, under
/// the reading's facts, and each branch's under the facts it supposes — and
/// the reading that comes takes the one whose premise is its own, at once,
/// letting go of the rest.
#[test]
fn the_next_reading_takes_the_answer_held_for_its_premise() {
    let pending = Pending {
        id: 4,
        snapshot: healthy(1, 10),
    };
    let mut ahead = Ahead::new();
    let body = answering_ahead(
        CONTINUE,
        &[
            (Branch::Appears, CONTINUE),
            (Branch::Taken, CONTINUE),
            (Branch::Missed, PAUSE),
        ],
    );
    assert_eq!(ahead.hold(&pending, &body), 0);
    assert_eq!(ahead.len(), 1 + AHEAD_BRANCHES);
    // The hand lost what it went for: the missed branch's pause.
    let next = reading(1, Some(1), Some(0), 9, (0, 1, 10));
    let taken = ahead.take(&next, 1_000 + REFLEX_COLLECT_MS);
    let held = taken.held.expect("held for this premise");
    assert_eq!(held.branch, Some(Branch::Missed));
    assert_eq!(held.chosen, PAUSE);
    assert_eq!(held.asked_with, 4);
    assert_eq!(held.model.as_deref(), Some("jev-1.13.0"));
    assert_eq!(taken.dropped, AHEAD_BRANCHES, "the rest let go");
    assert!(ahead.is_empty());
    // Its own answer is held for a next reading with the same facts.
    ahead.hold(&pending, &body);
    let same = ahead.take(&reading(1, Some(5), Some(0), 30, (7, 0, 13)), 1_500);
    assert_eq!(same.held.expect("the reading's own").branch, None);
    // A branch whose answer does not read is not held; the others are.
    let broken = answering_ahead(
        CONTINUE,
        &[(Branch::Appears, "retreat"), (Branch::Taken, CONTINUE)],
    );
    ahead.hold(&pending, &broken);
    assert_eq!(ahead.len(), 2, "its own and the one taken branch");
    assert_eq!(
        ahead.hold(&pending, "{"),
        2,
        "a body that does not read holds nothing"
    );
    assert!(ahead.is_empty());
}

/// No held answer is ever used on a reading whose premise is not the one it
/// was asked for (t-32797's proof): every part of the scene, the age of the
/// reading it was asked beside, every fact, an empty table — each leaves the
/// reading undecided, names why, and lets go of everything held.
#[test]
fn a_held_answer_is_never_used_on_another_premise() {
    let pending = Pending {
        id: 4,
        snapshot: healthy(1, 10),
    };
    let body = answering_ahead(
        CONTINUE,
        &[
            (Branch::Appears, PAUSE),
            (Branch::Taken, PAUSE),
            (Branch::Missed, PAUSE),
        ],
    );
    let soon = 1_000 + REFLEX_COLLECT_MS;
    let refused = |next: &Snapshot, now_ms: u64| {
        let mut ahead = Ahead::new();
        ahead.hold(&pending, &body);
        let taken = ahead.take(next, now_ms);
        assert!(ahead.is_empty(), "everything let go");
        assert_eq!(taken.dropped, 1 + AHEAD_BRANCHES);
        taken.held.expect_err("never used")
    };
    // Every part of the scene.
    for part in ["stream", "geometry", "owner", "plan"] {
        let mut next = healthy(1, 13);
        next.scene = next.scene.map(|mut scene| {
            match part {
                "stream" => scene.stream += 1,
                "geometry" => scene.geometry += 1,
                "owner" => scene.owner += 1,
                _ => scene.plan += 1,
            }
            scene
        });
        assert_eq!(refused(&next, soon), Unheld::Scene, "{part}");
    }
    let mut another_run = healthy(1, 13);
    if let Some(scene) = another_run.scene.as_mut() {
        scene.run = "rx-2".into();
    }
    assert_eq!(refused(&another_run, soon), Unheld::Scene, "run");
    let mut unnamed = healthy(1, 13);
    unnamed.scene = None;
    assert_eq!(
        refused(&unnamed, soon),
        Unheld::Scene,
        "a scene nobody names"
    );
    // The reading it was asked beside, too old — or undatable.
    assert_eq!(
        refused(&healthy(1, 13), 1_000 + REFLEX_APPLY_MAX_AGE_MS),
        Unheld::Stale
    );
    let mut undated = Ahead::new();
    let mut unread = pending.clone();
    unread.snapshot.read_ms = None;
    undated.hold(&unread, &body);
    assert_eq!(undated.take(&healthy(1, 13), soon).held, Err(Unheld::Stale));
    // Every fact: a reading whose facts no held answer names.
    for (fact, next) in [
        ("capture_over", reading(1, Some(1), Some(0), 60, (3, 0, 13))),
        ("unknown", reading(1, Some(1), None, 4, (3, 0, 13))),
        ("target and none", reading(1, None, None, 4, (3, 0, 13))),
        ("done", reading(1, Some(1), Some(0), 4, (0, 0, 13))),
        (
            "missed beside done",
            reading(1, Some(1), Some(0), 4, (3, 1, 13)),
        ),
    ] {
        assert_eq!(refused(&next, soon), Unheld::Facts, "{fact}");
    }
    let mut late_frame = healthy(1, 13);
    late_frame.state["sightings"][0]["age_ms"] = json!(LIMITS.max_frame_age_ns / 1_000_000 + 1);
    assert_eq!(refused(&late_frame, soon), Unheld::Facts, "frame_over");
    // Nothing held.
    let taken = Ahead::new().take(&healthy(1, 13), soon);
    assert_eq!((taken.held, taken.dropped), (Err(Unheld::Nothing), 0));
}

/// A decision a held answer decided is a row of the seat's words that cost
/// no request, on the reading it decided — its branch, the decision whose
/// request carried it and the premise it was used on named — and it is
/// numbered among the run's decisions without being asked.
#[test]
fn a_held_answers_row_is_an_answer_of_no_request() {
    let mut decider = Decider::new();
    let Offer::Ask(asked) = decider.offer(healthy(1, 10)) else {
        panic!("asked");
    };
    assert_eq!(decider.newest(), asked.id);
    let mut ahead = Ahead::new();
    ahead.hold(
        &asked,
        &answering_ahead(CONTINUE, &[(Branch::Missed, PAUSE)]),
    );
    assert_eq!(decider.settled(asked.id), None, "its answer came back");
    let next = reading(1, Some(1), Some(0), 9, (0, 1, 10));
    let Offer::Ask(own) = decider.offer(next.clone()) else {
        panic!("the reading's own question is asked as ever");
    };
    let held = ahead
        .take(&next, 1_000 + REFLEX_COLLECT_MS)
        .held
        .expect("held");
    let decided = decider.decided(next.clone());
    assert_eq!((asked.id, own.id, decided.id), (1, 2, 3));
    assert_eq!(decider.newest(), own.id, "a held decision is no reading");
    assert!(decider.asking(), "and changes nothing in flight");
    let row = held_row("rx-1", &decided, &held);
    assert_eq!(row["road"], json!(ROAD_AHEAD));
    assert_eq!(row["outcome"], json!(ANSWERED));
    assert_eq!(row["chosen"], json!(PAUSE));
    assert_eq!(row["decision"], json!(3));
    assert_eq!(row["state"], next.state, "the reading it decided");
    assert_eq!(row["provenance"], next.provenance());
    assert_eq!(row["branch"], json!("missed"));
    assert_eq!(row["askedWith"], json!(1));
    assert_eq!(row["premise"], json!(premise_of(&next)));
    assert_eq!(row["cached"], json!(true));
    assert_eq!(row["requests"], json!(0));
    assert_eq!(row["attempts"], json!(0));
    assert_eq!(row["model"], json!("jev-1.13.0"));
    assert_eq!(row["rubricVersion"], json!(REFLEX_DECIDE_RUBRIC_VERSION));
    assert_eq!(row["applied"], json!(false));
    // The question asked about it names the branches that rode with it.
    let own_row = asked_row(
        "rx-1",
        &asked,
        &sent(answering("continue")),
        None,
        true,
        false,
    );
    assert_eq!(own_row["branches"], json!(["appears", "taken", "missed"]));
    // Two readings of other facts are other premises.
    assert_ne!(premise_of(&next), premise_of(&healthy(1, 10)));
    assert_eq!(premise_of(&healthy(1, 10)), premise_of(&healthy(1, 99)));
}
