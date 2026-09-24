use super::*;
use crate::jev::choice::ChoiceRefusal;
use crate::jev::summary::Tally;

/// The rows a floor of 950 per thousand can be cleared on — what the clean
/// window is sized to, and what a thin one is one short of.
fn wanted() -> usize {
    crate::jev::summary::rows_that_can_clear(950)
}

#[test]
fn a_no_reader_seat_keeps_its_standing_when_its_first_positive_label_arrives() {
    for seat in [&crate::jev::RECALL, &crate::jev::PLACEMENT] {
        let wanted = window_wanted_for(seat).expect("a promoting seat");
        let mut rows: Vec<Value> = (0..wanted)
            .map(|at| json!({"at": at, "outcome": "answered", "elapsedMs": 1}))
            .collect();
        rows.push(json!({"transition": ROSE}));
        rows.push(json!({"at": wanted, "label": "request", "agreed": true}));
        assert_eq!(
            judge_seat(seat, &rows).expect("judged").verdict,
            Verdict::Keep,
            "{}",
            seat.id
        );
    }
}

#[test]
fn hindsight_waits_for_the_sample_floor_then_uses_the_same_wilson_line() {
    for seat in [&crate::jev::RECALL, &crate::jev::PLACEMENT] {
        let wanted = window_wanted_for(seat).expect("a promoting seat");
        let floor = seat.agreement_rows_wanted.expect("a label sample floor");
        for (compared, agreed, falls) in [
            (floor - 1, false, false),
            (floor, true, false),
            (floor, false, true),
        ] {
            let mut rows: Vec<Value> = (0..wanted)
                .map(|at| json!({"at": at, "outcome": "answered", "elapsedMs": 1}))
                .collect();
            rows.push(json!({"transition": ROSE}));
            rows.extend(
                (0..compared).map(|at| json!({"at": wanted + at, "label": at, "agreed": agreed})),
            );
            let verdict = judge_seat(seat, &rows).expect("judged").verdict;
            assert_eq!(
                matches!(verdict, Verdict::Fall(Line::Agreement { .. })),
                falls,
                "{} {compared} {agreed} {verdict:?}",
                seat.id
            );
        }
    }
}

/// No seat rises with no marks to hand, whatever kind its marks are (t-6155
/// F1): a window of rows answered in time and in shape, and not one
/// `agreed`, holds every promoting seat at `too_few_compared` — the
/// compaction seat, whose hindsight kind once let it through to Applying
/// on twenty answered rows and no mark, with the rest. The same rows with
/// the floor's worth of agreeing marks rise.
#[test]
fn a_seat_with_no_marks_holds_at_the_sample_floor_whatever_its_kind() {
    for seat in crate::jev::JEV_USES.iter().filter(|row| row.promotes) {
        let wanted = window_wanted_for(seat).expect("a promoting seat");
        let floor = seat.agreement_rows_wanted.expect("a label sample floor");
        let rows: Vec<Value> = (0..wanted)
            .map(|at| json!({"at": at, "outcome": "answered", "elapsedMs": 1}))
            .collect();
        assert_eq!(
            judge_seat(seat, &rows).expect("judged").verdict,
            Verdict::Hold(Line::TooFewCompared {
                compared: 0,
                wanted: floor
            }),
            "{}: answer rate, latency and shape alone never make a seat act",
            seat.id
        );
        let mut marked = rows.clone();
        marked.extend(marks_that_rise(seat, wanted));
        assert_eq!(
            judge_seat(seat, &marked).expect("judged").verdict,
            Verdict::Rise,
            "{}",
            seat.id
        );
    }
}

/// Thirty marks that agree nine times in ten, the three that disagree the
/// oldest, from `at` on.
fn thirty_marks(at: usize) -> Vec<Value> {
    const MARKS: usize = 30;
    const MISSES: usize = 3;
    (0..MARKS)
        .map(|n| json!({"at": at + n, "label": n, "agreed": n >= MISSES}))
        .collect()
}

