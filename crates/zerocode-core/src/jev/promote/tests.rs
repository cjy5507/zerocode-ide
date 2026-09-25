use super::*;
use crate::jev::choice::ChoiceRefusal;
use crate::jev::summary::Tally;

/// The rows a floor of 950 per thousand can be cleared on — what the clean
/// window is sized to, and what a thin one is one short of.
fn wanted() -> usize {
    crate::jev::summary::rows_that_can_clear(950)
}

/// A request `seat` was asked at `at`, answered: stamped with the rubric
/// the seat asks now and named as its labels name a request
/// ([`JevUse::request_name`] — `at` under each key), so a label of `at`
/// grades it (t-6877).
fn asked_by(seat: &JevUse, at: usize) -> Value {
    use serde_json::json;
    let mut row = json!({"at": at, "outcome": "answered", "elapsedMs": 1, "rubricVersion": seat.rubric_version});
    for key in seat.request_name {
        row[*key] = json!(at);
    }
    row
}

/// The name a label of `seat` gives request `n`: `n` under each of the
/// seat's naming keys, joined as the reader joins them — or `n` itself for
/// a seat whose labels name no request.
fn named(seat: &JevUse, n: usize) -> Value {
    use serde_json::json;
    if seat.request_name.is_empty() {
        json!(n)
    } else {
        json!(
            seat.request_name
                .iter()
                .map(|_| n.to_string())
                .collect::<Vec<_>>()
                .join(":")
        )
    }
}

/// A label row of `seat` grading request `n`, written at `at`, as the
/// seat's writers write one now: the request's name ([`named`]) and the
/// time it was asked — [`asked_by`] asks request `n` at `n` — which is all
/// that picks out one asking of words asked again (t-6877 round 3,
/// [`crate::jev::Naming::Words`]).
fn label_of(seat: &JevUse, at: usize, n: usize) -> Value {
    json!({"at": at, "label": named(seat, n), (REQUEST_AT.canonical): n})
}

/// A mark of `seat` grading request `n`, written at `at`, as the seat's
/// writer writes one (t-6877): a label row naming the request, for a seat
/// whose labels name one ([`JevUse::request_name`], [`label_of`]); for a
/// seat whose marks sit on the request row itself — the screen seats', the
/// summons', the judgment cache's, the branching fork's — a request row
/// carrying it.
fn mark(seat: &JevUse, at: usize, n: usize, fields: Value) -> Value {
    let mut row = if seat.request_name.is_empty() {
        asked_by(seat, at)
    } else {
        label_of(seat, at, n)
    };
    for (key, value) in fields.as_object().expect("the mark's fields") {
        row[key.as_str()] = value.clone();
    }
    row
}

/// How many requests a ledger of `seat` holds before its marks: enough for
/// a window, and enough for every mark the seat's line needs to name one.
fn asked_count(seat: &JevUse) -> usize {
    window_wanted_for(seat)
        .expect("a promoting seat")
        .max(marks_that_can_clear(seat).unwrap_or(0))
}

/// A rise as the judge writes it for `seat`: naming the rubric it asks.
fn rose(seat: &JevUse) -> Value {
    serde_json::json!({(TRANSITION.canonical): ROSE, "rubricVersions": [seat.rubric_version]})
}

