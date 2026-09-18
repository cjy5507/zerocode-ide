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
    use zerocode_core::jev::promote::{ROSE, Stand};
    use zerocode_core::jev::summary::TRANSITION;
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
        &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5}), json!({"at": 2, (TRANSITION.canonical): ROSE})],
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
    use zerocode_core::jev::promote::{FELL, ROSE};
    use zerocode_core::jev::summary::TRANSITION;
    let work = tempfile::tempdir().expect("tmp");
    let ledger = work.path().join(zerocode_core::jev::ROUTING.ledger);

    assert!(
        !super::super::decision_shadow::raised_at(&ledger),
        "a seat with no ledger has never risen"
    );
    fs::write(&ledger, format!("{}\n", json!({"at": 1, (TRANSITION.canonical): ROSE}))).expect("write");
    assert!(super::super::decision_shadow::raised_at(&ledger));
    fs::write(
        &ledger,
        format!("{}\n{}\n", json!({"at": 1, (TRANSITION.canonical): ROSE}), json!({"at": 2, (TRANSITION.canonical): FELL})),
    )
    .expect("write");
    assert!(!super::super::decision_shadow::raised_at(&ledger), "a fall takes it back");
}

/// A ledger of `n` requests under a working directory the state root owns.
fn ledger_with(home: &std::path::Path, rows: &[Value]) -> std::path::PathBuf {
    let ledger = home.join(zerocode_core::jev::ROUTING.ledger);
    fs::create_dir_all(home).expect("dir");
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    fs::write(&ledger, text).expect("write");
    ledger
}

fn answered(at: i64) -> Value {
    json!({"at": at, "outcome": "answered", "elapsedMs": 400, "requests": 1})
}

#[test]
fn the_lines_are_judged_once_a_window_and_not_at_the_end_of_every_turn() {
    use zerocode_core::jev::summary::JUDGED_EVERY_ROWS;
    let work = tempfile::tempdir().expect("tmp");
    let short: Vec<Value> = (0..JUDGED_EVERY_ROWS as i64 - 1).map(answered).collect();
    let ledger = ledger_with(work.path(), &short);
    assert_eq!(
        super::super::decision_shadow::judge_ledger(&ledger, None, 9),
        None,
        "a window one row short was judged anyway"
    );
    let full: Vec<Value> = (0..JUDGED_EVERY_ROWS as i64).map(answered).collect();
    let ledger = ledger_with(work.path(), &full);
    assert!(
        super::super::decision_shadow::judge_ledger(&ledger, None, 9).is_some(),
        "a full window was not judged"
    );
}

#[test]
fn a_verdict_that_changed_nothing_writes_nothing_down() {
    use zerocode_core::jev::summary::JUDGED_EVERY_ROWS;
    let work = tempfile::tempdir().expect("tmp");
    let full: Vec<Value> = (0..JUDGED_EVERY_ROWS as i64).map(answered).collect();
    let ledger = ledger_with(work.path(), &full);
    let before = fs::read_to_string(&ledger).expect("read");
    // Clean rows, but nobody has labelled anything: §4 holds, and a hold is
    // not news.
    assert!(matches!(
        super::super::decision_shadow::judge_ledger(&ledger, None, 9),
        Some(zerocode_core::jev::promote::Verdict::Hold(_))
    ));
    assert_eq!(fs::read_to_string(&ledger).expect("read"), before, "a hold was written down");
}

#[test]
fn three_fallbacks_in_a_row_take_an_acting_seat_back_without_waiting_for_a_window() {
    use zerocode_core::jev::promote::{FELL, FALLBACKS_THAT_END_IT, ROSE, Verdict};
    use zerocode_core::jev::summary::TRANSITION;
    let work = tempfile::tempdir().expect("tmp");
    let mut rows: Vec<Value> = vec![json!({"at": 1, (TRANSITION.canonical): ROSE})];
    rows.extend((0..FALLBACKS_THAT_END_IT as i64).map(|at| {
        json!({"at": 10 + at, "outcome": "timeout", "elapsedMs": 1_500, "requests": 1})
    }));
    let ledger = ledger_with(work.path(), &rows);
    // Three requests, not twenty: the rule that ends it does not wait.
    let verdict = super::super::decision_shadow::judge_ledger(&ledger, None, 99);
    assert!(
        matches!(verdict, Some(Verdict::Fall(_))),
        "an acting seat kept acting through three fallbacks: {verdict:?}"
    );
    let written = fs::read_to_string(&ledger).expect("read");
    assert!(written.contains(FELL), "the fall was not written down");
    assert!(
        !super::super::decision_shadow::raised_at(&ledger),
        "the seat is still standing after its fall"
    );
}

#[test]
fn a_recording_seat_is_not_ended_by_fallbacks_it_never_acted_on() {
    use zerocode_core::jev::promote::FALLBACKS_THAT_END_IT;
    let work = tempfile::tempdir().expect("tmp");
    let rows: Vec<Value> = (0..FALLBACKS_THAT_END_IT as i64 + 2)
        .map(|at| json!({"at": at, "outcome": "timeout", "elapsedMs": 1_500}))
        .collect();
    let ledger = ledger_with(work.path(), &rows);
    assert_eq!(
        super::super::decision_shadow::judge_ledger(&ledger, None, 9),
        None,
        "a seat that never rose was taken back from somewhere it had not been"
    );
}

#[test]
fn nothing_the_detached_shadow_runs_resolves_a_path_of_its_own() {
    // The batch runs after `fire` returns. Whoever set the environment that
    // answers "where does this project's state live" — a test fixture, most
    // often — may have put it back by then, and a path resolved at write time
    // lands in a person's real ledger. Thirty rows of a unit test's `no_key`
    // did (2026-09-19). Everything the batch uses is resolved in `fire`.
    let shipped = include_str!("../decision_shadow.rs");
    let from = shipped.find("async fn run_shadow_batch").expect("the batch");
    let batch = &shipped[from..];
    let batch = batch.split("\n/// ").next().unwrap_or(batch);
    for reader in ["current_dir(", "decision_shadow_path(", "ConfigLoader::default_for("] {
        assert!(
            !batch.contains(reader),
            "the detached batch resolves `{reader}` itself instead of taking what `fire` resolved"
        );
    }
    let fires = shipped.find("pub(super) fn fire(").expect("fire");
    let fire = &shipped[fires..from];
    assert!(fire.contains("ledger: decision_shadow_path("), "fire no longer freezes the ledger");
}

#[test]
fn a_windows_cost_is_read_from_the_judgment_rate_and_not_the_chat_table() {
    // The two tables are kept apart on purpose: a judgment bills input only
    // and is never a chat candidate the plan scorer ranks. Asked of the chat
    // table this answered `None` for every seat, and the card drew no cost at
    // all against a price written down since the launch post.
    let rate = api::systemone_rate(api::SYSTEMONE_MODEL).expect("the judgment rate is written down");
    assert_eq!(super::cost_of(0), Some(0.0), "no tokens is no cost, not an unpriced seat");
    let million = super::cost_of(1_000_000).expect("priced");
    assert!((million - rate.input).abs() < 1e-12, "a million tokens is one unit of the rate");
    let some = super::cost_of(14_145).expect("priced");
    assert!(some > 0.0 && some < million, "{some} is not between nothing and a million tokens");
    assert_eq!(
        super::super::plan_shadow::model_price_for(api::SYSTEMONE_MODEL),
        None,
        "the chat table now names the judgment model — say which table costs come from",
    );
}