/// A full window of answered rows, then `marks` — the ledger a seat keeps
/// when the vendor's alias moves under it (t-6187). `on_b` of the marks, the
/// newest, name version `B`; the rest name `A`; `None` writes no version at
/// all, the shape of every row before versions were recorded.
fn marks_across_a_version_change(
    seat: &JevUse,
    marks: impl FnOnce(usize) -> Vec<Value>,
    on_b: Option<usize>,
) -> Vec<Value> {
    let wanted = window_wanted_for(seat).expect("a promoting seat");
    let mut rows = window_then(seat, marks);
    if let Some(on_b) = on_b {
        let count = rows.len() - wanted;
        for (n, mark) in rows[wanted..].iter_mut().enumerate() {
            mark[crate::jev::summary::MODEL.canonical] =
                json!(if n + on_b >= count { "B" } else { "A" });
        }
    }
    rows
}

/// A version change cuts the marks a seat is judged on (t-6187): the
/// newest five of thirty marks on `B` are five comparisons, not thirty, and
/// the seat holds at the sample floor naming the version it cut away; marks
/// on `B` that clear every line on their own — with the three disagreements
/// the label's record must hold (t-6342) — are a sample of their own, and it
/// rises. The same thirty marks with no version recorded are one sample of
/// 27 in 30, which bounds under the agreement line.
#[test]
fn a_seat_is_judged_on_the_marks_of_the_version_that_answers_now() {
    for seat in [&crate::jev::PLACEMENT, &crate::jev::SUMMON] {
        let floor = seat.agreement_rows_wanted.expect("a label sample floor");
        let judged = judge_seat(
            seat,
            &marks_across_a_version_change(seat, thirty_marks, Some(5)),
        )
        .expect("judged");
        assert_eq!(
            judged.verdict,
            Verdict::Hold(Line::TooFewCompared {
                compared: 5,
                wanted: floor
            }),
            "{}: version A's marks judged version B",
            seat.id
        );
        assert_eq!(
            (judged.model.as_deref(), judged.cut.as_deref()),
            (Some("B"), Some("A")),
            "{}: the verdict names the version it was read on and the one it cut",
            seat.id
        );
        assert_eq!(
            judged.window.rows,
            window_wanted_for(seat).expect("a promoting seat"),
            "{}: requests that name no version are not cut",
            seat.id
        );
        let on_b = marks_that_rise(seat, 0).len();
        let risen = marks_across_a_version_change(
            seat,
            |at| {
                let mut marks = thirty_marks(at);
                marks.extend(marks_that_rise(seat, at + marks.len()));
                marks
            },
            Some(on_b),
        );
        assert_eq!(
            judge_seat(seat, &risen).expect("judged").verdict,
            Verdict::Rise,
            "{}: version B's own marks, clearing every line",
            seat.id
        );
        let whole = judge_seat(
            seat,
            &marks_across_a_version_change(seat, thirty_marks, None),
        )
        .expect("judged");
        assert!(
            matches!(whole.verdict, Verdict::Hold(Line::Agreement { .. })),
            "{}: with no versions recorded the thirty are one sample",
            seat.id
        );
        assert_eq!((whole.model, whole.cut), (None, None));
    }
}

/// The cut is where the version changed, counted back from the newest row,
/// and nowhere else (t-6187): rows that name no version belong to the
/// nearest named row after them, a version that comes back after another is
/// a new run of rows, and a request another version answered takes the
/// window's requests with it while the marks written after it stay.
#[test]
fn the_rows_of_the_newest_version_start_after_the_last_row_another_answered() {
    let answered = |at: i64, model: Option<&str>| {
        let mut row = json!({"at": at, "outcome": "answered", "elapsedMs": 1});
        if let Some(model) = model {
            row[crate::jev::summary::MODEL.canonical] = json!(model);
        }
        row
    };
    let rows = [
        answered(1, Some("B")),
        answered(2, None),
        answered(3, Some("A")),
        json!({"at": 4, "outcome": "timeout"}),
        answered(5, Some("B")),
        json!({"at": 6, "label": "x", "agreed": true}),
        answered(7, None),
        json!({"at": 8, "transition": ROSE}),
    ];
    let version = on_the_newest_version(&rows);
    assert_eq!((version.model, version.cut), (Some("B"), Some("A")));
    assert_eq!(
        version.requests,
        &rows[3..],
        "after the newest request A answered"
    );
    assert_eq!(version.marks, &rows[3..]);
    assert_eq!(
        asked_toward_judgment(&rows),
        3,
        "the timeout, B and the unnamed answer"
    );

    let unnamed = [
        answered(1, None),
        json!({"at": 2, "label": "x", "agreed": false}),
    ];
    let version = on_the_newest_version(&unnamed);
    assert_eq!((version.model, version.cut), (None, None));
    assert_eq!(
        version.requests,
        &unnamed[..],
        "a ledger with no versions is read whole"
    );

    // A label that names the version it graded is cut with that version,
    // though written after the other version's last request — and it does
    // not move which version is answering now: a request says that.
    let late = [
        answered(1, Some("A")),
        answered(2, Some("B")),
        json!({"at": 3, "label": "a", "agreed": false, "model": "A"}),
        json!({"at": 4, "label": "b", "agreed": true}),
    ];
    let version = on_the_newest_version(&late);
    assert_eq!((version.model, version.cut), (Some("B"), Some("A")));
    assert_eq!(version.requests, &late[1..]);
    assert_eq!(
        version.marks,
        &late[3..],
        "the late label of A's answer is A's"
    );
}

