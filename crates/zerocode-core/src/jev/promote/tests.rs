use super::*;
use crate::jev::choice::ChoiceRefusal;
use crate::jev::summary::Tally;

/// The rows a floor of 950 per thousand can be cleared on — what the clean
/// window is sized to, and what a thin one is one short of.
fn wanted() -> usize {
    rows_that_can_clear(950)
}

fn window(rows: usize, answered: usize, p95_ms: Option<u64>) -> Tally {
    Tally {
        rows,
        answered,
        called: rows,
        p95_ms,
        ..Tally::default()
    }
}

fn clean() -> Evidence<'static> {
    // A window that clears every line, so each test can break exactly one.
    static WINDOW: std::sync::OnceLock<Tally> = std::sync::OnceLock::new();
    Evidence {
        window: WINDOW.get_or_init(|| window(200, 200, Some(600))),
        floor_permille: 950,
        deadline_ms: 1_500,
        agreement_floor_permille: 800,
        agreement: Agreement {
            compared: 60,
            agreed: 57,
        },
        labels: Some(Labels {
            compared: 40,
            judgment_right: 34,
            probe_right: 31,
        }),
        fallbacks_in_a_row: 0,
    }
}

#[test]
fn every_schema_token_this_product_writes_is_recognised_as_one() {
    // zo writes the word alone; the choice reader writes it with the rule
    // that broke after it. A judge that knew only one of those shapes would
    // promote a seat whose replies were arriving malformed.
    assert!(names_a_schema_failure(SCHEMA));
    for refusal in [
        ChoiceRefusal::NoAnswer,
        ChoiceRefusal::NotAChoice,
        ChoiceRefusal::UnknownOption,
        ChoiceRefusal::Keys,
        ChoiceRefusal::NotOne,
        ChoiceRefusal::OutOfRange,
    ] {
        assert!(
            names_a_schema_failure(refusal.token()),
            "{}",
            refusal.token()
        );
    }
    for other in [
        "timeout",
        "no_key",
        "not_consented",
        "budget",
        "schemata",
        "schemaless",
    ] {
        assert!(
            !names_a_schema_failure(other),
            "{other} is not a schema failure"
        );
    }
}

#[test]
fn a_clean_window_rises_and_an_acting_seat_keeps() {
    assert_eq!(judge(Stand::Recording, &clean()), Verdict::Rise);
    assert_eq!(judge(Stand::Applying, &clean()), Verdict::Keep);
}

#[test]
fn a_thin_window_holds_but_never_takes_back_a_seat_that_already_earned_its_place() {
    let thin = window(wanted() - 1, wanted() - 1, Some(10));
    let evidence = Evidence {
        window: &thin,
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &evidence),
        Verdict::Hold(Line::TooFewRows {
            rows: wanted() - 1,
            wanted: wanted()
        })
    );
    assert_eq!(
        judge(Stand::Applying, &evidence),
        Verdict::Keep,
        "a fresh window is not evidence"
    );
}

#[test]
fn the_window_is_as_wide_as_its_floor_needs_so_a_perfect_one_can_clear_it() {
    // The share is read as a bound, and twenty of twenty bound at 0.839: a
    // window of the judgment's cadence could never clear a floor of 0.95, and
    // the routing seat was held to exactly that for three days. The window
    // is sized to the floor, so a perfect window of that size rises and one
    // row short of it is named short of rows, never short of answers.
    assert!(
        wanted() > JUDGED_EVERY_ROWS,
        "0.95 needs more than a cadence"
    );
    let full = window(wanted(), wanted(), Some(10));
    assert_eq!(
        judge(
            Stand::Recording,
            &Evidence {
                window: &full,
                ..clean()
            }
        ),
        Verdict::Rise
    );
    let one_miss = window(wanted(), wanted() - 1, Some(10));
    let Verdict::Hold(Line::Answered {
        bound_permille,
        floor_permille,
    }) = judge(
        Stand::Recording,
        &Evidence {
            window: &one_miss,
            ..clean()
        },
    )
    else {
        panic!("one miss in the smallest window that clears rose anyway");
    };
    assert_eq!(floor_permille, 950);
    assert!(
        bound_permille < 950,
        "bounded at {bound_permille} per thousand"
    );
}

