use serde_json::json;

use super::*;
use crate::jev::promote::Line;
use crate::jev::{NOTIFY, PLACEMENT, RECALL, ROUTING, STALL, SUMMON};

/// Graded answers laid out cell by cell: `(confidence, marks, agreed,
/// baseline agreed)` — every mark with a baseline mark beside it.
fn cells(cells: &[(f64, usize, usize, usize)]) -> Vec<Graded> {
    let mut graded = Vec::new();
    for (confidence, marks, agreed, baseline) in cells {
        for k in 0..*marks {
            graded.push(Graded {
                confidence: Some(*confidence),
                agreed: k < *agreed,
                baseline: Some(k < *baseline),
            });
        }
    }
    graded
}

/// Every graded answer's confidence, as the answered requests' — each
/// request graded once.
fn answered_as(graded: &[Graded]) -> Vec<Option<f64>> {
    graded.iter().map(|one| one.confidence).collect()
}

fn line_of(seat: &JevUse, graded: &[Graded]) -> Result<u16, NoLine> {
    calibrate(seat, graded, &answered_as(graded))
        .expect("a seat that rises and names bands")
        .line
}

/// The notify seat as t-9087 r1 read it (73 marks of the current label):
/// its share climbs to 84.3% from 0.6 and falls to 4 of 11 from 0.8 — the
/// most confident answers are its worst. Confidence runs against being
/// right, and no line is drawn however a line's bound looks.
#[test]
fn a_seat_whose_confidence_runs_against_being_right_gets_no_line() {
    let graded = cells(&[
        (0.1, 4, 2, 1),
        (0.4, 11, 8, 1),
        (0.55, 7, 5, 1),
        (0.65, 23, 22, 1),
        (0.75, 17, 17, 0),
        (0.82, 2, 2, 0),
        (0.87, 2, 0, 0),
        (0.92, 5, 1, 5),
        (0.97, 2, 1, 1),
    ]);
    let calibrated = calibrate(&NOTIFY, &graded, &answered_as(&graded)).expect("notify rises");
    // The r1 table, line by line.
    let bounds: Vec<Option<u16>> = calibrated
        .grid
        .iter()
        .map(AtLine::lower_bound_permille)
        .collect();
    assert_eq!(
        bounds,
        [688, 703, 710, 719, 566, 151, 63, 82, 94]
            .map(Some)
            .to_vec()
    );
    assert_eq!(calibrated.line, Err(NoLine::NonMonotone));
    assert_eq!(NoLine::NonMonotone.token(), "non_monotone");
}

/// A seat whose line would pass on its own numbers — 101 of 112 right from
/// 0.3, bound at 832‰ against a baseline right on three in ten — gets none
/// when its most confident dozen are right twice: a line is read on what
/// the seat's confidence says, and here it says the opposite of being
/// right.
#[test]
fn a_line_that_would_pass_is_withheld_when_confidence_runs_against_being_right() {
    let graded = cells(&[(0.2, 40, 10, 12), (0.6, 100, 99, 30), (0.97, 12, 2, 4)]);
    let calibrated = calibrate(&NOTIFY, &graded, &answered_as(&graded)).expect("notify rises");
    assert_eq!(
        calibrated.at(300).and_then(AtLine::lower_bound_permille),
        Some(832)
    );
    assert_eq!(calibrated.line, Err(NoLine::NonMonotone));
}

/// The stall seat as r1 read it (91 marks of the four-hour label): every
/// answer at 0.7 or above, 93.4% of them right (lower bound 863‰) against
/// the always-long-running baseline's 60.4%. Taken whole it already passes
/// what the judge asks — no line is wanted, and a line would only shed
/// answers the seat gets right.
#[test]
fn a_seat_that_passes_whole_wants_no_line() {
    let graded = cells(&[
        (0.75, 2, 2, 1),
        (0.82, 6, 6, 4),
        (0.87, 9, 9, 5),
        (0.92, 16, 14, 9),
        (0.97, 58, 54, 36),
    ]);
    let calibrated = calibrate(&STALL, &graded, &answered_as(&graded)).expect("stall rises");
    assert_eq!(calibrated.grid[0].lower_bound_permille(), Some(863));
    assert_eq!(calibrated.line, Err(NoLine::Whole));
}