#[test]
fn a_no_reader_seat_keeps_its_standing_when_its_first_positive_label_arrives() {
    for seat in [&crate::jev::RECALL, &crate::jev::PLACEMENT] {
        let wanted = window_wanted_for(seat).expect("a promoting seat");
        let mut rows: Vec<Value> = (0..wanted).map(|at| asked_by(seat, at)).collect();
        rows.push(rose(seat));
        rows.push(mark(seat, wanted, 0, json!({"agreed": true})));
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
            let mut rows: Vec<Value> = (0..wanted).map(|at| asked_by(seat, at)).collect();
            rows.push(rose(seat));
            rows.extend(
                (0..compared).map(|at| mark(seat, wanted + at, at, json!({"agreed": agreed}))),
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
        let floor = seat.agreement_rows_wanted.expect("a label sample floor");
        let asked = asked_count(seat);
        let rows: Vec<Value> = (0..asked).map(|at| asked_by(seat, at)).collect();
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
        marked.extend(marks_that_rise(seat, asked));
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
fn thirty_marks(seat: &JevUse, at: usize) -> Vec<Value> {
    const MARKS: usize = 30;
    const MISSES: usize = 3;
    (0..MARKS)
        .map(|n| mark(seat, at + n, n, json!({"agreed": n >= MISSES})))
        .collect()
}

/// A full window of answered rows, then `marks` — the ledger a seat keeps
/// when the vendor's alias moves under it (t-6187). `on_b` of the marks, the
/// newest, name version `B`; the rest name `A`; `None` writes no version at
/// all, the shape of every row before versions were recorded.
fn marks_across_a_version_change(
    seat: &JevUse,
    marks: impl FnOnce(&JevUse, usize) -> Vec<Value>,
    on_b: Option<usize>,
) -> Vec<Value> {
    let asked = asked_count(seat);
    let mut rows = window_then(seat, marks);
    if let Some(on_b) = on_b {
        let count = rows.len() - asked;
        for (n, mark) in rows[asked..].iter_mut().enumerate() {
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
    // Seats whose labels are rows of their own, naming their requests: a
    // mark on a request row is that request's, and its version the
    // request's (the window's cut, below).
    for seat in [&crate::jev::PLACEMENT, &crate::jev::STALL] {
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
            |seat, at| {
                let mut marks = thirty_marks(seat, at);
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
    // A seat whose writer never versioned its words and whose labels name
    // their request by its attempt: every row here is its series, as every
    // ledger was read before.
    let seat = &crate::jev::CHALLENGER;
    assert_eq!(seat.request_name, &["attempt"]);
    let answered = |at: i64, model: Option<&str>| {
        let mut row =
            json!({"at": at, "attempt": format!("a{at}"), "outcome": "answered", "elapsedMs": 1});
        if let Some(model) = model {
            row[crate::jev::summary::MODEL.canonical] = json!(model);
        }
        row
    };
    let rows = [
        answered(1, Some("B")),
        answered(2, None),
        answered(3, Some("A")),
        json!({"at": 4, "attempt": "a4", "outcome": "timeout"}),
        answered(5, Some("B")),
        json!({"at": 6, "label": "a5", "agreed": true}),
        answered(7, None),
        json!({"at": 8, "transition": ROSE}),
    ];
    let version = on_the_newest_version(seat, &rows);
    assert_eq!((version.model, version.cut), (Some("B"), Some("A")));
    assert!(
        same_rows(&version.requests, &rows[3..7]),
        "after the newest request A answered, and never the transition"
    );
    assert!(same_rows(&version.marks, &rows[3..7]));
    assert_eq!(
        asked_toward_judgment(seat, &rows),
        3,
        "the timeout, B and the unnamed answer"
    );

    let unnamed = [
        answered(1, None),
        json!({"at": 2, "label": "a1", "agreed": false}),
    ];
    let version = on_the_newest_version(seat, &unnamed);
    assert_eq!((version.model, version.cut), (None, None));
    assert!(
        same_rows(&version.requests, &unnamed[..]),
        "a ledger with no versions is read whole"
    );

    // A late label of A's answer is A's — its request says so, whether or
    // not the label spells it (t-6877) — though written after B's last
    // request; it does not move which version is answering now (a request
    // says that), and it cuts none of B's rows away. A label spelling a
    // version its request was not answered by grades nothing.
    let late = [
        answered(1, Some("A")),
        answered(2, Some("B")),
        json!({"at": 3, "label": "a1", "agreed": false, "model": "A"}),
        json!({"at": 4, "label": "a1", "agreed": false}),
        json!({"at": 5, "label": "a2", "agreed": true}),
        json!({"at": 6, "label": "a2", "agreed": false, "model": "A"}),
    ];
    let version = on_the_newest_version(seat, &late);
    assert_eq!((version.model, version.cut), (Some("B"), Some("A")));
    // Of A's two labels the newest names it; B's label that spells A grades
    // nothing, and is not in the series at all.
    assert!(same_rows(
        &version.requests,
        &[late[1].clone(), late[3].clone(), late[4].clone()]
    ));
    assert!(
        version.marks.len() == 2 && version.marks[0] == &late[1] && version.marks[1] == &late[4],
        "B's request and B's label, and nothing of A's: {:?}",
        version.marks
    );
}

/// Whether `held` is exactly `rows`, row for row.
fn same_rows(held: &[&Value], rows: &[Value]) -> bool {
    held.len() == rows.len() && held.iter().zip(rows).all(|(held, row)| *held == row)
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
        rows.push(json!({"kind": "judgment", "at": at, "attempt": "s@1", "step": n, "outcome": "answered", "elapsedMs": 1, model: "jev-1.13.0"}));
        rows.push(json!({"kind": "step", "at": at + 1, model: chat[n % chat.len()]}));
        rows.push(json!({"kind": "label", "at": at + 2, "label": format!("s@1:{n}"), "agreed": n >= crate::jev::NEGATIVES_WANTED, "baselineAgreed": n % 2 == 0}));
        rows.push(json!({"kind": "step", "at": at + 3, model: chat[(n + 1) % chat.len()]}));
    }
    let version = on_the_newest_version(seat, &rows);
    assert_eq!(
        (version.model, version.cut),
        (Some("jev-1.13.0"), None),
        "a step's chat model is not the version that answered"
    );
    let evidence: Vec<Value> = rows
        .iter()
        .filter(|row| row["kind"] != "step")
        .cloned()
        .collect();
    assert!(
        same_rows(&version.requests, &evidence),
        "the step rows are nobody's evidence"
    );
    assert!(
        same_rows(&version.marks, &evidence),
        "no step cuts the labels away"
    );
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.cut.as_deref()),
        (wanted, None)
    );
    assert_eq!(judged.verdict, Verdict::Rise, "{judged:?}");

    // A ledger whose requests all went unanswered names no version at all —
    // not the chat model its steps ran on.
    let unanswered = [
        json!({"kind": "judgment", "at": 1, "attempt": "s@1", "step": 1, "outcome": "no_key"}),
        json!({"kind": "step", "at": 2, model: "claude-opus-5"}),
        json!({"kind": "label", "at": 3, "label": "s@1:1", "agreed": false}),
    ];
    let version = on_the_newest_version(seat, &unanswered);
    assert_eq!((version.model, version.cut), (None, None));
    assert!(same_rows(
        &version.marks,
        &[unanswered[0].clone(), unanswered[2].clone()]
    ));
}

/// A seat already acting is judged on the new version's rows and keeps
/// acting while they are too few to say anything — the standing is read from
/// the whole ledger — and the judgment's cadence starts again with the
/// version, so the screen's countdown and the judgment still land on one row.
#[test]
fn a_change_of_version_restarts_the_window_and_leaves_the_standing() {
    let seat = &crate::jev::SUMMON;
    let wanted = window_wanted_for(seat).expect("summon rises");
    let row = |at: usize, model: &str| {
        let mut row = asked_by(seat, at);
        row["agreed"] = json!(true);
        row["model"] = json!(model);
        row
    };
    let mut rows: Vec<Value> = (0..wanted).map(|at| row(at, "A")).collect();
    rows.push(rose(seat));
    rows.extend((0..3).map(|n| row(wanted + 1 + n, "B")));
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(judged.verdict, Verdict::Keep, "{judged:?}");
    assert_eq!(judged.window.rows, 3);
    assert_eq!(asked_toward_judgment(seat, &rows), 3);
    assert_eq!(
        rows_to_next_judgment(seat, asked_toward_judgment(seat, &rows)),
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
        for misses in [1, 2] {
            // A window with marks that clear every agreement line, so the
            // one line left to clear is the answer line.
            // The misses are the newest requests, inside the window however
            // many requests the marks needed to name.
            let asked = asked_count(seat);
            let mut rows: Vec<Value> = (0..asked)
                .map(|at| {
                    let mut row = asked_by(seat, at);
                    if at >= asked - misses {
                        row["outcome"] = json!("timeout");
                    }
                    row
                })
                .collect();
            rows.extend(marks_that_rise(seat, asked));
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
    // A transition's key spelled with an escape is the key the parser
    // reads (t-6877 round 3).
    let escaped = format!(
        "{}{}\n",
        text(&rows),
        r#"{"at":4,"\u0074ransition":"fall"}"#
    );
    assert_eq!(
        stand_in(&escaped),
        Stand::Recording,
        "an escaped fall is a fall"
    );
}

#[test]
fn only_a_change_is_written_down() {
    let seat = &crate::jev::PLACEMENT;
    let held = window(200, 200, Some(600));
    assert_eq!(transition_row(seat, 9, Verdict::Keep, &held), None);
    assert_eq!(
        transition_row(seat, 9, Verdict::Hold(Line::Schema { rows: 1 }), &held),
        None
    );

    let rose = transition_row(seat, 9, Verdict::Rise, &held).expect("a rise is written");
    assert_eq!(rose[TRANSITION.canonical], ROSE);
    assert_eq!(rose["rows"], 200);
    assert_eq!(
        rose[RUBRIC_VERSIONS.canonical],
        serde_json::json!([seat.rubric_version]),
        "a transition names the rubric it was decided on"
    );
    assert_eq!(
        rose[ON_LINE],
        serde_json::Value::Null,
        "a rise broke no line"
    );

    let fell = transition_row(
        seat,
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
    let row = transition_row(&crate::jev::PLACEMENT, 9, Verdict::Rise, &held).expect("row");
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
        RUBRIC_VERSIONS.canonical,
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
    let row = |at: i64, agreed: bool| json!({"at": at, "outcome": "answered", "elapsedMs": 600, "requests": 1, "agreed": agreed, "rubricVersion": seat.rubric_version});
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
    // A label row's mark counts too, and only from the window's first row on
    // — on the orchestration seat whose labels are rows of their own, each
    // naming the stall it grades (t-6877); the summons' marks sit on its
    // requests.
    let stall = &crate::jev::STALL;
    assert_eq!(window_wanted_for(stall), Some(wanted), "the same lines");
    let asked = |at: i64, key: String| json!({"at": at, "stall": key, "outcome": "answered", "elapsedMs": 600, "requests": 1, "rubricVersion": stall.rubric_version});
    // Three old stalls, graded no: outside the window, and on the record the
    // label is known to be able to say no by (t-6342).
    let mut labelled: Vec<serde_json::Value> = (1..=3)
        .map(|n: i64| asked(-10 - n, format!("old{n}")))
        .collect();
    labelled.extend((0..wanted as i64).map(|at| asked(at, format!("k{at}"))));
    labelled.extend((0..wanted as i64).map(|at| {
        json!({"at": at, "label": format!("k{at}"), "agreed": true, "baselineAgreed": at % 2 == 0})
    }));
    labelled.extend(
        (1..=3).map(|n: i64| json!({"at": -n, "label": format!("old{n}"), "agreed": false})),
    );
    let judged = judge_seat(stall, &labelled).expect("judged");
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
            mark(
                seat,
                at + n,
                n,
                json!({"agreed": n >= misses, "baselineAgreed": n % 2 == 0}),
            )
        })
        .collect()
}

/// A window of answered rows — enough of them for every mark to name one
/// ([`asked_count`]) — then `marks`, handed the seat and the row the marks
/// start at.
fn window_then(seat: &JevUse, marks: impl FnOnce(&JevUse, usize) -> Vec<Value>) -> Vec<Value> {
    let asked = asked_count(seat);
    let mut rows: Vec<Value> = (0..asked).map(|at| asked_by(seat, at)).collect();
    rows.extend(marks(seat, asked));
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
        let all_yes = window_then(seat, |seat, at| {
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
        let said_no = window_then(seat, marks_that_rise);
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
    let beaten = window_then(seat, |seat, at| {
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
    let unmeasured = window_then(seat, |seat, at| {
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
        judge_seat(seat, &window_then(seat, marks_that_rise))
            .expect("judged")
            .verdict,
        Verdict::Rise
    );
    // A seat with no baseline is held to its floor alone.
    let own = &crate::jev::BROWSER_READ;
    assert!(!own.baseline.binds());
    let unmarked = window_then(own, |own, at| {
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
    let rows = window_then(seat, |seat, at| {
        (0..5)
            .map(|n| mark(seat, at + n, n, json!({"notCompared": "not_carried"})))
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

/* ---- one rubric's evidence is not another's (t-6877) ------------------------ */

/// A tool text guard request as its writer files it: `judged` is the name a
/// label repeats, `rubricVersion` says which words asked, and `model` which
/// version answered — none when nothing did.
fn guard_request(at: usize, judged: u64, rubric: u32, model: Option<&str>, outcome: &str) -> Value {
    use serde_json::json;
    let mut row = json!({
        "at": at,
        "judged": judged,
        "rubricVersion": rubric,
        "outcome": outcome,
        "elapsedMs": 400,
        "requests": 1,
    });
    if let Some(model) = model {
        row[crate::jev::summary::MODEL.canonical] = json!(model);
    }
    row
}

/// The guard's label row for the request named `judged`, shaped as
/// `write_text_labels` shapes it: no version of its own — the request it
/// names carries that.
fn guard_label(at: usize, judged: u64, agreed: bool) -> Value {
    use serde_json::json;
    json!({
        "kind": "label",
        "at": at,
        "label": judged.to_string(),
        "agreed": agreed,
        "baselineAgreed": at.is_multiple_of(2),
    })
}

/// The version that answers every request here.
const ANSWERING: &str = "jev-1.13.0";

/// A full window of answered requests under `rubric`, named from `first`
/// on, at `at`.. — and the marks that clear every line of `seat`, each
/// naming its own request.
fn guard_window_that_rises(seat: &JevUse, rubric: u32, first: u64, at: usize) -> Vec<Value> {
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let misses = seat.negatives_wanted.expect("the guard asks for negatives");
    let marks = marks_that_can_clear(seat).expect("a width the line can be cleared on");
    assert!(marks <= wanted, "every mark names a request of the window");
    let mut rows: Vec<Value> = (0..wanted)
        .map(|n| {
            guard_request(
                at + n,
                first + n as u64,
                rubric,
                Some(ANSWERING),
                "answered",
            )
        })
        .collect();
    rows.extend((0..marks).map(|n| guard_label(at + wanted + n, first + n as u64, n >= misses)));
    rows
}

/// `n` answered requests under `rubric`, named from `first` on, at `at`..
fn guard_requests(n: usize, rubric: u32, first: u64, at: usize) -> Vec<Value> {
    (0..n)
        .map(|k| {
            guard_request(
                at + k,
                first + k as u64,
                rubric,
                Some(ANSWERING),
                "answered",
            )
        })
        .collect()
}

/// A rise as the judge wrote it before transitions named a rubric.
fn unversioned_rise(at: usize) -> Value {
    serde_json::json!({"at": at, (TRANSITION.canonical): ROSE})
}

/// A rise the judge writes now: naming the rubric it was decided on.
fn rise_on(at: usize, rubrics: &[u32]) -> Value {
    serde_json::json!({"at": at, (TRANSITION.canonical): ROSE, "rubricVersions": rubrics})
}

/// The text guard as a fixture of its history: its row asking version 2,
/// the words it asked from t-6982 on, with version 1 behind them. Pinned
/// here and not read off today's row, because the guard's words move on —
/// its row asks whatever version its writer stamps now — and the tests
/// that stand on this read a history of one rubric following another;
/// what the row asks today is held by
/// `todays_text_guard_stands_on_its_own_words_alone` (t-6877 round 3).
static GUARD_ASKING_TWO: JevUse = JevUse {
    rubric_version: 2,
    ..crate::jev::TOOL_TEXT_GUARD
};

/// The history fixture above, as the tests hold a seat.
fn guard() -> &'static JevUse {
    &GUARD_ASKING_TWO
}

/// Today's text guard stands on the words its row asks now and on nothing
/// its older words earned (t-6877 round 3, astra: the fixture above is a
/// history, this is the row) — whatever version that is: the version before
/// it with a full window, its marks and its rise, then twenty requests of
/// today's words, is not due, holds on the twenty with nothing compared and
/// stands at recording, off the rows and off the text; today's own window
/// and marks are due and rise, and the rise names today's words.
#[test]
fn todays_text_guard_stands_on_its_own_words_alone() {
    let seat = &crate::jev::TOOL_TEXT_GUARD;
    let today = seat.rubric_version;
    let before = today - 1;
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let history = || {
        let mut rows = guard_window_that_rises(seat, before, 1, 0);
        rows.push(rise_on(5_000, &[before]));
        rows
    };
    let mut thin = history();
    thin.extend(guard_requests(JUDGED_EVERY_ROWS, today, 1_000, 10_000));
    assert!(
        !judgment_due(seat, &thin),
        "twenty requests of today's words are not a window"
    );
    let judged = judge_seat(seat, &thin).expect("judged");
    assert_eq!(
        (judged.verdict, judged.agreement.compared),
        (
            Verdict::Hold(Line::TooFewRows {
                rows: JUDGED_EVERY_ROWS,
                wanted
            }),
            0
        ),
        "the older words' marks grade nothing of today's: {judged:?}"
    );
    assert_eq!(standing(seat, &thin), Stand::Recording);
    assert_eq!(standing_in(seat, &text_of(&thin)), Stand::Recording);

    let mut own = history();
    own.extend(guard_window_that_rises(seat, today, 1_000, 10_000));
    assert!(judgment_due(seat, &own), "today's own window is full");
    let judged = judge_seat(seat, &own).expect("judged");
    assert_eq!(judged.verdict, Verdict::Rise, "{judged:?}");
    assert_eq!(
        judged.agreement.compared,
        marks_that_can_clear(seat).expect("a width"),
        "today's own marks, and only those"
    );
    let rose = transition_row(seat, 20_000, judged.verdict, &judged.window).expect("a rise");
    assert_eq!(rose[RUBRIC_VERSIONS.canonical], serde_json::json!([today]));
}

/// A thick first-rubric series does not judge a thin second-rubric window
/// (t-6877, astra m-7097): the guard's words moved from version 1 to 2 with a
/// full window and forty marks behind version 1 and twenty requests under
/// version 2 — the judge is not due (twenty of version 2 are not a window),
/// and asked anyway it holds on version 2's twenty rows with nothing
/// compared, never on version 1's evidence.
#[test]
fn a_thick_version_one_series_does_not_judge_a_thin_version_two_window() {
    let seat = guard();
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let mut rows = guard_window_that_rises(seat, 1, 1, 0);
    let thin = JUDGED_EVERY_ROWS;
    rows.extend(guard_requests(thin, 2, 1_000, 10_000));
    assert!(
        !judgment_due(seat, &rows),
        "twenty second-version requests are not a window, whatever the first version left"
    );
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        judged.verdict,
        Verdict::Hold(Line::TooFewRows { rows: thin, wanted }),
        "{judged:?}"
    );
    assert_eq!(judged.window.rows, thin, "the window is version 2's own");
    assert_eq!(
        judged.agreement.compared, 0,
        "version 1's marks grade nothing of version 2"
    );
    assert_eq!(judged.model.as_deref(), Some(ANSWERING));
    assert_eq!(
        judged.cut, None,
        "a change of rubric is not a change of model"
    );
}

/// A late label on an old request stays in its own series (t-6877): the
/// label of a version 1 request, written after version 2's window, grades
/// version 1 — it neither joins version 2's comparisons nor, when it names
/// the older model that answered its request, cuts version 2's marks away.
#[test]
fn a_late_label_on_an_old_request_stays_in_its_own_series() {
    let seat = guard();
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let graded = 5;
    let mut rows = vec![guard_request(0, 1, 1, Some(ANSWERING), "answered")];
    rows.extend(guard_requests(wanted, 2, 100, 10));
    rows.extend((0..graded).map(|n| guard_label(10 + wanted + n, 100 + n as u64, true)));
    let late_at = 10 + wanted + graded;
    let mut late = rows.clone();
    late.push(guard_label(late_at, 1, false));
    let judged = judge_seat(seat, &late).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (graded, graded),
        "the late label grades version 1, not version 2: {judged:?}"
    );
    assert_eq!(
        judged.verdict,
        Verdict::Hold(Line::TooFewCompared {
            compared: graded,
            wanted: seat.agreement_rows_wanted.expect("a sample floor")
        })
    );
    // Naming the version that answered its own request — an older one —
    // it is still version 1's label, and version 2's marks stand.
    let mut older = rows;
    let mut label = guard_label(late_at, 1, false);
    label[crate::jev::summary::MODEL.canonical] = serde_json::json!("jev-1.12.0");
    older.push(label);
    let judged = judge_seat(seat, &older).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.cut.as_deref()),
        (graded, None),
        "version 2's five comparisons are not cut by version 1's late label: {judged:?}"
    );
}

/// A row that names no rubric version is version 1's (t-6877): every row
/// written before versions were recorded, and every seat whose writer never
/// versioned its words. A seat asking version 1 counts them and stands on
/// their rise; a seat asking version 2 counts none of them, and their rise is
/// not its rise.
#[test]
fn an_unversioned_row_reads_as_version_one() {
    use serde_json::json;
    let answered =
        |at: usize| json!({"at": at, "outcome": "answered", "elapsedMs": 1, "requests": 1});
    let rows: Vec<Value> = (0..3)
        .map(answered)
        .chain(std::iter::once(unversioned_rise(3)))
        .chain((4..7).map(answered))
        .collect();
    let first = &crate::jev::PLACEMENT;
    assert_eq!(
        crate::worker_placement::WORKER_PLACEMENT_RUBRIC_VERSION,
        1,
        "placement asks version 1: its rows may name none"
    );
    let judged = judge_seat(first, &rows).expect("judged");
    assert_eq!(
        judged.window.rows, 6,
        "unversioned rows are version 1's requests"
    );
    assert_eq!(
        judged.verdict,
        Verdict::Keep,
        "and their rise stands for it: {judged:?}"
    );

    let second = guard();
    let wanted = window_wanted_for(second).expect("the guard rises");
    let judged = judge_seat(second, &rows).expect("judged");
    assert_eq!(
        judged.window.rows, 0,
        "version 1's rows are not version 2's"
    );
    assert_eq!(
        judged.verdict,
        Verdict::Hold(Line::TooFewRows { rows: 0, wanted }),
        "and version 1's rise is not version 2's: {judged:?}"
    );
}

/// A change of rubric returns a risen seat to recording (t-6877, the
/// contract of run-6774's audit m-6856): a rise earned under version 1 does
/// not stand under version 2 — with nothing yet asked under version 2, with
/// three requests, and the other way round, a version 2 rise does not stand
/// for a seat whose words went back to version 1 (a rollback starts
/// recording; it revives nothing). Only a rise naming the rubric the seat
/// asks now stands.
#[test]
fn a_rubric_change_returns_a_risen_seat_to_recording() {
    let seat = guard();
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let mut rows = guard_window_that_rises(seat, 1, 1, 0);
    rows.push(unversioned_rise(5_000));
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        judged.verdict,
        Verdict::Hold(Line::TooFewRows { rows: 0, wanted }),
        "nothing asked under version 2 yet, and version 1's rise is not version 2's: {judged:?}"
    );
    rows.extend(guard_requests(3, 2, 1_000, 10_000));
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        judged.verdict,
        Verdict::Hold(Line::TooFewRows { rows: 3, wanted }),
        "three requests under version 2, still recording: {judged:?}"
    );
    // A rise that names version 1 is the same rise, read the same way.
    let named = rows.len() - 4;
    rows[named] = rise_on(5_000, &[1]);
    assert_eq!(
        judge_seat(seat, &rows).expect("judged").verdict,
        Verdict::Hold(Line::TooFewRows { rows: 3, wanted })
    );
    // And a rise that names the rubric the seat asks now stands.
    rows[named] = rise_on(5_000, &[2]);
    assert_eq!(
        judge_seat(seat, &rows).expect("judged").verdict,
        Verdict::Keep
    );

    // Rolled back: a seat asking version 1 does not stand on version 2's rise.
    let first = &crate::jev::PLACEMENT;
    let answered = |at: usize| serde_json::json!({"at": at, "outcome": "answered", "elapsedMs": 1, "requests": 1});
    let rolled_back: Vec<Value> = (0..3)
        .map(answered)
        .chain(std::iter::once(rise_on(3, &[2])))
        .chain((4..7).map(answered))
        .collect();
    let judged = judge_seat(first, &rolled_back).expect("judged");
    assert!(
        matches!(
            judged.verdict,
            Verdict::Hold(Line::TooFewRows { rows: 6, .. })
        ),
        "a rollback starts recording: {judged:?}"
    );
}

/// The second version rises again on its own sample (t-6877, the positive
/// control): version 1's window, marks and rise on the ledger, then version
/// 2's own full window and its own forty marks — the judge is due, and it
/// says rise, on version 2's evidence alone.
#[test]
fn the_second_version_rises_again_on_its_own_sample() {
    let seat = guard();
    let mut rows = guard_window_that_rises(seat, 1, 1, 0);
    rows.push(unversioned_rise(5_000));
    rows.extend(guard_window_that_rises(seat, 2, 1_000, 10_000));
    assert!(judgment_due(seat, &rows), "version 2's window is full");
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(judged.verdict, Verdict::Rise, "{judged:?}");
    assert_eq!(
        judged.window.rows,
        window_wanted_for(seat).expect("the guard rises")
    );
    assert_eq!(
        judged.agreement.compared,
        marks_that_can_clear(seat).expect("a width"),
        "version 2's own marks, and only those"
    );
}

/// A timeout names no model and belongs to the rubric that asked it (t-6877,
/// astra m-7141): three of them after version 2's answers are version 2's
/// failures — an acting seat is ended now, through the same cadence — while
/// three late timeouts of version 1 are not version 2's; and a version 2 that
/// has only timed out so far is a window of three failures, not a seat
/// standing on version 1.
#[test]
fn a_no_model_timeout_stays_in_the_current_rubrics_requests() {
    let seat = guard();
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let mut acting = guard_window_that_rises(seat, 2, 1_000, 10_000);
    acting.push(rise_on(15_000, &[2]));
    // One more answered request, so the count sits between two judgments
    // and only the fallback rule can make one due.
    acting.push(guard_request(16_000, 1_999, 2, Some(ANSWERING), "answered"));
    let timeouts = |rubric: u32| -> Vec<Value> {
        (0..FALLBACKS_THAT_END_IT as usize)
            .map(|n| guard_request(20_000 + n, 5_000 + n as u64, rubric, None, "timeout"))
            .collect()
    };
    let mut ended = acting.clone();
    ended.extend(timeouts(2));
    assert!(
        judgment_due(seat, &ended),
        "three fallbacks running end an acting seat now"
    );
    let judged = judge_seat(seat, &ended).expect("judged");
    assert_eq!(
        judged.verdict,
        Verdict::Fall(Line::Fallbacks {
            in_a_row: FALLBACKS_THAT_END_IT
        }),
        "{judged:?}"
    );
    assert_eq!(
        judged.window.failures,
        vec![("timeout".to_string(), FALLBACKS_THAT_END_IT as usize)],
        "the timeouts are in version 2's window, not dropped from it"
    );

    let mut late = acting;
    late.extend(timeouts(1));
    assert!(
        !judgment_due(seat, &late),
        "version 1's late timeouts are not version 2's failures"
    );
    assert_eq!(
        judge_seat(seat, &late).expect("judged").verdict,
        Verdict::Keep
    );

    let mut only_timeouts = guard_window_that_rises(seat, 1, 1, 0);
    only_timeouts.push(unversioned_rise(5_000));
    only_timeouts.extend(timeouts(2));
    let judged = judge_seat(seat, &only_timeouts).expect("judged");
    assert_eq!(
        judged.verdict,
        Verdict::Hold(Line::TooFewRows { rows: 3, wanted }),
        "a version that has only timed out borrows nothing: {judged:?}"
    );
    assert_eq!(judged.model, None, "nothing of version 2 has answered");
}

/// A label joins the request it names, and only that (t-6877, astra §3): a
/// label naming no request on the ledger compares nothing, two labels naming
/// one request are one comparison (the newest), and a label that spells a
/// rubric its request does not carry compares nothing either — the request
/// is the authority, and the reader guesses no version.
#[test]
fn an_orphan_a_duplicate_and_a_contradicting_label_are_not_compared() {
    let seat = guard();
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let mut rows = guard_requests(wanted, 2, 100, 0);
    let at = wanted;
    rows.push(guard_label(at, 100, true));
    rows.push(guard_label(at + 1, 100, false));
    rows.push(guard_label(at + 2, 999, true));
    let mut contradicting = guard_label(at + 3, 101, true);
    contradicting["rubricVersion"] = serde_json::json!(1);
    rows.push(contradicting);
    let mut agreeing = guard_label(at + 4, 102, true);
    agreeing["rubricVersion"] = serde_json::json!(2);
    rows.push(agreeing);
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (2, 1),
        "request 100 once, by its newest label; 102 once; 999 and the contradicting 101 never: {judged:?}"
    );
}

/// The standing read off a ledger's text is the one read off its parsed
/// rows, rubric and all (t-6877): a rise naming the seat's words stands, a
/// rise naming none is the first rubric's, a rise naming other words or
/// something that is not a version stands for nothing, a request of newer
/// words after the rise puts the seat behind its series, and the request
/// lines are read for their rubric wherever the key falls in the line.
#[test]
fn a_ledgers_text_stands_where_its_series_does() {
    use serde_json::json;
    let text = |rows: &[Value]| {
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
    };
    let first = &crate::jev::PLACEMENT;
    let second = guard();
    let both = |rows: &[Value]| {
        for seat in [first, second] {
            assert_eq!(
                standing_in(seat, &text(rows)),
                standing(seat, rows),
                "{}: the text and the rows disagree on {rows:?}",
                seat.id
            );
        }
    };
    let unversioned = [
        json!({"at": 1, "outcome": "answered", "rubricVersion": 1}),
        unversioned_rise(2),
    ];
    both(&unversioned);
    assert_eq!(standing_in(first, &text(&unversioned)), Stand::Applying);
    assert_eq!(standing_in(second, &text(&unversioned)), Stand::Recording);

    let named = [
        json!({"at": 1, "outcome": "answered", "rubricVersion": 2}),
        rise_on(2, &[2]),
    ];
    both(&named);
    assert_eq!(standing_in(second, &text(&named)), Stand::Applying);
    assert_eq!(standing_in(first, &text(&named)), Stand::Recording);

    let malformed = [json!({"at": 2, (TRANSITION.canonical): ROSE, "rubricVersions": "2"})];
    both(&malformed);
    assert_eq!(standing_in(second, &text(&malformed)), Stand::Recording);

    // The words went back: a request of newer words after the rise.
    let rolled_back = [
        rise_on(1, &[1]),
        json!({"at": 2, "outcome": "answered", "rubricFingerprint": "abc", "rubricVersion": 2}),
    ];
    both(&rolled_back);
    assert_eq!(standing_in(first, &text(&rolled_back)), Stand::Recording);
    // And a request of the seat's own or older words after it changes
    // nothing — nor does a label that only mentions the key in a value.
    let stood = [
        rise_on(1, &[1]),
        json!({"at": 2, "outcome": "answered", "rubricVersion": 1}),
        json!({"at": 3, "label": "x", "agreed": true, "note": "\"rubricVersion\": 9"}),
    ];
    both(&stood);
    assert_eq!(standing_in(first, &text(&stood)), Stand::Applying);
    // A torn last line is passed over, as ever.
    let torn = format!("{}{{\"at\": 9, \"transition\": \"fa", text(&named));
    assert_eq!(standing_in(second, &torn), Stand::Applying);
}

/// A rollback starts its own series and can rise again on it (t-6877, the
/// brief precheck's exit): the guard's words went 1 → 2 → 1 — version 1's
/// window, marks and rise, then version 2's, then the seat asks version 1
/// again. Nothing of the old run revives: not its rise, not its requests,
/// not its marks, not a late label of one of its requests — the series
/// starts after version 2's newest request, where the seat records. Once the
/// rolled-back words have a window and marks of their own the judge is due
/// and says rise, on those alone.
#[test]
fn a_rollback_starts_its_own_series_and_can_rise_again_on_it() {
    let seat = &JevUse {
        rubric_version: 1,
        ..*guard()
    };
    let wanted = window_wanted_for(seat).expect("the guard rises");
    let marks = marks_that_can_clear(seat).expect("a width");
    let mut rows = guard_window_that_rises(seat, 1, 1, 0);
    rows.push(rise_on(5_000, &[1]));
    rows.extend(guard_window_that_rises(seat, 2, 1_000, 10_000));
    rows.push(rise_on(15_000, &[2]));
    assert_eq!(
        standing(seat, &rows),
        Stand::Recording,
        "version 1's old rise is behind version 2's requests"
    );
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        (
            judged.verdict,
            judged.window.rows,
            judged.agreement.compared
        ),
        (Verdict::Hold(Line::TooFewRows { rows: 0, wanted }), 0, 0),
        "none of version 1's old rows or marks is the rolled-back series': {judged:?}"
    );

    // A late label of an old version 1 request, after the rollback: its
    // request is behind the series' start, so it grades nothing here.
    rows.push(guard_label(16_000, 1, false));
    // The rolled-back words' own window and marks.
    rows.extend(guard_window_that_rises(seat, 1, 50_000, 20_000));
    assert!(
        judgment_due(seat, &rows),
        "the rolled-back words' own window is full"
    );
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(judged.verdict, Verdict::Rise, "{judged:?}");
    assert_eq!(
        (judged.window.rows, judged.agreement.compared),
        (wanted, marks),
        "their own window and marks, and nothing of the old run"
    );
    assert_eq!(
        standing_in(seat, &text_of(&rows)),
        standing(seat, &rows),
        "the text reads the rollback as the rows do"
    );
}

/// A request that names something that is not a version is nobody's
/// evidence and cuts nothing (t-6877, the brief precheck: absent is not
/// malformed) — null, a word, a negative, a zero — whichever words the seat
/// asks; and an empty ledger is a seat that records.
#[test]
fn a_malformed_rubric_is_nobodys_evidence_and_an_empty_ledger_records() {
    use serde_json::json;
    let second = guard();
    let first = &JevUse {
        rubric_version: 1,
        ..*second
    };
    for seat in [first, second] {
        let wanted = window_wanted_for(seat).expect("the guard rises");
        assert_eq!(standing(seat, &[]), Stand::Recording);
        assert_eq!(standing_in(seat, ""), Stand::Recording);
        assert_eq!(
            judge_seat(seat, &[]).expect("judged").verdict,
            Verdict::Hold(Line::TooFewRows { rows: 0, wanted })
        );
        let own = seat.rubric_version;
        let mut rows = guard_requests(3, own, 100, 0);
        for (n, spelled) in [json!(null), json!("2"), json!(-1), json!(0), json!(1.5)]
            .into_iter()
            .enumerate()
        {
            let mut row = guard_request(10 + n, 900 + n as u64, own, Some("jev-0.1.0"), "answered");
            row["rubricVersion"] = spelled;
            rows.push(row);
        }
        rows.extend(guard_requests(2, own, 200, 20));
        let version = on_the_newest_version(seat, &rows);
        assert_eq!(
            (version.asked(), version.model, version.cut),
            (5, Some(ANSWERING), None),
            "{}: the malformed rows are neither counted nor a model to cut at",
            seat.id
        );
        assert_eq!(
            standing_in(seat, &text_of(&rows)),
            Stand::Recording,
            "{}: nor, read off the text, words newer than the seat's",
            seat.id
        );
    }
}

/// A ledger's text, one row a line, as a writer appends it.
fn text_of(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

/* ---- a label is one request's, and inherits everything from it (t-6877 round 2) ---- */

/// A late label of a request an older version answered is that version's
/// comparison and not the newer one's (t-6877 round 2, astra R1a): the
/// guard's version 2 asked once under model A and once under model B, and
/// a label naming A's request arrives after B's — carrying no model of its
/// own, as the guard's labels never do. The label's version is its
/// request's: it joins none of B's comparisons, and it cuts none of B's
/// marks away either.
#[test]
fn a_late_label_of_an_older_answering_version_is_that_versions_and_not_the_newer_ones() {
    let seat = guard();
    let rows = vec![
        guard_request(1, 11, 2, Some("jev-1.12.0"), "answered"),
        guard_request(2, 22, 2, Some(ANSWERING), "answered"),
        guard_label(3, 11, true),
    ];
    let version = on_the_newest_version(seat, &rows);
    assert_eq!(
        (version.model, version.cut, version.asked()),
        (Some(ANSWERING), Some("jev-1.12.0"), 1),
        "B answers now, A is where the requests were cut"
    );
    assert!(
        version.marks.iter().all(|row| LABEL.read(row).is_none()),
        "A's late label is not among B's marks: {:?}",
        version.marks
    );
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        judged.agreement.compared, 0,
        "a label of A's request is not a comparison of B's: {judged:?}"
    );
    // B's own label, written after A's late one, is B's comparison — the
    // late label of the other version cut nothing.
    let mut graded = rows;
    graded.push(guard_label(4, 22, true));
    let judged = judge_seat(seat, &graded).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (1, 1),
        "{judged:?}"
    );
}

/// A label grades one asking of a name and guesses none (t-6877 round 2,
/// astra R1b): a name a request carries can be asked twice — the recall and
/// mention seats name a request by the fingerprints of what was asked — so
/// a label names the time of the asking it grades ([`REQUEST_AT`]), and a
/// label that names none joins a name only while exactly one request above
/// it carries the name. Version 1 asked `5`, then version 2 asked `5`: the
/// label with no time could mean either and grades neither; the label
/// naming version 1's time is version 1's and not in version 2's series;
/// the label naming version 2's time is version 2's one comparison. A label
/// naming a time no request above it was asked at — its request trimmed
/// away — grades nothing, whatever request of the same name remains; and a
/// label of a name asked once joins it as it always did.
#[test]
fn a_label_grades_one_asking_of_a_name_and_guesses_none() {
    use serde_json::json;
    let seat = guard();
    let at = |mut label: Value, when: u64| {
        label[REQUEST_AT.canonical] = json!(when);
        label
    };
    let rows = vec![
        guard_request(10, 5, 1, Some(ANSWERING), "answered"),
        guard_request(20, 5, 2, Some(ANSWERING), "answered"),
        guard_label(30, 5, true),
        at(guard_label(31, 5, true), 10),
        at(guard_label(32, 5, false), 20),
        guard_request(40, 9, 2, Some(ANSWERING), "answered"),
        at(guard_label(41, 9, true), 35),
        guard_request(50, 7, 2, Some(ANSWERING), "answered"),
        guard_label(51, 7, true),
    ];
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (2, 1),
        "version 2's `5` once, by the label naming its time, and `7` once: {judged:?}"
    );
    // A time that is not a time names no request.
    let mut spelled = rows;
    spelled.push(at(guard_label(52, 7, false), 50));
    let mut wrong = guard_label(53, 7, false);
    wrong[REQUEST_AT.canonical] = json!("50");
    spelled.push(wrong);
    let judged = judge_seat(seat, &spelled).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (2, 0),
        "`7` by its newest well-formed label; the misspelled time grades nothing: {judged:?}"
    );
}

/// A label naming no time guesses no asking of words asked again (t-6877
/// round 3, astra R1b): the recall seat names a request by the fingerprints
/// of what was asked, so the same words asked twice under the same rubric
/// and answered by the same version are two askings, not one. A label
/// naming its asking's time grades that asking and no other, and of two
/// labels of one asking the newest counts; a label naming no time — every
/// label written before round 2 — grades neither asking, so it never
/// overwrites the exact label's mark, nor grades the one asking left above
/// it when its own was trimmed away; and a time two askings of the words
/// share names neither. Read at the judge every seat is read by.
#[test]
fn a_label_naming_no_time_grades_no_asking_of_words_asked_again() {
    use serde_json::json;
    let seat = &crate::jev::RECALL;
    let rubric = seat.rubric_version;
    let asked = |at: u64| {
        json!({"at": at, "outcome": "answered", "requests": 1, "elapsedMs": 1,
               "query": 7, "notes": 9, "rubricVersion": rubric, "model": "M"})
    };
    let label = |at: u64, agreed: bool, when: Option<u64>| {
        let mut row =
            json!({"at": at, "label": "7:9", "agreed": agreed, "baselineAgreed": !agreed});
        if let Some(when) = when {
            row[REQUEST_AT.canonical] = json!(when);
        }
        row
    };
    let marks = |rows: &[Value]| {
        let judged = judge_seat(seat, rows).expect("recall rises");
        let agreement = judged.agreement;
        (
            agreement.compared,
            agreement.agreed,
            agreement.baseline_compared,
            agreement.baseline_agreed,
        )
    };
    assert_eq!(
        marks(&[
            asked(10),
            asked(20),
            label(21, false, Some(20)),
            label(22, true, None)
        ]),
        (1, 0, 1, 1),
        "the label naming 20 stands; the one naming no time grades neither asking"
    );
    assert_eq!(
        marks(&[
            asked(10),
            asked(20),
            label(21, true, Some(10)),
            label(22, false, Some(20))
        ]),
        (2, 1, 2, 1),
        "each asking by its own time"
    );
    assert_eq!(
        marks(&[
            asked(10),
            asked(20),
            label(21, true, Some(20)),
            label(22, false, Some(20))
        ]),
        (1, 0, 1, 1),
        "two labels of one asking: the newest"
    );
    let mut answered_by_another = label(21, true, Some(20));
    answered_by_another[crate::jev::summary::MODEL.canonical] = json!("N");
    assert_eq!(
        marks(&[asked(10), asked(20), answered_by_another]),
        (0, 0, 0, 0),
        "a label spelling a version its asking was not answered by grades nothing"
    );
    assert_eq!(
        marks(&[asked(20), label(22, true, None)]),
        (0, 0, 0, 0),
        "its own asking trimmed away, one of the same words left: no time, no asking"
    );
    assert_eq!(
        marks(&[asked(20), label(22, true, Some(10))]),
        (0, 0, 0, 0),
        "a time no asking above it was made at names none"
    );
    assert_eq!(
        marks(&[asked(20), asked(20), label(22, true, Some(20))]),
        (0, 0, 0, 0),
        "a time two askings of the words share names neither"
    );
}

/// Two requests carrying a name the writer made for one request are two
/// requests nothing tells apart (t-6877 round 3, astra R1b) — a guard's
/// `judged` minted again by a replay, or by a turn key a compaction made
/// again: a label naming it grades neither, however alike the two were
/// asked and answered; a name carried once grades as ever.
#[test]
fn a_label_of_a_name_two_requests_carry_grades_neither() {
    let seat = guard();
    let rows = vec![
        guard_request(10, 5, 2, Some(ANSWERING), "answered"),
        guard_request(20, 5, 2, Some(ANSWERING), "answered"),
        guard_label(21, 5, true),
        guard_request(30, 6, 2, Some(ANSWERING), "answered"),
        guard_label(31, 6, false),
    ];
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (1, 0),
        "`6` once; `5`, carried by two requests, never: {judged:?}"
    );
}

/// A turn is the one name several requests carry by design (t-6877 round
/// 3, astra R1b): the routing seat's label names a turn's attempt, and
/// every judgment the turn asked — its own, the agents it spawned — carries
/// it. The label grades the turn while its rows could hand it nothing
/// different — one rubric, one answering version, one side of the series'
/// start — and nothing when they could.
#[test]
fn a_turns_label_grades_the_turn_its_several_requests_are() {
    use serde_json::json;
    let seat = &crate::jev::ROUTING;
    let asked = |at: u64, model: &str| {
        json!({"at": at, "outcome": "answered", "requests": 1, "elapsedMs": 1, "attempt": "s@1",
               "rubricVersion": seat.rubric_version, "model": model})
    };
    let label = json!({"at": 30, "label": "s@1", "attempt": "s@1", "agreed": true});
    let graded = |rows: &[Value]| {
        on_the_newest_version(seat, rows)
            .marks
            .iter()
            .any(|row| LABEL.read(row).is_some())
    };
    assert!(
        graded(&[
            asked(10, "M"),
            asked(11, "M"),
            asked(12, "M"),
            label.clone()
        ]),
        "one turn of three judgments, one comparison"
    );
    assert!(
        !graded(&[asked(10, "M"), asked(11, "N"), label]),
        "a turn two versions answered hands its label two versions: it grades neither"
    );
}

/// A label grades one part of its request where the seat's request has
/// several (t-6877 round 3): the compaction seat writes a label for each
/// block the compaction dropped — the turn a block is read again it regrets
/// that block, the rest agree once the window has passed — and each is one
/// comparison; two labels of one block are one, the newest. Read as one
/// request's duplicates, the regret written first would be overwritten by a
/// later block's agreement.
#[test]
fn each_dropped_block_is_one_comparison_of_its_compaction() {
    use serde_json::json;
    let seat = &crate::jev::COMPACTION;
    let asked = json!({"at": 10, "judged": 77, "outcome": "answered", "requests": 1, "elapsedMs": 1,
                       "rubricVersion": seat.rubric_version, "model": "M"});
    let block = |at: u64, block: u64, agreed: bool| json!({"kind": "label", "at": at, "label": "77", "block": block, "agreed": agreed, "applied": false});
    let rows = [
        asked,
        block(20, 1, false),
        block(30, 2, true),
        block(30, 3, false),
        block(31, 3, true),
    ];
    let judged = judge_seat(seat, &rows).expect("compaction rises");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (3, 2),
        "block 1's regret, block 2, and block 3 by its newest label: {judged:?}"
    );
}

/// The text reader and the row reader agree on what fences a series
/// (t-6877 round 2, astra R2): a request of newer words fences the seat
/// behind it, and nothing else does — not a label spelling a newer version,
/// not a request spelling a fraction, a word or a null, not a line torn
/// before its value ends — however the key and its colon are spaced, and
/// wherever a key of the same spelling sits inside a value. Read off the
/// rows and off the text, each line kind, on a seat asking version 1 and
/// one asking version 2.
#[test]
fn the_text_reader_and_the_row_reader_agree_on_what_fences_a_series() {
    use serde_json::json;
    let first = &crate::jev::PLACEMENT;
    let second = guard();
    // (a line as a writer or a hand spelled it, whether it fences a seat
    // asking version 1, whether it fences one asking version 2)
    let lines: [(&str, bool, bool); 10] = [
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":2}"#,
            true,
            false,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion" :2}"#,
            true,
            false,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion": 2 }"#,
            true,
            false,
        ),
        (
            r#"{"at":2,"outcome":"control","rubricVersion":3}"#,
            true,
            true,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":2.5}"#,
            false,
            false,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":null}"#,
            false,
            false,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":"3"}"#,
            false,
            false,
        ),
        (
            r#"{"at":2,"label":"orphan","rubricVersion":3,"agreed":true}"#,
            false,
            false,
        ),
        (
            r#"{"at":2,"outcome":"answered","asked":{"rubricVersion":3},"note":"\"rubricVersion\": 9"}"#,
            false,
            false,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":3"#,
            false,
            false,
        ),
    ];
    for (line, fences_first, fences_second) in lines {
        for (seat, fences) in [(first, fences_first), (second, fences_second)] {
            let text = format!(
                "{}\n{line}\n",
                json!({"at": 1, (TRANSITION.canonical): ROSE, "rubricVersions": [seat.rubric_version]})
            );
            let rows: Vec<Value> = text
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let expected = if fences {
                Stand::Recording
            } else {
                Stand::Applying
            };
            assert_eq!(
                standing(seat, &rows),
                expected,
                "{}: the rows, on {line}",
                seat.id
            );
            assert_eq!(
                standing_in(seat, &text),
                expected,
                "{}: the text, on {line}",
                seat.id
            );
        }
    }
}