#[test]
fn a_door_refusal_is_not_a_row_the_seat_was_asked() {
    // Three rows of a missing key beside 25 answers held the routing seat at
    // a bound of 0.728 (2026-09-20). The door's refusals sit in `rows` for the
    // screen and outside the population the line is read over.
    let mut refused = window(wanted() + 3, wanted(), Some(10));
    refused.refused = 3;
    refused.failures = vec![("no_key".to_string(), 3)];
    assert_eq!(
        judge(
            Stand::Recording,
            &Evidence {
                window: &refused,
                ..clean()
            }
        ),
        Verdict::Rise,
        "a key the person had not saved was held against the seat"
    );
}

#[test]
fn a_seat_falls_on_the_line_it_breaks_and_the_first_one_is_the_one_named() {
    let slow = window(200, 200, Some(4_847));
    let evidence = Evidence {
        window: &slow,
        ..clean()
    };
    assert_eq!(
        judge(Stand::Applying, &evidence),
        Verdict::Fall(Line::Latency {
            p95_ms: 4_847,
            deadline_ms: 1_500
        })
    );

    let mut malformed = window(200, 199, Some(600));
    malformed.failures = vec![(ChoiceRefusal::NotOne.token().to_string(), 1)];
    let evidence = Evidence {
        window: &malformed,
        ..clean()
    };
    assert_eq!(
        judge(Stand::Applying, &evidence),
        Verdict::Fall(Line::Schema { rows: 1 })
    );

    let mut both = window(200, 150, Some(9_000));
    both.failures = vec![(SCHEMA.to_string(), 50)];
    let evidence = Evidence {
        window: &both,
        ..clean()
    };
    assert!(
        matches!(
            judge(Stand::Applying, &evidence),
            Verdict::Fall(Line::Answered { .. })
        ),
        "the answered line is asked before the latency and the schema ones"
    );
}

#[test]
fn with_no_labels_a_seat_rises_on_its_route_change_budget_and_holds_under_it() {
    // Nobody labelled anything, and nobody has to: a clean window whose
    // judgment names what the probe names four times in five rises.
    let unlabelled = Evidence {
        labels: None,
        ..clean()
    };
    assert_eq!(judge(Stand::Recording, &unlabelled), Verdict::Rise);
    // Too few rows where both readers answered say nothing yet — the probe
    // times out on a third of them (11 of 28 on this machine) — and a seat
    // already acting is not taken back on a thin comparison.
    let thin = Evidence {
        labels: None,
        agreement: Agreement {
            compared: JUDGED_EVERY_ROWS - 1,
            agreed: JUDGED_EVERY_ROWS - 1,
        },
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &thin),
        Verdict::Hold(Line::TooFewCompared {
            compared: JUDGED_EVERY_ROWS - 1,
            wanted: JUDGED_EVERY_ROWS
        })
    );
    assert_eq!(judge(Stand::Applying, &thin), Verdict::Keep);
    // Half the axes: the seat this machine has (53% over 30 axes) — a
    // different router, not a faster probe. It holds, and an acting one falls.
    let half = Evidence {
        labels: None,
        agreement: Agreement {
            compared: 30,
            agreed: 16,
        },
        ..clean()
    };
    let Verdict::Hold(Line::Agreement {
        bound_permille,
        floor_permille: 800,
    }) = judge(Stand::Recording, &half)
    else {
        panic!("half agreement rose");
    };
    assert!(bound_permille < 800, "bounded at {bound_permille}");
    assert!(matches!(
        judge(Stand::Applying, &half),
        Verdict::Fall(Line::Agreement { .. })
    ));
    // Twenty of twenty bound at 0.839: the budget is one a single judgment
    // window can clear, unlike the answered floor.
    let twenty = Evidence {
        labels: None,
        agreement: Agreement {
            compared: JUDGED_EVERY_ROWS,
            agreed: JUDGED_EVERY_ROWS,
        },
        ..clean()
    };
    assert_eq!(judge(Stand::Recording, &twenty), Verdict::Rise);
}