/// A row that is neither a request nor a mark names no version, whatever it
/// spells under `model` (t-6284). zo's step governor files one `step` row
/// per request of a turn between its seat's judgments and labels, and until
/// then named on it the model the step ran on: read as versions, every step
/// cut the marks away, and this machine's ledger (56 judgments, 213 labels,
/// 316 steps) summed to a window cut at `claude-fable-5-1` with nothing
/// compared. The same shape here: each judgment Jev answered, a step on one
/// chat model, the label, a step on the next — and the window is the whole
/// ledger, the marks every label, and the seat rises on them.
#[test]
fn a_row_that_is_neither_a_request_nor_a_mark_cuts_no_window() {
    let seat = &crate::jev::ZO_STEP_EFFORT;
    let wanted = window_wanted_for(seat).expect("the step seat rises");
    let chat = ["claude-fable-5-1", "claude-opus-5", "gpt-6-sol"];
    let model = crate::jev::summary::MODEL.canonical;
    let mut rows: Vec<Value> = Vec::new();
    for n in 0..wanted {
        let at = n * 4;
        rows.push(json!({"kind": "judgment", "at": at, "outcome": "answered", "elapsedMs": 1, model: "jev-1.13.0"}));
        rows.push(json!({"kind": "step", "at": at + 1, model: chat[n % chat.len()]}));
        rows.push(json!({"kind": "label", "at": at + 2, "agreed": n >= crate::jev::NEGATIVES_WANTED, "baselineAgreed": n % 2 == 0}));
        rows.push(json!({"kind": "step", "at": at + 3, model: chat[(n + 1) % chat.len()]}));
    }
    let version = on_the_newest_version(&rows);
    assert_eq!(
        (version.model, version.cut),
        (Some("jev-1.13.0"), None),
        "a step's chat model is not the version that answered"
    );
    assert_eq!(version.requests, &rows[..]);
    assert_eq!(version.marks, &rows[..], "no step cuts the labels away");
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.cut.as_deref()),
        (wanted, None)
    );
    assert_eq!(judged.verdict, Verdict::Rise, "{judged:?}");

    // A ledger whose requests all went unanswered names no version at all —
    // not the chat model its steps ran on.
    let unanswered = [
        json!({"kind": "judgment", "at": 1, "outcome": "no_key"}),
        json!({"kind": "step", "at": 2, model: "claude-opus-5"}),
        json!({"kind": "label", "at": 3, "agreed": false}),
    ];
    let version = on_the_newest_version(&unanswered);
    assert_eq!((version.model, version.cut), (None, None));
    assert_eq!(version.marks, &unanswered[..]);
}

