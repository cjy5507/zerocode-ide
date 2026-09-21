use std::fmt::Write as _;
use std::fs;

use serde_json::json;

use super::*;

fn write(dir: &std::path::Path, name: &str, rows: &[Value]) {
    fs::create_dir_all(dir).expect("dir");
    let mut text = String::new();
    for row in rows {
        let _ = writeln!(text, "{row}");
    }
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
        .map(|seat| super::one(seat, &roots, None, None, 1_000, 0))
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
fn a_screen_seats_rows_are_counted_across_its_session_folders() {
    // The window's screen seats append beside each walk's evidence:
    // <sessions>/<session>/<ledger>. Counted from every folder, oldest first,
    // and named by the first — never "never asked" with rows on disk.
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().join("nowhere")];
    let sessions = home.path().join("computer-use").join("sessions");
    let seat = &zerocode_core::jev::BROWSER;
    write(&sessions.join("20260920-065957-3951"), seat.ledger, &[json!({"at": 10, "outcome": "answered", "elapsedMs": 40, "requests": 1})]);
    write(
        &sessions.join("20260919-040538-16330"),
        seat.ledger,
        &[json!({"at": 5, "outcome": "no_look"}), json!({"at": 6, "outcome": "answered", "elapsedMs": 60, "requests": 1})],
    );
    write(&sessions.join("20260918-000000-1"), "unrelated.txt", &[json!({"at": 1})]);
    let report = super::one(seat, &roots, Some(&sessions), None, 1_000, 0);
    assert_eq!(report.week.rows, 3, "every session folder's rows");
    assert_eq!(report.week.answered, 2);
    assert_eq!(report.week.p95_ms, Some(60));
    assert_eq!(
        report.found.as_deref(),
        Some(sessions.join("20260919-040538-16330").join(seat.ledger).as_path()),
        "named by the oldest session that has the file"
    );
    assert_eq!(
        super::one(seat, &roots, None, None, 1_000, 0).found,
        None,
        "without a sessions folder the seat is as unread as before"
    );
    assert!(super::session_ledgers(&home.path().join("missing"), seat.ledger).is_empty());
}

#[test]
fn a_screen_seat_is_counted_once_when_its_root_ledger_holds_what_a_session_copied() {
    // Both places hold the same rows (§4, decision 3). The root is the one
    // counted: a counter that added them would say a walk that answered
    // thirty-five times answered seventy.
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().join("jev")];
    let sessions = home.path().join("computer-use").join("sessions");
    let seat = &zerocode_core::jev::BROWSER;
    let walk: Vec<Value> = (0..35)
        .map(|n| json!({"at": 10 + n, "outcome": "answered", "elapsedMs": 40, "requests": 1, "pressed": true, "agreed": true}))
        .collect();
    write(&roots[0], seat.ledger, &walk);
    write(&sessions.join("20260921-000000-1"), seat.ledger, &walk);

    let report = super::one(seat, &roots, Some(&sessions), None, 1_000, 0);

    assert_eq!(report.week.rows, walk.len(), "one ledger, counted once");
    assert_eq!(report.found.as_deref(), Some(roots[0].join(seat.ledger).as_path()));
    assert_eq!(report.asked_ever, walk.len());
}

/// The rows a walk writes under `~/.zo/jev` are read by the same counter the
/// orchestration seats' are, and they carry the seat all the way to a rise
/// (§4, decision 3 and 5).
#[test]
fn a_screen_seats_root_ledger_carries_it_to_a_rise_the_card_can_draw() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().join("jev")];
    let seat = &zerocode_core::jev::BROWSER;
    let wanted = summary::rows_that_can_clear(seat.answer_floor_permille.expect("a rise line"));
    let walk: Vec<Value> = (0..wanted)
        .map(|n| {
            json!({
                "at": 10 + i64::try_from(n).unwrap_or_default(),
                "outcome": "answered",
                "elapsedMs": 300,
                "requests": 1,
                "pressed": true,
                "agreed": true,
            })
        })
        .collect();
    write(&roots[0], seat.ledger, &walk);
    let settings = json!({
        zerocode_core::jev::SMART_SETTINGS_KEY: { seat.setting: JevMode::Auto.key() }
    });

    let report = super::one(seat, &roots, None, Some(&settings), 1_000, 0);

    let judged = report.judged.as_ref().expect("a rising seat is judged");
    assert_eq!(judged.window_wanted, wanted, "35 walks, not routing's 73");
    assert_eq!(judged.window.asked(), wanted);
    assert_eq!(report.verdict(), Some(Verdict::Rise));
    assert_eq!(report.clears_rise_floor, Some(true));
    assert_eq!(report.rows_to_next_judgment(), Some(JUDGED_EVERY_ROWS - wanted % JUDGED_EVERY_ROWS));
    // It has not risen yet — nothing has written the transition — so the card
    // still draws it as recording, and says what it is waiting on.
    assert_eq!(report.stand, Stand::Recording);
    assert!(!report.applies);
}