/// A seat all of whose answers sit in one stretch of confidence cannot be
/// split by any line: each line holds all of them or none.
#[test]
fn a_seat_whose_answers_are_all_one_colour_gets_no_line() {
    let graded = cells(&[(0.97, 60, 40, 30)]);
    assert_eq!(line_of(&STALL, &graded), Err(NoLine::OneColour));
    let graded = cells(&[(0.1, 60, 40, 30)]);
    assert_eq!(line_of(&STALL, &graded), Err(NoLine::OneColour));
}

/// The summons as r1 read it (121 marks of rubric 4): right on 119, and its
/// baseline — the pinned model's own CLI — right on all 121. The label
/// said no twice, so no line of it can show a label that can say no.
#[test]
fn a_seat_whose_label_never_says_no_on_a_line_gets_no_line() {
    let graded = cells(&[
        (0.2, 12, 11, 12),
        (0.4, 29, 28, 29),
        (0.55, 11, 11, 11),
        (0.65, 17, 17, 17),
        (0.75, 14, 14, 14),
        (0.82, 12, 12, 12),
        (0.87, 9, 9, 9),
        (0.92, 11, 11, 11),
        (0.97, 6, 6, 6),
    ]);
    assert_eq!(
        line_of(&SUMMON, &graded),
        Err(NoLine::Line(Line::OneSided {
            disagreed: 1,
            wanted: crate::jev::NEGATIVES_WANTED
        }))
    );
}

/// Placement as r1 read it (44 marks): 88.2% right from 0.7 — on
/// seventeen marks, bound at 656‰ — and every line holding enough marks
/// to speak bounds far under the floor.
#[test]
fn a_seat_whose_confident_part_is_too_thin_gets_no_line() {
    let graded = cells(&[
        (0.2, 2, 0, 2),
        (0.4, 12, 0, 12),
        (0.55, 7, 3, 7),
        (0.65, 6, 4, 5),
        (0.75, 11, 10, 11),
        (0.82, 2, 2, 2),
        (0.87, 3, 3, 3),
        (0.92, 1, 0, 1),
    ]);
    let calibrated =
        calibrate(&PLACEMENT, &graded, &answered_as(&graded)).expect("placement rises");
    assert_eq!(
        calibrated
            .at(700)
            .map(|at| (at.marks, at.lower_bound_permille())),
        Some((17, Some(656)))
    );
    assert_eq!(
        calibrated.line,
        Err(NoLine::Line(Line::Agreement {
            bound_permille: 377,
            floor_permille: crate::jev::ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE
        }))
    );
}

/// A seat whose confident answers are right and its others are not gets the
/// lowest line its confident answers pass on: fifty answers from 0.9, three
/// of them wrong, bound at 837‰ against a baseline right on three in ten,
/// and half the answered requests acted on.
#[test]
fn a_seat_whose_confidence_separates_gets_the_lowest_line_that_passes() {
    let graded = cells(&[(0.2, 30, 12, 9), (0.6, 20, 12, 6), (0.9, 50, 47, 15)]);
    let calibrated = calibrate(&NOTIFY, &graded, &answered_as(&graded)).expect("notify rises");
    assert_eq!(calibrated.line, Ok(700));
    let at = calibrated.at(700).expect("the line");
    assert_eq!(
        (
            at.marks,
            at.lower_bound_permille(),
            at.apply_share(),
            at.error_permille(),
            at.baseline_error_permille(),
            at.under_error_permille()
        ),
        (50, Some(837), Some(0.5), Some(60), Some(700), Some(520))
    );
    assert_eq!(calibrated.marks_wanted, 40);
}

