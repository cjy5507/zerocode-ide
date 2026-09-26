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