#[test]
fn a_seat_that_never_rises_is_never_asked_to_clear_a_line() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf(), home.path().join("other")];
    let answered = [json!({"at": 10, "outcome": "answered", "elapsedMs": 1})];
    // `.iter()` and not the table itself: `JEV_USES` is a static array of a
    // `Copy` type, so walking it by value hands out copies and these take a
    // `&'static JevUse`.
    #[allow(clippy::explicit_iter_loop)]
    for seat in zerocode_core::jev::JEV_USES.iter() {
        write(home.path(), seat.ledger, &answered);
        let row = super::one(seat, &roots, None, None, 1_000, 0);
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
    // `.iter()` and not the table itself: `JEV_USES` is a static array of a
    // `Copy` type, so walking it by value hands out copies and these take a
    // `&'static JevUse`.
    #[allow(clippy::explicit_iter_loop)]
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
    let quiet = super::one(seat, &roots, None, Some(&auto), 1_000, 0);
    assert_eq!(quiet.stand, Stand::Recording);
    assert!(!quiet.applies, "auto starts recording");

    write(
        home.path(),
        seat.ledger,
        &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5}), json!({"at": 2, (TRANSITION.canonical): ROSE})],
    );
    let raised = super::one(seat, &roots, None, Some(&auto), 1_000, 0);
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
    let row = super::one(seat, &roots, None, None, 1_000, 0);
    assert!(matches!(row.verdict(), Some(Verdict::Hold(Line::TooFewRows { rows: 1, .. }))));

    // And a seat with no rise line is never judged at all. Recall, not an
    // orchestration seat: those rise now (2026-09-20), on the table's lines.
    let quiet = super::one(&zerocode_core::jev::RECALL, &roots, None, None, 1_000, 0);
    assert_eq!(quiet.verdict(), None);
    // An orchestration seat is judged by the table on its own rows: thin here.
    write(home.path(), zerocode_core::jev::SUMMON.ledger, &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5, "agreed": true})]);
    let summon = super::one(&zerocode_core::jev::SUMMON, &roots, None, None, 1_000, 0);
    assert!(matches!(summon.verdict(), Some(Verdict::Hold(Line::TooFewRows { rows: 1, .. }))), "{:?}", summon.verdict());
    assert_eq!(summon.judged.as_ref().map(|judged| judged.agreement.compared), Some(1));
}

/// A routing row as the executor writes one, with the probe's answer beside
/// the judgment's.
fn compared(at: i64, jev: [&str; 3], probe: Option<[&str; 3]>) -> Value {
    let axis = |choice: &str| json!({"choice": choice, "probabilities": {}, "confidence": 0.9});
    let mut row = json!({
        "at": at, "task": format!("{at:016x}"), "rubricVersion": 1, "outcome": "answered",
        "elapsedMs": 400, "retries": 0, "cached": false, "requests": 1,
        "jev": {"complexity": axis(jev[0]), "risk": axis(jev[1]), "intent": axis(jev[2])},
    });
    row["probe"] = probe.map_or_else(
        || json!("timeout"),
        |probe| json!({"complexity": probe[0], "risk": probe[1], "intent": probe[2], "confidence": "high"}),
    );
    row
}