/// A seat whose answers are right less often than its baseline on the same
/// marks acts on none of them: every line that reaches the floor loses to
/// the cheapest reader.
#[test]
fn a_seat_worse_than_its_baseline_gets_no_line() {
    let graded = cells(&[(0.2, 40, 20, 38), (0.9, 60, 57, 60)]);
    assert_eq!(
        line_of(&NOTIFY, &graded),
        Err(NoLine::Line(Line::Baseline {
            bound_permille: 862,
            baseline_permille: 1_000
        }))
    );
}

/// A line that clears the floor by shedding a tail — 793‰ taken whole,
/// 825‰ from 0.3 — lifts the bound too little to say confidence separates
/// anything.
#[test]
fn a_line_that_lifts_the_bound_too_little_is_no_line() {
    let graded = cells(&[(0.2, 50, 37, 25), (0.6, 150, 133, 75)]);
    assert_eq!(
        line_of(&NOTIFY, &graded),
        Err(NoLine::NoLift {
            lift_permille: 32,
            wanted_permille: CALIBRATION.lift_permille
        })
    );
}

/// A line whose answers pass but act on too small a share of what the seat
/// answers is no line to rise on: five hundred more answers under it.
#[test]
fn a_line_that_acts_on_too_little_is_no_line() {
    let graded = cells(&[(0.2, 30, 12, 9), (0.6, 20, 12, 6), (0.9, 50, 47, 15)]);
    let mut answered = answered_as(&graded);
    answered.extend(std::iter::repeat_n(Some(0.1), 500));
    answered.extend(std::iter::repeat_n(None, 10));
    let calibrated = calibrate(&NOTIFY, &graded, &answered).expect("notify rises");
    assert_eq!(
        calibrated.line,
        Err(NoLine::Line(Line::ApplyShare {
            share_permille: 81,
            floor_permille: CALIBRATION.apply_share_floor_permille
        }))
    );
    assert_eq!(
        calibrated.at(700).map(|at| (at.acted, at.answered)),
        Some((50, 610))
    );
}

/// Graded answers that carry no confidence leave nothing to read a line
/// off; a seat with none graded yet has too few marks. A seat that never
/// rises, or names no bands, is not calibrated at all.
#[test]
fn a_seat_with_no_confident_answers_or_no_lines_is_not_read() {
    let unconfident: Vec<Graded> = cells(&[(0.9, 50, 45, 10)])
        .into_iter()
        .map(|one| Graded {
            confidence: None,
            ..one
        })
        .collect();
    assert_eq!(line_of(&NOTIFY, &unconfident), Err(NoLine::NoConfidence));
    assert_eq!(
        line_of(&NOTIFY, &[]),
        Err(NoLine::Line(Line::TooFewCompared {
            compared: 0,
            wanted: 40
        }))
    );
    assert_eq!(calibrate(&crate::jev::AGENT_TOOL, &[], &[]), None);
    for seat in crate::jev::JEV_USES.iter().filter(|seat| !seat.promotes) {
        assert_eq!(calibrate(seat, &[], &[]), None, "{}", seat.id);
    }
}