/// The text reader reads every line the rows reader would read (t-6877
/// round 3, astra R2): a request of newer words fences the seat though a
/// value of it says `transition` or an object inside it carries a
/// transition's key; a key spelled with an escape is the key the parser
/// reads — `"rubric\u0056ersion"` is the rubric, `"\u0074ransition"` a
/// transition; and a row that is both asked and a transition is read as the
/// rows read it, fence first. Each ledger read off the rows and off the
/// text, on a seat asking version 1 and one asking version 2.
#[test]
fn the_text_reader_reads_every_line_the_rows_reader_would() {
    let first = &crate::jev::PLACEMENT;
    let second = guard();
    // (the ledger after the seat's own rise at its own words, as `S`, and
    // where the seat stands on it)
    let ledgers: [(&str, Stand); 10] = [
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":3,"note":"transition"}"#,
            Stand::Recording,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":3,"meta":{"transition":"rise"}}"#,
            Stand::Recording,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":3,"note":"a \"transition\" in a value"}"#,
            Stand::Recording,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubric\u0056ersion":3}"#,
            Stand::Recording,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubric\u0056ersion":1}"#,
            Stand::Applying,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":1,"note":"transition"}"#,
            Stand::Applying,
        ),
        (
            r#"{"at":2,"\u0074ransition":"fall","rubricVersions":[S]}"#,
            Stand::Recording,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":3}
{"at":3,"\u0074ransition":"rise","rubricVersions":[S]}"#,
            Stand::Applying,
        ),
        (
            r#"{"at":2,"transition":"rise","rubricVersions":[S],"outcome":"control","rubricVersion":3}"#,
            Stand::Recording,
        ),
        (
            r#"{"at":2,"outcome":"answered","rubricVersion":3,"note":"transi"#,
            Stand::Applying,
        ),
    ];
    // Every ledger a reader reads apart from where the seat stands, named
    // at once — not the first alone.
    let mut apart = Vec::new();
    for (after, stands) in ledgers {
        for seat in [first, second] {
            let own = seat.rubric_version.to_string();
            let text = format!(
                "{}\n{}\n",
                rise_on(1, &[seat.rubric_version]),
                after.replace('S', &own)
            );
            let rows: Vec<Value> = text
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let (off_rows, off_text) = (standing(seat, &rows), standing_in(seat, &text));
            if (off_rows, off_text) != (stands, stands) {
                apart.push(format!(
                    "{}: {after} — rows {off_rows:?}, text {off_text:?}, not {stands:?}",
                    seat.id
                ));
            }
        }
    }
    assert!(
        apart.is_empty(),
        "the readers read these ledgers apart:\n{}",
        apart.join("\n")
    );
}