#[test]
fn a_persons_labels_outrank_agreement_and_thin_ones_fall_through_to_it() {
    // Labels are the one comparison that says a reader is right: a window's
    // worth of them decide, whatever the agreement says, and fewer than that
    // leave the agreement to decide.
    let disagreeing_but_right = Evidence {
        agreement: Agreement {
            compared: 30,
            agreed: 16,
        },
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &disagreeing_but_right),
        Verdict::Rise,
        "labels said it is righter than the probe it disagrees with"
    );
    let thin_labels = Evidence {
        labels: Some(Labels {
            compared: JUDGED_EVERY_ROWS - 1,
            judgment_right: 19,
            probe_right: 0,
        }),
        agreement: Agreement {
            compared: 30,
            agreed: 16,
        },
        ..clean()
    };
    assert!(
        matches!(
            judge(Stand::Recording, &thin_labels),
            Verdict::Hold(Line::Agreement { .. })
        ),
        "nineteen labels are not yet a word, and the agreement held"
    );
    let worse = Evidence {
        labels: Some(Labels {
            compared: 40,
            judgment_right: 30,
            probe_right: 31,
        }),
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &worse),
        Verdict::Hold(Line::Labels {
            judgment_right: 30,
            probe_right: 31
        })
    );
    let tied = Evidence {
        labels: Some(Labels {
            compared: 40,
            judgment_right: 31,
            probe_right: 31,
        }),
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &tied),
        Verdict::Rise,
        "as right is right enough"
    );
}

#[test]
fn a_seat_that_never_had_labels_keeps_acting_but_a_worse_one_falls() {
    // Labels going missing is not the seat getting worse: it is judged on its
    // agreement instead. Labels saying the probe is righter is.
    assert_eq!(
        judge(
            Stand::Applying,
            &Evidence {
                labels: None,
                ..clean()
            }
        ),
        Verdict::Keep
    );
    let worse = Evidence {
        labels: Some(Labels {
            compared: 40,
            judgment_right: 10,
            probe_right: 31,
        }),
        ..clean()
    };
    assert_eq!(
        judge(Stand::Applying, &worse),
        Verdict::Fall(Line::Labels {
            judgment_right: 10,
            probe_right: 31
        })
    );
}

#[test]
fn three_fallbacks_in_a_row_end_it_now_and_two_do_not() {
    let two = Evidence {
        fallbacks_in_a_row: FALLBACKS_THAT_END_IT - 1,
        ..clean()
    };
    assert_eq!(judge(Stand::Applying, &two), Verdict::Keep);
    let three = Evidence {
        fallbacks_in_a_row: FALLBACKS_THAT_END_IT,
        ..clean()
    };
    assert_eq!(
        judge(Stand::Applying, &three),
        Verdict::Fall(Line::Fallbacks {
            in_a_row: FALLBACKS_THAT_END_IT
        })
    );
    assert_eq!(
        judge(Stand::Recording, &three),
        Verdict::Rise,
        "a seat that is not acting has not fallen back"
    );
}

#[test]
fn a_bound_is_floored_so_it_never_reads_as_clearing_a_line_it_sits_under() {
    assert_eq!(permille(0.9499), 949);
    assert_eq!(permille(0.95), 950);
    assert_eq!(permille(1.0), 1000);
    assert_eq!(permille(-1.0), 0);
}

#[test]
fn a_seat_stands_where_its_last_transition_left_it() {
    use serde_json::json;
    assert_eq!(
        stand_from(&[]),
        Stand::Recording,
        "a seat that never rose is recording"
    );
    let rows = [
        json!({"at": 1, "outcome": "answered"}),
        json!({"at": 2, (TRANSITION.canonical): ROSE}),
        json!({"at": 3, "outcome": "answered"}),
    ];
    assert_eq!(stand_from(&rows), Stand::Applying);
    let rows = [
        json!({"at": 2, (TRANSITION.canonical): ROSE}),
        json!({"at": 4, (TRANSITION.canonical): FELL, ON_LINE: "latency"}),
        json!({"at": 5, "outcome": "answered"}),
    ];
    assert_eq!(stand_from(&rows), Stand::Recording, "the last one decides");
}

