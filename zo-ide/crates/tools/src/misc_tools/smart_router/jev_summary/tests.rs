use std::fs;

use serde_json::json;

use super::*;

fn write(dir: &std::path::Path, name: &str, rows: &[Value]) {
    fs::create_dir_all(dir).expect("dir");
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    fs::write(dir.join(name), text).expect("write");
}

#[test]
fn a_line_that_does_not_parse_does_not_take_the_ledger_with_it() {
    let home = tempfile::tempdir().expect("tmp");
    let path = home.path().join("ledger.jsonl");
    fs::write(&path, "{\"at\":1,\"outcome\":\"answered\"}\n{half\n{\"at\":2,\"outcome\":\"answered\"}\n")
        .expect("write");
    assert_eq!(read_rows(&path).len(), 2, "a crash's half line is not a refusal");
}

#[test]
fn the_local_day_starts_where_the_person_is_and_a_millisecond_decides_the_side() {
    let offset = 9 * 3_600; // Seoul
    let midnight = start_of_day_ms(1_789_700_000_000, offset);
    assert_eq!(start_of_day_ms(midnight, offset), midnight, "midnight belongs to its own day");
    assert_eq!(start_of_day_ms(midnight - 1, offset), midnight - 24 * 60 * 60 * 1000);
    assert_eq!(start_of_day_ms(midnight + 1, offset), midnight);
    assert_ne!(
        start_of_day_ms(midnight, 0),
        midnight,
        "a UTC reading of the same instant names a different day boundary"
    );
}

#[test]
fn a_bound_is_compared_in_the_lines_own_units() {
    assert!(clears(0.95, 950));
    assert!(clears(0.9509, 950));
    assert!(!clears(0.9499, 950));
    assert!(clears(1.0, 1000));
}

#[test]
fn every_seat_in_the_table_gets_a_row_whether_or_not_it_has_a_ledger() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf(), home.path().join("other")];
    write(
        home.path(),
        zerocode_core::jev::ROUTING.ledger,
        &[json!({"at": 10, "outcome": "answered", "elapsedMs": 40, "requests": 1})],
    );
    let seats: Vec<SeatReport> = zerocode_core::jev::JEV_USES
        .iter()
        .map(|seat| super::one(seat, &roots, None, 1_000, 0))
        .collect();
    assert_eq!(seats.len(), zerocode_core::jev::JEV_USES.len());
    let routing = seats.iter().find(|seat| seat.id == "routing").expect("routing");
    assert_eq!(routing.week.rows, 1);
    assert!(routing.found.is_some());
    assert_eq!(routing.rise_floor_permille, zerocode_core::jev::ROUTING.answer_floor_permille);
    let unused: Vec<&SeatReport> = seats.iter().filter(|seat| seat.found.is_none()).collect();
    assert!(!unused.is_empty(), "the table has seats no ledger has been written for");
    for seat in unused {
        assert_eq!(seat.week.rows, 0);
        assert_eq!(seat.week.answered_share(), None, "never asked is not zero percent");
        assert_eq!(seat.clears_rise_floor, None);
    }
}

#[test]
fn a_seat_that_never_rises_is_never_asked_to_clear_a_line() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf(), home.path().join("other")];
    let answered = [json!({"at": 10, "outcome": "answered", "elapsedMs": 1})];
    for seat in zerocode_core::jev::JEV_USES.iter() {
        write(home.path(), seat.ledger, &answered);
        let row = super::one(seat, &roots, None, 1_000, 0);
        assert_eq!(
            row.clears_rise_floor.is_some(),
            seat.promotes,
            "{} promotes={}",
            seat.id,
            seat.promotes
        );
        assert_eq!(row.rows_to_next_judgment().is_some(), seat.promotes);
    }
}

#[test]
fn a_seat_that_can_rise_has_a_stage_to_time_it() {
    // A rise line with no deadline is a promotion nobody can fail on latency.
    for seat in zerocode_core::jev::JEV_USES.iter() {
        assert_eq!(
            deadline_ms_for(seat).is_some(),
            seat.promotes,
            "{} promotes={} deadline={:?}",
            seat.id,
            seat.promotes,
            deadline_ms_for(seat)
        );
    }
}

#[test]
fn a_seat_starts_recording_and_a_rise_row_in_its_own_ledger_makes_it_act() {
    use zerocode_core::jev::promote::{ROSE, Stand, TRANSITION};
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::ROUTING;
    let auto = json!({ "smart": { seat.setting: "auto" } });

    write(home.path(), seat.ledger, &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5})]);
    let quiet = super::one(seat, &roots, Some(&auto), 1_000, 0);
    assert_eq!(quiet.stand, Stand::Recording);
    assert!(!quiet.applies, "auto starts recording");

    write(
        home.path(),
        seat.ledger,
        &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5}), json!({"at": 2, TRANSITION: ROSE})],
    );
    let raised = super::one(seat, &roots, Some(&auto), 1_000, 0);
    assert_eq!(raised.stand, Stand::Applying);
    assert!(raised.applies, "a rise its own ledger recorded makes auto act");
}

#[test]
fn a_thin_window_holds_and_says_which_line_it_is_short_of() {
    use zerocode_core::jev::promote::{Line, Verdict};
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::ROUTING;
    write(home.path(), seat.ledger, &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5})]);
    let row = super::one(seat, &roots, None, 1_000, 0);
    assert!(matches!(row.verdict, Some(Verdict::Hold(Line::TooFewRows { rows: 1, .. }))));

    // And a seat with no rise line is never judged at all.
    let quiet = super::one(&zerocode_core::jev::STALL, &roots, None, 1_000, 0);
    assert_eq!(quiet.verdict, None);
}

#[test]
fn the_routing_seat_reads_its_standing_from_the_ledger_it_writes() {
    use zerocode_core::jev::promote::{FELL, ROSE, TRANSITION};
    let work = tempfile::tempdir().expect("tmp");
    let ledger = work.path().join(zerocode_core::jev::ROUTING.ledger);

    assert!(
        !super::super::decision_shadow::raised_at(&ledger),
        "a seat with no ledger has never risen"
    );
    fs::write(&ledger, format!("{}\n", json!({"at": 1, TRANSITION: ROSE}))).expect("write");
    assert!(super::super::decision_shadow::raised_at(&ledger));
    fs::write(
        &ledger,
        format!("{}\n{}\n", json!({"at": 1, TRANSITION: ROSE}), json!({"at": 2, TRANSITION: FELL})),
    )
    .expect("write");
    assert!(!super::super::decision_shadow::raised_at(&ledger), "a fall takes it back");
}