/// A label that names no request grades nothing (t-6877 round 2, astra
/// R3): zo's step seat writes its progress mark naming the judgment it
/// grades — the turn's attempt and the step the judgment was asked at — and
/// a mark naming none, as every mark written before it named one, is not
/// read as the nearest judgment's. The reader guesses no request.
#[test]
fn a_label_that_names_no_request_grades_nothing() {
    use serde_json::json;
    let seat = &crate::jev::ZO_STEP_EFFORT;
    let judgment = json!({
        "kind": "judgment", "at": 10, "attempt": "s@1", "step": 3, "outcome": "answered",
        "elapsedMs": 5, "requests": 1, "model": ANSWERING,
    });
    let unnamed = json!({"kind": "label", "at": 11, "attempt": "s@1", "step": 4, "agreed": true});
    let judged = judge_seat(seat, &[judgment.clone(), unnamed]).expect("judged");
    assert_eq!(
        judged.agreement.compared, 0,
        "a mark naming no judgment grades none: {judged:?}"
    );
    let named = json!({"kind": "label", "at": 11, "label": "s@1:3", "attempt": "s@1", "step": 4, "agreed": true});
    let judged = judge_seat(seat, &[judgment, named]).expect("judged");
    assert_eq!(
        (judged.agreement.compared, judged.agreement.agreed),
        (1, 1),
        "a mark naming its judgment is its comparison: {judged:?}"
    );
}