#[test]
fn only_a_change_is_written_down() {
    let held = window(200, 200, Some(600));
    assert_eq!(transition_row(9, Verdict::Keep, &held), None);
    assert_eq!(
        transition_row(9, Verdict::Hold(Line::Schema { rows: 1 }), &held),
        None
    );

    let rose = transition_row(9, Verdict::Rise, &held).expect("a rise is written");
    assert_eq!(rose[TRANSITION.canonical], ROSE);
    assert_eq!(rose["rows"], 200);
    assert_eq!(
        rose[ON_LINE],
        serde_json::Value::Null,
        "a rise broke no line"
    );

    let fell = transition_row(
        9,
        Verdict::Fall(Line::Latency {
            p95_ms: 4_847,
            deadline_ms: 1_500,
        }),
        &held,
    )
    .expect("a fall is written");
    assert_eq!(fell[TRANSITION.canonical], FELL);
    assert_eq!(fell[ON_LINE], "latency");
}

#[test]
fn a_transition_row_carries_the_numbers_and_none_of_the_request() {
    let held = window(40, 38, Some(700));
    let row = transition_row(9, Verdict::Rise, &held).expect("row");
    // Sorted before it is compared: whether a JSON map keeps insertion order
    // is a feature the WORKSPACE turns on and not a promise this crate makes,
    // so the same row comes back one way here and another in the gate
    // (trap 316).
    let mut keys: Vec<&str> = row
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut wanted = vec![
        "answered",
        "answeredLowerBoundPermille",
        "at",
        "p95Ms",
        "rows",
        TRANSITION.canonical,
    ];
    wanted.sort_unstable();
    assert_eq!(keys, wanted, "a closed set, whatever order the build keeps");
}

#[test]
fn every_line_names_itself_with_a_word_that_carries_nothing_of_the_ask() {
    let lines = [
        Line::TooFewRows {
            rows: 1,
            wanted: 20,
        },
        Line::Answered {
            bound_permille: 1,
            floor_permille: 950,
        },
        Line::Latency {
            p95_ms: 1,
            deadline_ms: 2,
        },
        Line::Schema { rows: 1 },
        Line::TooFewCompared {
            compared: 1,
            wanted: 20,
        },
        Line::Agreement {
            bound_permille: 1,
            floor_permille: 800,
        },
        Line::Labels {
            judgment_right: 1,
            probe_right: 2,
        },
        Line::Fallbacks { in_a_row: 3 },
    ];
    let mut tokens: Vec<&str> = lines.iter().map(|line| line.token()).collect();
    let before = tokens.len();
    tokens.sort_unstable();
    tokens.dedup();
    assert_eq!(tokens.len(), before, "two lines share a word");
    for token in tokens {
        assert!(
            token.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{token} is not a plain word"
        );
    }
}

#[test]
fn a_verdict_and_its_row_call_a_rise_and_a_fall_the_same_thing() {
    assert_eq!(Verdict::Rise.token(), ROSE);
    assert_eq!(Verdict::Fall(Line::Schema { rows: 1 }).token(), FELL);
    assert_eq!(Verdict::Rise.line(), None);
    assert_eq!(Verdict::Keep.line(), None);
    assert_eq!(
        Verdict::Hold(Line::TooFewCompared {
            compared: 1,
            wanted: 20
        })
        .line(),
        Some(Line::TooFewCompared {
            compared: 1,
            wanted: 20
        })
    );
    let mut words: Vec<&str> = [
        Verdict::Hold(Line::Schema { rows: 1 }),
        Verdict::Rise,
        Verdict::Keep,
        Verdict::Fall(Line::Schema { rows: 1 }),
    ]
    .iter()
    .map(|verdict| verdict.token())
    .collect();
    words.sort_unstable();
    words.dedup();
    assert_eq!(words.len(), 4, "two verdicts share a word");
    assert_eq!(Stand::Recording.token(), "recording");
    assert_eq!(Stand::Applying.token(), "applying");
    // The reason a walk gives for pressing nothing names the stand it was
    // standing at, so the two cannot come to say different things about the
    // same seat.
    assert!(SEAT_RECORDING.ends_with(Stand::Recording.token()));
}