/// A seat already acting is judged on the new version's rows and keeps
/// acting while they are too few to say anything — the standing is read from
/// the whole ledger — and the judgment's cadence starts again with the
/// version, so the screen's countdown and the judgment still land on one row.
#[test]
fn a_change_of_version_restarts_the_window_and_leaves_the_standing() {
    let seat = &crate::jev::SUMMON;
    let wanted = window_wanted_for(seat).expect("summon rises");
    let row = |at: usize, model: &str| json!({"at": at, "outcome": "answered", "elapsedMs": 1, "agreed": true, "model": model});
    let mut rows: Vec<Value> = (0..wanted).map(|at| row(at, "A")).collect();
    rows.push(json!({"at": wanted, (TRANSITION.canonical): ROSE}));
    rows.extend((0..3).map(|n| row(wanted + 1 + n, "B")));
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(judged.verdict, Verdict::Keep, "{judged:?}");
    assert_eq!(judged.window.rows, 3);
    assert_eq!(asked_toward_judgment(&rows), 3);
    assert_eq!(
        rows_to_next_judgment(seat, asked_toward_judgment(&rows)),
        Some(wanted - 3)
    );
    assert!(
        !judgment_due(seat, &rows),
        "a judgment of three rows has one thing to say"
    );
}

#[test]
fn one_forgiven_timeout_does_not_forgive_a_second_one() {
    for seat in [&crate::jev::RECALL, &crate::jev::PLACEMENT] {
        let wanted = window_wanted_for(seat).expect("a promoting seat");
        for misses in [1, 2] {
            // A window with marks that clear every agreement line, so the
            // one line left to clear is the answer line.
            let mut rows: Vec<Value> = (0..wanted)
                .map(|at| json!({"at": at, "outcome": if at < misses {"timeout"} else {"answered"}, "elapsedMs": 1}))
                .collect();
            rows.extend(marks_that_rise(seat, wanted));
            let verdict = judge_seat(seat, &rows).expect("judged").verdict;
            assert_eq!(
                verdict == Verdict::Rise,
                misses == 1,
                "{} {verdict:?}",
                seat.id
            );
        }
    }
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
            ..Agreement::default()
        },
        agreement_rows_wanted: A_WINDOW_OF_COMPARISONS,
        window_forgives: 0,
        labels: Some(Labels {
            compared: 40,
            judgment_right: 34,
            probe_right: 31,
        }),
        fallbacks_in_a_row: 0,
        negatives_wanted: crate::jev::NEGATIVES_WANTED,
        disagreed_on_record: crate::jev::NEGATIVES_WANTED,
        baseline: Baseline::None,
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
            ..Agreement::default()
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
            ..Agreement::default()
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
            ..Agreement::default()
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
            ..Agreement::default()
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
            ..Agreement::default()
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