/// The skills seat is one question (t-6877 round 2, astra R3): the search's
/// row asks the search's words alone, and a suggestion's request — the
/// other rubric, once written into the same ledger — is not one of its
/// requests and fences nothing of the search's own asked since. A ledger
/// the two questions were written into before they were two seats reads as
/// the search's series from the last suggestion row on.
#[test]
fn the_skills_seat_asks_one_question_and_a_mixed_ledger_reads_as_its_own_series() {
    use serde_json::json;
    let seat = &crate::jev::SKILLS;
    assert_eq!(
        seat.rubric_version,
        crate::jev::questions::SKILL_SEARCH_RUBRIC_VERSION,
        "the search asks the search's words"
    );
    let asked = |at: usize, rubric: u32| json!({"at": at, "task": at, "catalog": 1, "rubricVersion": rubric, "outcome": "answered", "elapsedMs": 5, "requests": 1});
    let mut rows: Vec<Value> = (0..20)
        .map(|at| asked(at, crate::jev::questions::SKILL_SEARCH_RUBRIC_VERSION))
        .collect();
    rows.extend(
        (20..40).map(|at| asked(at, crate::jev::questions::SKILL_SUGGESTION_RUBRIC_VERSION)),
    );
    rows.extend((40..43).map(|at| asked(at, crate::jev::questions::SKILL_SEARCH_RUBRIC_VERSION)));
    let version = on_the_newest_version(seat, &rows);
    assert_eq!(
        version.asked(),
        3,
        "the search's requests since the last suggestion row, and none of the suggestion's"
    );
    let judged = judge_seat(seat, &rows).expect("judged");
    assert_eq!(judged.window.rows, 3, "{judged:?}");
}