/// The table beside a ledger is what the product reads, row by row: a row
/// counts for the words its seat asks now, only for a seat whose stage
/// reads a line, and only alone; anything that is not a table is none.
#[test]
fn the_table_is_read_only_for_the_words_asked_now_and_a_stage_that_reads_a_line() {
    let row = |seat: &str, rubric: u32, line: Value| json!({ SEAT_KEY: seat, "rubricVersion": rubric, COMPUTED_AT_KEY: 1, ACT_FROM_KEY: line });
    let table = Thresholds::parse(
        &json!([
            row(NOTIFY.id, NOTIFY.rubric_version, json!(700)),
            row(PLACEMENT.id, PLACEMENT.rubric_version + 1, json!(600)),
            row(STALL.id, STALL.rubric_version, Value::Null),
            row(SUMMON.id, SUMMON.rubric_version, json!(800)),
            row(SUMMON.id, SUMMON.rubric_version, json!(850)),
            row(RECALL.id, RECALL.rubric_version, json!(500)),
            row(ROUTING.id, ROUTING.rubric_version, json!(1_001)),
            json!("not a row"),
        ])
        .to_string(),
    );
    assert_eq!(table.line_of(&NOTIFY), Some(700));
    assert_eq!(table.line_of(&PLACEMENT), None, "another rubric's line");
    assert_eq!(table.line_of(&STALL), None, "a row that drew no line");
    assert_eq!(
        table.line_of(&SUMMON),
        None,
        "two rows for one seat's words"
    );
    const { assert!(!RECALL.reads_act_line) };
    assert_eq!(table.line_of(&RECALL), None, "a stage that reads no line");
    assert_eq!(
        table.line_of(&ROUTING),
        None,
        "a line that is not a per-thousand"
    );
    for text in ["", "{}", "[1, 2]", "not json", "{\"seat\": \"notify\"}"] {
        assert_eq!(Thresholds::parse(text), Thresholds::default(), "{text}");
    }
    let dir = std::env::temp_dir().join(format!("jev-threshold-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    assert_eq!(Thresholds::read_in(&dir), Thresholds::default(), "no file");
    std::fs::write(
        dir.join(THRESHOLDS_FILE),
        json!([row(NOTIFY.id, NOTIFY.rubric_version, json!(650))]).to_string(),
    )
    .expect("the table");
    assert_eq!(Thresholds::read_in(&dir).line_of(&NOTIFY), Some(650));
    assert_eq!(
        line_beside(&NOTIFY, &dir.join(NOTIFY.ledger)),
        Some(650),
        "the file beside the ledger"
    );
    assert_eq!(line_beside(&NOTIFY, Path::new("")), None);
    std::fs::remove_dir_all(&dir).expect("cleaned");
}

/// A row written for a calibration is the row the product reads back: the
/// line, the rubric it was read under, and the numbers it stands on.
#[test]
fn a_row_the_replay_writes_reads_back_as_the_line_it_drew() {
    let graded = cells(&[(0.2, 30, 12, 9), (0.6, 20, 12, 6), (0.9, 50, 47, 15)]);
    let calibrated = calibrate(&NOTIFY, &graded, &answered_as(&graded)).expect("notify rises");
    let row = row(&NOTIFY, &calibrated, 1_790_000_000_000);
    assert_eq!(row[SEAT_KEY], NOTIFY.id);
    assert_eq!(row["rubricVersion"], NOTIFY.rubric_version);
    assert_eq!(row[COMPUTED_AT_KEY], 1_790_000_000_000_i64);
    assert_eq!(
        (row[MARKS_KEY].clone(), row[LOWER_BOUND_KEY].clone()),
        (json!(50), json!(837))
    );
    assert_eq!(
        (row[ACT_FROM_KEY].clone(), row[APPLY_SHARE_KEY].clone()),
        (json!(700), json!(0.5))
    );
    assert_eq!(row[REASON_KEY], Value::Null);
    assert_eq!(
        row[GRID_KEY].as_array().map(Vec::len),
        Some(CALIBRATION.grid.len())
    );
    let table = Thresholds::parse(&json!([row]).to_string());
    assert_eq!(table.line_of(&NOTIFY), Some(700));

    let none = calibrate(&NOTIFY, &cells(&[(0.97, 60, 40, 30)]), &[]).expect("notify rises");
    let row = super::row(&NOTIFY, &none, 1);
    assert_eq!(
        (row[ACT_FROM_KEY].clone(), row[REASON_KEY].clone()),
        (Value::Null, json!("one_colour"))
    );
    assert_eq!(row[APPLY_SHARE_KEY], Value::Null);
    assert_eq!(
        Thresholds::parse(&json!([row]).to_string()).line_of(&NOTIFY),
        None
    );
}