#[test]
fn the_labels_file_is_named_in_the_settings_or_nowhere() {
    use serde_json::json;
    assert_eq!(labels_path_in(&json!({})), None);
    assert_eq!(labels_path_in(&json!({"smart": {}})), None);
    assert_eq!(labels_path_in(&json!({"smart": {"jev": {}}})), None);
    assert_eq!(
        labels_path_in(&json!({"smart": {"jev": {"labels": "  "}}})),
        None,
        "blank is unnamed"
    );
    assert_eq!(
        labels_path_in(&json!({"smart": {"jev": {"labels": " /a/labels.jsonl "}}})),
        Some("/a/labels.jsonl"),
        "a path is read without the spaces around it"
    );
}

#[test]
fn label_drafts_are_off_until_a_person_says_otherwise() {
    use serde_json::json;
    // It writes a person's own prompts to a file the ledger beside it refuses
    // to write, so nothing but a plain `true` turns it on.
    assert!(!label_drafts_wanted(&json!({})));
    assert!(!label_drafts_wanted(&json!({"smart": {"jev": {}}})));
    assert!(!label_drafts_wanted(
        &json!({"smart": {"jev": {"labelDrafts": "yes"}}})
    ));
    assert!(!label_drafts_wanted(
        &json!({"smart": {"jev": {"labelDrafts": 1}}})
    ));
    assert!(!label_drafts_wanted(
        &json!({"smart": {"jev": {"labelDrafts": false}}})
    ));
    assert!(label_drafts_wanted(
        &json!({"smart": {"jev": {"labelDrafts": true}}})
    ));
}

/// The table-driven judge: an orchestration seat rises on its own rows —
/// enough of them, answered, in time, and marked `agreed` with what the
/// coordinator or the window's rule did — and is due every twenty requests.
#[test]
fn an_orchestration_seat_is_judged_by_the_table_on_its_own_agreed_marks() {
    use serde_json::json;
    let seat = &crate::jev::SUMMON;
    let floor = seat.answer_floor_permille.expect("summon rises");
    let wanted = rows_that_can_clear(floor);
    let row = |at: i64, agreed: bool| json!({"at": at, "outcome": "answered", "elapsedMs": 600, "requests": 1, "agreed": agreed});
    // Thin: held short of rows, and not yet due.
    let thin: Vec<serde_json::Value> = (0..3).map(|at| row(at, true)).collect();
    let judged = judge_seat(seat, &thin).expect("a promoting seat is judged");
    assert_eq!(judged.window_wanted, wanted);
    assert!(matches!(
        judged.verdict,
        Verdict::Hold(Line::TooFewRows { rows: 3, .. })
    ));
    assert!(!judgment_due(&thin));
    assert!(judgment_due(
        &(0..JUDGED_EVERY_ROWS as i64)
            .map(|at| row(at, true))
            .collect::<Vec<_>>()
    ));
    // Full and agreeing: rises.
    let full: Vec<serde_json::Value> = (0..wanted as i64).map(|at| row(at, true)).collect();
    let judged = judge_seat(seat, &full).expect("judged");
    assert_eq!(judged.verdict, Verdict::Rise, "{judged:?}");
    assert_eq!(judged.agreement.compared, wanted);
    // Full but disagreeing with the coordinator every other time (this
    // machine's summon seat: 0 of 13): holds on the agreement line.
    let half: Vec<serde_json::Value> = (0..wanted as i64).map(|at| row(at, at % 2 == 0)).collect();
    assert!(matches!(
        judge_seat(seat, &half).expect("judged").verdict,
        Verdict::Hold(Line::Agreement { .. })
    ));
    // A label row's mark counts too, and only from the window's first row on.
    let mut labelled: Vec<serde_json::Value> = (0..wanted as i64)
        .map(|at| json!({"at": at, "outcome": "answered", "elapsedMs": 600, "requests": 1}))
        .collect();
    labelled.extend(
        (0..wanted as i64).map(|at| json!({"at": at, "label": format!("k{at}"), "agreed": true})),
    );
    labelled.push(json!({"at": -5, "label": "old", "agreed": false}));
    let judged = judge_seat(seat, &labelled).expect("judged");
    assert_eq!(
        judged.agreement,
        Agreement {
            compared: wanted,
            agreed: wanted
        },
        "the old mark is outside the window"
    );
    assert_eq!(judged.verdict, Verdict::Rise);
    // Every seat in the table rises now (t-5806): recall is judged on the
    // same marks, on its own lines.
    assert!(judge_seat(&crate::jev::RECALL, &full).is_some());
}