/* ---- a window's marks reach back to hold the sample floor (t-9087) ---------- */

/// `asked` requests of `seat` asked from `at` on, then marks of the first
/// `marked` of them written after the last, each carrying what `fields` says
/// of the `k`th — every mark a label naming its own request and the time it
/// was asked, as the seat's writer files one ([`mark`], t-6877).
fn asked_then_marked(
    seat: &JevUse,
    at: usize,
    asked: usize,
    marked: usize,
    fields: impl Fn(usize) -> Value,
) -> Vec<Value> {
    asked_then_marked_as(seat, seat.rubric_version, None, at, asked, marked, fields)
}

/// [`asked_then_marked`] asked under `rubric` and answered by `model`, when
/// one is named — a label inherits both from its request. For a seat whose
/// marks sit on the request row ([`mark`]), the first `marked` requests
/// carry them.
fn asked_then_marked_as(
    seat: &JevUse,
    rubric: u32,
    model: Option<&str>,
    at: usize,
    asked: usize,
    marked: usize,
    fields: impl Fn(usize) -> Value,
) -> Vec<Value> {
    let ask = |n: usize| {
        let mut row = asked_by(seat, n);
        row[RUBRIC_VERSION.canonical] = json!(rubric);
        if let Some(model) = model {
            row[MODEL.canonical] = json!(model);
        }
        row
    };
    if seat.request_name.is_empty() {
        return (0..asked)
            .map(|k| {
                let mut row = ask(at + k);
                if k < marked {
                    for (key, value) in fields(k).as_object().expect("the mark's fields") {
                        row[key.as_str()] = value.clone();
                    }
                }
                row
            })
            .collect();
    }
    let mut rows: Vec<Value> = (at..at + asked).map(ask).collect();
    rows.extend((0..marked).map(|k| mark(seat, at + asked + k, at + k, fields(k))));
    rows
}