/// The standing read off a ledger's text is the one read off its parsed rows
/// — only the lines that carry a transition's key are parsed, so a row that
/// merely names the word in a value and a torn last line are passed over
/// without changing what the last transition says.
#[test]
fn a_ledgers_text_stands_where_its_rows_do() {
    use serde_json::json;
    let rows = [
        json!({"at": 1, "outcome": "answered", "note": "a \"transition\" named in a value"}),
        json!({"at": 2, (TRANSITION.canonical): ROSE}),
        json!({"at": 3, "outcome": "answered"}),
    ];
    let text = |rows: &[Value]| {
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
    };
    assert_eq!(stand_in(""), Stand::Recording, "an empty ledger never rose");
    assert_eq!(stand_in(&text(&rows)), stand_from(&rows));
    assert_eq!(stand_in(&text(&rows)), Stand::Applying);
    let torn = format!("{}{{\"at\": 9, \"transition\": \"fa", text(&rows));
    assert_eq!(
        stand_in(&torn),
        Stand::Applying,
        "a torn last line is passed over"
    );
    let fell = [
        rows[1].clone(),
        json!({"at": 4, (TRANSITION.canonical): FELL, ON_LINE: "latency"}),
        rows[2].clone(),
    ];
    assert_eq!(
        stand_in(&text(&fell)),
        Stand::Recording,
        "the last one decides"
    );
    assert_eq!(
        stand_in(&text(&rows[..1])),
        Stand::Recording,
        "a word in a value is not a transition"
    );
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
    let wanted = window_wanted_for(seat).expect("summon rises");
    assert_eq!(
        wanted,
        crate::jev::summary::rows_that_can_clear_forgiving(
            floor,
            seat.window_forgives.expect("summon forgives a bad minute")
        )
    );
    let row = |at: i64, agreed: bool| json!({"at": at, "outcome": "answered", "elapsedMs": 600, "requests": 1, "agreed": agreed});
    // Thin: held short of rows, and not yet due.
    let thin: Vec<serde_json::Value> = (0..3).map(|at| row(at, true)).collect();
    let judged = judge_seat(seat, &thin).expect("a promoting seat is judged");
    assert_eq!(judged.window_wanted, wanted);
    assert!(matches!(
        judged.verdict,
        Verdict::Hold(Line::TooFewRows { rows: 3, .. })
    ));
    assert!(!judgment_due(seat, &thin));
    // Counted from the window, not from the first row: a judgment at row
    // twenty of a fifty-three-row window could only ever say `too_few_rows`.
    assert!(!judgment_due(
        seat,
        &(0..JUDGED_EVERY_ROWS as i64)
            .map(|at| row(at, true))
            .collect::<Vec<_>>()
    ));
    assert!(judgment_due(
        seat,
        &(0..wanted as i64)
            .map(|at| row(at, true))
            .collect::<Vec<_>>()
    ));
    assert!(judgment_due(
        seat,
        &(0..(wanted + JUDGED_EVERY_ROWS) as i64)
            .map(|at| row(at, true))
            .collect::<Vec<_>>()
    ));
    // Full, agreeing but for the three the label said no to, and beating the
    // seat's baseline beside it: rises (t-6342).
    let full: Vec<serde_json::Value> = (0..wanted as i64)
        .map(|at| {
            let mut marked = row(at, at >= crate::jev::NEGATIVES_WANTED as i64);
            marked["baselineAgreed"] = json!(at % 2 == 0);
            marked
        })
        .collect();
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
    labelled.extend((0..wanted as i64).map(|at| {
        json!({"at": at, "label": format!("k{at}"), "agreed": true, "baselineAgreed": at % 2 == 0})
    }));
    // Three old marks that said no: outside the window, and on the record the
    // label is known to be able to say no by (t-6342).
    labelled.extend(
        (1..=3).map(|n: i64| json!({"at": -n, "label": format!("old{n}"), "agreed": false})),
    );
    let judged = judge_seat(seat, &labelled).expect("judged");
    assert_eq!(
        judged.agreement,
        Agreement {
            compared: wanted,
            agreed: wanted,
            baseline_compared: wanted,
            baseline_agreed: wanted.div_ceil(2),
            not_compared: 0,
        },
        "the old marks are outside the window"
    );
    assert_eq!(judged.verdict, Verdict::Rise);
    // Every seat in the table rises now (t-5806): recall is judged on the
    // same marks, on its own lines.
    assert!(judge_seat(&crate::jev::RECALL, &full).is_some());
}

/// Marks that clear every agreement line of `seat` from `at` on (t-6342):
/// three that say no — the record's evidence that the label can — then as
/// many that say yes as the seat's budget needs to bound above its line with
/// those three inside, each beside a baseline mark that is right half the
/// time.
fn marks_that_rise(seat: &JevUse, at: usize) -> Vec<Value> {
    let misses = seat.negatives_wanted.expect("a promoting seat");
    let marks = marks_that_can_clear(seat).expect("a width the line can be cleared on");
    (0..marks)
        .map(|n| {
            json!({"at": at + n, "label": n, "agreed": n >= misses, "baselineAgreed": n % 2 == 0})
        })
        .collect()
}

/// A window of answered rows, then `marks`.
fn window_then(seat: &JevUse, marks: impl FnOnce(usize) -> Vec<Value>) -> Vec<Value> {
    let wanted = window_wanted_for(seat).expect("a promoting seat");
    let mut rows: Vec<Value> = (0..wanted)
        .map(|at| json!({"at": at, "outcome": "answered", "elapsedMs": 1}))
        .collect();
    rows.extend(marks(wanted));
    rows
}

/// A label that never says no is not evidence, however many times it says
/// yes (t-6342): this machine's placement seat rose on thirty marks that all
/// said yes, and any answer would have earned them. Every promoting seat
/// holds on a record with fewer disagreements than it asks for — and the
/// same record with three of them, counted over the version's whole record
/// rather than the window, rises.
#[test]
fn a_seat_whose_labels_never_say_no_cannot_rise() {
    for seat in crate::jev::JEV_USES.iter().filter(|row| row.promotes) {
        let all_yes = window_then(seat, |at| {
            marks_that_rise(seat, at)
                .into_iter()
                .map(|mut mark| {
                    mark["agreed"] = json!(true);
                    mark
                })
                .collect()
        });
        assert_eq!(
            judge_seat(seat, &all_yes).expect("judged").verdict,
            Verdict::Hold(Line::OneSided {
                disagreed: 0,
                wanted: crate::jev::NEGATIVES_WANTED
            }),
            "{}",
            seat.id
        );
        let said_no = window_then(seat, |at| marks_that_rise(seat, at));
        assert_eq!(
            judge_seat(seat, &said_no).expect("judged").verdict,
            Verdict::Rise,
            "{}",
            seat.id
        );
    }
}

