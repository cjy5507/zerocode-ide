use super::*;
use crate::jev::choice::ChoiceRefusal;
use crate::jev::summary::Tally;

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
    let thin = window(JUDGED_EVERY_ROWS - 1, JUDGED_EVERY_ROWS - 1, Some(10));
    let evidence = Evidence {
        window: &thin,
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &evidence),
        Verdict::Hold(Line::TooFewRows {
            rows: JUDGED_EVERY_ROWS - 1,
            wanted: JUDGED_EVERY_ROWS
        })
    );
    assert_eq!(
        judge(Stand::Applying, &evidence),
        Verdict::Keep,
        "a fresh window is not evidence"
    );
}

#[test]
fn the_share_is_read_as_a_bound_so_a_perfect_thin_window_does_not_carry_a_seat() {
    let twenty = window(JUDGED_EVERY_ROWS, JUDGED_EVERY_ROWS, Some(10));
    let evidence = Evidence {
        window: &twenty,
        ..clean()
    };
    let Verdict::Hold(Line::Answered {
        bound_permille,
        floor_permille,
    }) = judge(Stand::Recording, &evidence)
    else {
        panic!("twenty of twenty rose on a share of one");
    };
    assert_eq!(floor_permille, 950);
    assert!(
        bound_permille < 850,
        "bounded at {bound_permille} per thousand"
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
fn agreement_alone_never_carries_a_seat_up() {
    // §4: matching the probe is not evidence of being right, so a seat with
    // no labels holds however clean the rest of its window is.
    let evidence = Evidence {
        labels: None,
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &evidence),
        Verdict::Hold(Line::NoLabels {
            wanted: JUDGED_EVERY_ROWS
        })
    );
    let thin = Evidence {
        labels: Some(Labels {
            compared: JUDGED_EVERY_ROWS - 1,
            judgment_right: 19,
            probe_right: 0,
        }),
        ..clean()
    };
    assert_eq!(
        judge(Stand::Recording, &thin),
        Verdict::Hold(Line::NoLabels {
            wanted: JUDGED_EVERY_ROWS
        })
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
    // Labels going missing is not the seat getting worse; labels saying the
    // probe is righter is.
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
    let keys: Vec<&str> = row
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "answered",
            "answeredLowerBoundPermille",
            "at",
            "p95Ms",
            "rows",
            TRANSITION.canonical
        ],
        "a closed set, in serde's order"
    );
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
        Line::NoLabels { wanted: 20 },
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
        Verdict::Hold(Line::NoLabels { wanted: 20 }).line(),
        Some(Line::NoLabels { wanted: 20 })
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