/// A seat whose marks are sparser than its requests is judged on its marks
/// (t-9087). The notify seat marks a ring only when the person was at the
/// window or turned to it — 110 of the 444 rings this machine's ledger held
/// on 2026-09-25 — so the 53 rings its answer floor is read on held 15 marks,
/// and the seat sat at `too_few_compared` with 110 in hand; the placement
/// seat's 25 held 9 of 87. The window's marks reach back from its first
/// request to hold the sample floor, and no further: a window that already
/// holds the floor reads its own marks alone.
#[test]
fn a_seat_whose_marks_are_sparser_than_its_requests_is_judged_on_its_marks() {
    for seat in [&crate::jev::NOTIFY, &crate::jev::PLACEMENT] {
        let wanted = window_wanted_for(seat).expect("a promoting seat");
        let floor = seat.agreement_rows_wanted.expect("a label sample floor");
        let misses = seat.negatives_wanted.expect("a promoting seat");
        let older = marks_that_can_clear(seat).expect("a width the line can be cleared on");
        let inside = 3;
        // Rings asked before the window, each marked before it began — the
        // three that say no the oldest...
        let mut rows = asked_then_marked(
            seat,
            0,
            older,
            older,
            |k| json!({"agreed": k >= misses, "baselineAgreed": k % 2 == 0}),
        );
        // ...then the window, and the few marks its own rings earned.
        let start = rows.len();
        rows.extend(asked_then_marked(
            seat,
            start,
            wanted,
            inside,
            |_| json!({"agreed": true, "baselineAgreed": false}),
        ));
        let judged = judge_seat(seat, &rows).expect("judged");
        assert_eq!(
            judged.agreement.compared, floor,
            "{}: the window reaches back to the newest {floor} marks, and no further",
            seat.id
        );
        assert_eq!(judged.agreement.agreed, floor, "{}", seat.id);
        assert_eq!(judged.verdict, Verdict::Rise, "{}: {judged:?}", seat.id);

        // A window that holds the floor on its own reads only its own marks:
        // the older ones that said no are left where they are.
        let mut full = asked_then_marked(
            seat,
            0,
            older,
            older,
            |_| json!({"agreed": false, "baselineAgreed": false}),
        );
        let start = full.len();
        full.extend(asked_then_marked(
            seat,
            start,
            wanted,
            floor,
            |_| json!({"agreed": true, "baselineAgreed": false}),
        ));
        let judged = judge_seat(seat, &full).expect("judged");
        assert_eq!(
            (judged.agreement.compared, judged.agreement.agreed),
            (floor, floor),
            "{}",
            seat.id
        );

        // A record holding fewer marks than the floor is read whole, and
        // says how few.
        let mut thin = asked_then_marked(
            seat,
            0,
            older,
            floor - inside - 1,
            |k| json!({"agreed": k >= misses}),
        );
        let start = thin.len();
        thin.extend(asked_then_marked(
            seat,
            start,
            wanted,
            inside,
            |_| json!({"agreed": true}),
        ));
        assert_eq!(
            judge_seat(seat, &thin).expect("judged").verdict,
            Verdict::Hold(Line::TooFewCompared {
                compared: floor - 1,
                wanted: floor
            }),
            "{}",
            seat.id
        );
    }
}