/// A seat's agreement is held to its cheapest reader's share over the same
/// marks, not only to its own floor (t-6342): a placement seat right four
/// times in five beside a "today's tab" that was right every time has
/// earned nothing, and one with no baseline marks at all has not been
/// compared with anything yet. A seat whose marks grade only its own act has
/// no such reader and is held to its floor alone.
#[test]
fn a_seat_must_beat_its_baseline_not_only_its_floor() {
    let seat = &crate::jev::PLACEMENT;
    assert!(seat.baseline.binds());
    let beaten = window_then(seat, |at| {
        marks_that_rise(seat, at)
            .into_iter()
            .map(|mut mark| {
                mark["baselineAgreed"] = json!(true);
                mark
            })
            .collect()
    });
    let Verdict::Hold(Line::Baseline {
        bound_permille,
        baseline_permille,
    }) = judge_seat(seat, &beaten).expect("judged").verdict
    else {
        panic!("a seat the baseline matches rose");
    };
    assert_eq!(baseline_permille, 1_000);
    assert!(bound_permille >= seat.agreement_floor_permille.expect("a budget"));
    // An acting seat the baseline matches falls.
    let mut acting = beaten.clone();
    acting.insert(0, json!({"transition": ROSE}));
    assert!(matches!(
        judge_seat(seat, &acting).expect("judged").verdict,
        Verdict::Fall(Line::Baseline { .. })
    ));
    // No baseline marks: nothing to beat yet.
    let unmeasured = window_then(seat, |at| {
        marks_that_rise(seat, at)
            .into_iter()
            .map(|mut mark| {
                mark.as_object_mut()
                    .expect("a mark")
                    .remove("baselineAgreed");
                mark
            })
            .collect()
    });
    assert!(matches!(
        judge_seat(seat, &unmeasured).expect("judged").verdict,
        Verdict::Hold(Line::TooFewBaseline { compared: 0, .. })
    ));
    // Beaten comfortably: it rises.
    assert_eq!(
        judge_seat(seat, &window_then(seat, |at| marks_that_rise(seat, at)))
            .expect("judged")
            .verdict,
        Verdict::Rise
    );
    // A seat with no baseline is held to its floor alone.
    let own = &crate::jev::BROWSER_READ;
    assert!(!own.baseline.binds());
    let unmarked = window_then(own, |at| {
        marks_that_rise(own, at)
            .into_iter()
            .map(|mut mark| {
                mark.as_object_mut()
                    .expect("a mark")
                    .remove("baselineAgreed");
                mark
            })
            .collect()
    });
    assert_eq!(
        judge_seat(own, &unmarked).expect("judged").verdict,
        Verdict::Rise
    );
}

/// A seat whose label found nothing to grade says so — "no label" — rather
/// than reading as one still counting a thin sample (t-6342): the step seat's
/// every judgment was held back on its wire, so each label row names why it
/// compares nothing and not one carries a mark.
#[test]
fn a_seat_whose_rows_all_compare_nothing_has_no_label() {
    let seat = &crate::jev::ZO_STEP_EFFORT;
    let rows = window_then(seat, |at| {
        (0..5)
            .map(|n| json!({"at": at + n, "label": n, "notCompared": "not_carried"}))
            .collect()
    });
    assert_eq!(
        judge_seat(seat, &rows).expect("judged").verdict,
        Verdict::Hold(Line::Unlabeled { withheld: 5 })
    );
    let mut acting = rows;
    acting.insert(0, json!({"transition": ROSE}));
    assert_eq!(
        judge_seat(seat, &acting).expect("judged").verdict,
        Verdict::Keep,
        "a fresh window with nothing to grade is not evidence against an acting seat"
    );
}