#[test]
fn agreement_is_one_comparison_per_axis_where_both_readers_answered() {
    use zerocode_core::jev::promote::Agreement;
    let rows = [
        compared(1, ["large", "low", "analysis"], Some(["large", "low", "analysis"])),
        compared(2, ["large", "high", "other"], Some(["trivial", "low", "other"])),
        compared(3, ["small", "low", "other"], None),
        json!({"at": 4, "outcome": "no_key"}),
    ];
    let held: Vec<&Value> = rows.iter().collect();
    assert_eq!(
        super::super::decision_shadow::agreement_in(&held),
        Agreement { compared: 6, agreed: 4 },
        "two rows both answered (six axes, four the same); a timeout and a refusal compare nothing"
    );
    assert_eq!(super::super::decision_shadow::agreement_in(&[]), Agreement::default());
}

#[test]
fn the_screen_and_the_judge_read_one_window() {
    // The summary once judged the week with no labels while the judge judged
    // the last twenty with them, and the two could say different things of
    // the same file. The seat's report now carries the judge's own reading.
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::ROUTING;
    let rows: Vec<Value> = (0..30)
        .map(|at| compared(at, ["large", "low", "analysis"], Some(["large", "low", "analysis"])))
        .collect();
    write(home.path(), seat.ledger, &rows);
    let report = super::one(seat, &roots, None, None, i64::MAX / 2, 0);
    let judged = report.judged.as_ref().expect("the routing seat is judged");
    assert_eq!(Some(judged.clone()), super::super::decision_shadow::judge_rows(&rows, None));
    assert_eq!(judged.window.rows, 30, "the window is the last rows the floor can be cleared on");
    assert_eq!(judged.agreement.compared, 90);
    assert_eq!(report.asked_ever, 30);
    assert_eq!(report.rows_to_next_judgment(), Some(10), "cadence counts every request, not the week's");
    assert_eq!(report.verdict(), judged.verdict.into());
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
    let mut text = String::new();
    for row in rows {
        let _ = writeln!(text, "{row}");
    }
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
    let short: Vec<Value> = (0..i64::try_from(JUDGED_EVERY_ROWS).expect("a window fits") - 1).map(answered).collect();
    let ledger = ledger_with(work.path(), &short);
    assert_eq!(
        super::super::decision_shadow::judge_ledger(&ledger, None, 9),
        None,
        "a window one row short was judged anyway"
    );
    let full: Vec<Value> = (0..i64::try_from(JUDGED_EVERY_ROWS).expect("a window fits")).map(answered).collect();
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
    let full: Vec<Value> = (0..i64::try_from(JUDGED_EVERY_ROWS).expect("a window fits")).map(answered).collect();
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
    rows.extend((0..i64::from(FALLBACKS_THAT_END_IT)).map(|at| {
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
    let rows: Vec<Value> = (0..i64::from(FALLBACKS_THAT_END_IT) + 2)
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
    // The control sample's batch detaches the same way, and takes the
    // ledger the active road resolved before it wrote its rows.
    for head in ["async fn run_shadow_batch", "async fn run_control_batch"] {
        let at = shipped.find(head).unwrap_or_else(|| panic!("{head} is gone"));
        let batch = &shipped[at..];
        let batch = batch.split("\n/// ").next().unwrap_or(batch);
        for reader in ["current_dir(", "decision_shadow_path(", "ConfigLoader::default_for("] {
            assert!(
                !batch.contains(reader),
                "the detached batch `{head}` resolves `{reader}` itself instead of taking what the road resolved"
            );
        }
    }
    let fires = shipped.find("pub(super) fn fire(").expect("fire");
    let fire = &shipped[fires..from];
    assert!(fire.contains("ledger: decision_shadow_path("), "fire no longer freezes the ledger");
    let actives = shipped.find("pub(super) fn active_assessments(").expect("the active road");
    let active = &shipped[actives..];
    let active = &active[..active.find("\n}\n").map_or(active.len(), |end| end)];
    assert!(
        active.contains("let ledger = decision_shadow_path(") && active.contains("ControlBatch {\n        ledger,"),
        "the active road no longer hands the ledger it resolved to its control batch"
    );
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

#[test]
fn a_draft_is_the_shape_the_label_reader_takes_and_only_when_it_was_asked_for() {
    // Nobody could label a seat's work: the ledger keeps fingerprints and the
    // transcripts kept none of the ten judged tasks (2026-09-19). The draft is
    // written where the words still are.
    let shipped = include_str!("../decision_shadow.rs");
    let from = shipped.find("fn draft_labels(").expect("the drafter");
    let body = &shipped[from..shipped[from..].find("\n/// ").map_or(shipped.len(), |at| from + at)];
    assert!(
        body.contains("promote::label_drafts_wanted"),
        "the drafter writes a person's prompts without being asked to"
    );
    assert!(body.contains("already.contains(&task)"), "a filled-in draft can be buried by a blank one");
    assert!(
        body.contains("runtime::judged_axes()"),
        "the axes are spelled here instead of read from the rubric"
    );
    // Both roads that judge must draft. A seat in `on` never reaches `fire`,
    // and drafting only there left the one mode that makes the rule matter
    // with no labels to write.
    let judging: Vec<&str> = ["pub(super) fn fire(", "pub(super) fn active_assessments("]
        .iter()
        .map(|head| {
            let at = shipped.find(head).unwrap_or_else(|| panic!("{head} is gone"));
            let rest = &shipped[at..];
            &rest[..rest.find("\n}\n").map_or(rest.len(), |end| end)]
        })
        .collect();
    for road in judging {
        assert!(
            road.contains("draft_labels("),
            "a road that judges does not draft the words it judged"
        );
    }
    // The fingerprint's key is spelled once for the file (`TASK`): the draft
    // writes it and the judge joins control rows by it.
    for word in ["\"description\"", "\"prompt\"", "TASK: task"] {
        assert!(body.contains(word), "the draft is missing {word}, which the label reader joins on");
    }
    assert!(shipped.contains("const TASK: &str = \"task\";"), "the task key is no longer spelled once");
}

#[test]
fn the_control_sample_is_one_task_in_five_and_the_same_ones_every_time() {
    use zerocode_core::jev::summary::{JUDGED_EVERY_ROWS, rows_that_can_clear};
    use super::super::decision_shadow::{PROBE_CONTROL_EVERY, control_sampled};
    /// The probe answered this many of this machine's rows (2026-09-20).
    const PROBE_ANSWERED_ROWS: usize = 17;
    const PROBE_ROWS: usize = 28;
    // Decided on the fingerprint, so a task is in the sample or out of it for
    // good: a retry, a memo hit and a test all get the same answer.
    for task in [0_u64, 1, 4, 5, 6, 0x62e7_5100_5eed_a222, u64::MAX] {
        assert_eq!(control_sampled(task), control_sampled(task));
        assert_eq!(control_sampled(task), task % PROBE_CONTROL_EVERY == 0, "{task}");
    }
    let rounds = 40_u64;
    let picked = (0..PROBE_CONTROL_EVERY * rounds).filter(|task| control_sampled(*task)).count();
    assert_eq!(picked, usize::try_from(rounds).expect("fits"), "one in {PROBE_CONTROL_EVERY}");

    // The rate is sized to the judge's line. The window is the rows the
    // routing floor can be cleared on; the probe answered 17 of this
    // machine's 28 rows (2026-09-20). One in five leaves the twenty compared
    // axes with a row to spare; one in ten never reaches them.
    let window = rows_that_can_clear(zerocode_core::jev::ROUTING.answer_floor_permille.expect("routing rises"));
    let axes = runtime::judged_axes().count();
    let compared_axes = |every: u64| {
        let control_rows = window / usize::try_from(every).expect("fits");
        control_rows * PROBE_ANSWERED_ROWS / PROBE_ROWS * axes
    };
    assert!(
        compared_axes(PROBE_CONTROL_EVERY) >= JUDGED_EVERY_ROWS,
        "one in {PROBE_CONTROL_EVERY} compares {} axes a window, under the {JUDGED_EVERY_ROWS} the judge wants",
        compared_axes(PROBE_CONTROL_EVERY)
    );
    assert!(
        compared_axes(PROBE_CONTROL_EVERY * 2) < JUDGED_EVERY_ROWS,
        "a sample half as dense would still fill the line — the rate is coarser than it needs to be"
    );
}

/// An active row as the executor writes one: the judgment acted on and the
/// probe not run.
fn active(at: i64, task: &str, jev: [&str; 3]) -> Value {
    let mut row = compared(at, jev, None);
    row["task"] = json!(task);
    row["routeUse"] = json!("applied");
    row["probe"] = json!("not_run");
    row
}

/// The control row drawn beside it: the probe's answer, the judgment copied.
fn control(at: i64, task: &str, jev: [&str; 3], probe: [&str; 3]) -> Value {
    use zerocode_core::jev::summary::CONTROL;
    let mut row = compared(at, jev, Some(probe));
    row["task"] = json!(task);
    row["routeUse"] = json!(CONTROL);
    row["outcome"] = json!(CONTROL);
    row["requests"] = json!(0);
    row["elapsedMs"] = json!(0);
    row
}

#[test]
fn a_control_row_is_compared_beside_its_windows_row_and_counted_nowhere_else() {
    use zerocode_core::jev::promote::{Agreement, Line, Verdict};
    use zerocode_core::jev::summary::JUDGED_EVERY_ROWS;
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::ROUTING;
    let jev = ["large", "low", "analysis"];
    // Twenty-five active rows compare nothing on their own. Two of their
    // tasks have a control row — one agreeing on every axis, one on two —
    // and a third control row belongs to a task no row of the window holds.
    let mut rows: Vec<Value> = (0..25).map(|at| active(at, &format!("task-{at}"), jev)).collect();
    rows.push(control(100, "task-3", jev, ["large", "low", "analysis"]));
    rows.push(control(101, "task-4", jev, ["large", "high", "analysis"]));
    rows.push(control(102, "task-gone", jev, ["large", "low", "analysis"]));
    write(home.path(), seat.ledger, &rows);

    let judged = super::super::decision_shadow::judge_rows(&rows, None).expect("routing is judged");
    assert_eq!(judged.window.rows, 25, "a control row was counted in the window");
    assert_eq!(judged.window.answered, 25);
    assert_eq!(judged.window.p95_ms, Some(400), "a control row's zero elapsed joined the latency");
    assert_eq!(judged.agreement, Agreement { compared: 6, agreed: 5 }, "two control rows, three axes each");
    assert_eq!(judged.control_rows, 2, "the control row of a task outside the window was joined");
    assert!(
        matches!(judged.verdict, Verdict::Hold(Line::TooFewRows { rows: 25, .. })),
        "{:?}",
        judged.verdict
    );

    // A clock just past the rows, so the day and the week both hold them.
    let report = super::one(seat, &roots, None, None, 1_000, 0);
    assert_eq!(report.judged, Some(judged));
    assert_eq!(report.asked_ever, 25, "the cadence counted a control row");
    assert_eq!(report.rows_to_next_judgment(), Some(JUDGED_EVERY_ROWS - 25 % JUDGED_EVERY_ROWS));
    assert_eq!((report.today.rows, report.week.rows), (25, 25), "the day or the week counted a control row");

    // Without the control rows the same window compares nothing at all.
    let bare: Vec<Value> = rows.iter().take(25).cloned().collect();
    let bare = super::super::decision_shadow::judge_rows(&bare, None).expect("judged");
    assert_eq!((bare.agreement, bare.control_rows), (Agreement::default(), 0));
}

#[test]
fn an_orchestration_seat_has_no_control_rows_to_borrow() {
    // The window's seats read agreement off their own `agreed` marks; the
    // column is theirs too, so one shape reaches the screen, and it is zero.
    let rows: Vec<Value> = (0..3)
        .map(|at| json!({"at": at, "outcome": "answered", "elapsedMs": 5, "agreed": true}))
        .collect();
    let judged = zerocode_core::jev::promote::judge_seat(&zerocode_core::jev::SUMMON, &rows).expect("judged");
    assert_eq!((judged.agreement.compared, judged.control_rows), (3, 0));
}