/// The words r1 of t-9087 moved are judged on their own series however far
/// a window's marks reach back (t-9087 r2, over t-6877): the command guard's
/// version 2, the stall seat's 4 and the summons' 5 each ask the words
/// before them graded by another label, and the label's version rides the
/// request. A window of the new words holding fewer marks than the sample
/// floor reaches back through its own series and stops there — short of the
/// older words' thick record and the rise it earned, of a late label of an
/// older request, of the requests another version answered, and of the
/// words a rollback left behind — and inside its series it still reaches
/// its floor.
#[test]
fn a_moved_rubrics_window_reaches_back_through_its_own_series_alone() {
    const OLDER_VERSION: &str = "jev-1.12.0";
    for seat in [
        &crate::jev::COMMAND_GUARD,
        &crate::jev::STALL,
        &crate::jev::SUMMON,
    ] {
        let today = seat.rubric_version;
        let before = today - 1;
        let wanted = window_wanted_for(seat).expect("a promoting seat");
        let floor = seat.agreement_rows_wanted.expect("a label sample floor");
        let misses = seat.negatives_wanted.expect("a promoting seat");
        let thick = marks_that_can_clear(seat)
            .expect("a width the line can be cleared on")
            .max(wanted);
        let inside = 3;
        let rising =
            |k: usize| json!({"agreed": k >= misses, "baselineAgreed": k.is_multiple_of(2)});
        let yes = |_: usize| json!({"agreed": true, "baselineAgreed": false});
        let no = |_: usize| json!({"agreed": false, "baselineAgreed": true});

        // The older words' thick record and its rise, then a few requests of
        // today's: today's marks alone, a window not yet full, recording.
        let mut rows = asked_then_marked_as(seat, before, None, 0, thick, thick, rising);
        rows.push(rise_on(rows.len(), &[before]));
        let start = rows.len();
        rows.extend(asked_then_marked_as(
            seat,
            today,
            None,
            start,
            inside + 2,
            inside,
            yes,
        ));
        let judged = judge_seat(seat, &rows).expect("judged");
        assert_eq!(
            judged.agreement.compared, inside,
            "{}: today's marks alone",
            seat.id
        );
        assert!(
            matches!(judged.verdict, Verdict::Hold(Line::TooFewRows { .. })),
            "{}: {judged:?}",
            seat.id
        );
        assert_eq!(standing(seat, &rows), Stand::Recording, "{}", seat.id);

        // Today's words marked before their window, then the window and its
        // few marks: the reach back reads today's newest marks to the floor.
        let mut rows = asked_then_marked_as(seat, before, None, 0, thick, 0, no);
        let start = rows.len();
        rows.extend(asked_then_marked_as(
            seat,
            today,
            None,
            start,
            thick - inside,
            thick - inside,
            rising,
        ));
        let start = rows.len();
        rows.extend(asked_then_marked_as(
            seat, today, None, start, wanted, inside, yes,
        ));
        let clean = judge_seat(seat, &rows).expect("judged");
        assert_eq!(
            (clean.agreement.compared, clean.agreement.agreed),
            (floor, floor),
            "{}: a sparse window of today's words reads its floor in today's series",
            seat.id
        );
        // The older words' requests graded after the window began — a late
        // label of each, or, for a seat whose marks sit on its requests, an
        // older binary's marked requests beside the newer — change nothing:
        // not the marks, and not how far back they reach.
        let late = rows.len();
        if seat.request_name.is_empty() {
            rows.extend(asked_then_marked_as(
                seat, before, None, late, floor, floor, no,
            ));
        } else {
            rows.extend((0..floor).map(|k| mark(seat, late + k, k, no(k))));
        }
        assert_eq!(
            judge_seat(seat, &rows).expect("judged"),
            clean,
            "{}: the older words' late marks",
            seat.id
        );

        // Today's words answered by an older version, then by the version
        // answering now: the reach back stops where the version changed.
        let mut rows =
            asked_then_marked_as(seat, today, Some(OLDER_VERSION), 0, thick, thick, rising);
        let start = rows.len();
        rows.extend(asked_then_marked_as(
            seat,
            today,
            Some(ANSWERING),
            start,
            wanted,
            inside,
            yes,
        ));
        let judged = judge_seat(seat, &rows).expect("judged");
        assert_eq!(
            judged.agreement.compared, inside,
            "{}: the version answering now",
            seat.id
        );
        assert_eq!(judged.cut.as_deref(), Some(OLDER_VERSION), "{}", seat.id);
        assert!(
            matches!(
                judged.verdict,
                Verdict::Hold(Line::TooFewCompared { compared, wanted: asked_for }) if compared == inside && asked_for == floor
            ),
            "{}: {judged:?}",
            seat.id
        );

        // Words that went back: today's thick record, one request of newer
        // words, then today's window again — the series starts after the
        // newer words, and the reach back stops there.
        let mut rows = asked_then_marked_as(seat, today, None, 0, thick, thick, rising);
        let fence = rows.len();
        rows.extend(asked_then_marked_as(
            seat,
            today + 1,
            None,
            fence,
            1,
            0,
            yes,
        ));
        let start = rows.len();
        rows.extend(asked_then_marked_as(
            seat, today, None, start, wanted, inside, yes,
        ));
        let judged = judge_seat(seat, &rows).expect("judged");
        assert_eq!(
            judged.agreement.compared, inside,
            "{}: the rollback's own series",
            seat.id
        );
    }
}
