use std::fmt::Write as _;
use std::fs;

use serde_json::json;

use super::*;

/// One seat counted the card's way — its numbers and days, no recent list.
fn one(
    seat: &'static JevUse,
    roots: &[PathBuf],
    sessions: Option<&Path>,
    settings: Option<&Value>,
    now_ms: i64,
    offset_s: i64,
) -> SeatReport {
    super::one_with(seat, roots, sessions, settings, now_ms, offset_s, 0)
}

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
        .map(|seat| one(seat, &roots, None, None, 1_000, 0))
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
    let report = one(seat, &roots, Some(&sessions), None, 1_000, 0);
    assert_eq!(report.week.rows, 3, "every session folder's rows");
    assert_eq!(report.week.answered, 2);
    assert_eq!(report.week.p95_ms, Some(60));
    assert_eq!(
        report.found.as_deref(),
        Some(sessions.join("20260919-040538-16330").join(seat.ledger).as_path()),
        "named by the oldest session that has the file"
    );
    assert_eq!(
        one(seat, &roots, None, None, 1_000, 0).found,
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

    let report = one(seat, &roots, Some(&sessions), None, 1_000, 0);

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
    let wanted = promote::window_wanted_for(seat).expect("a rise line");
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

    let report = one(seat, &roots, None, Some(&settings), 1_000, 0);

    let judged = report.judged.as_ref().expect("a rising seat is judged");
    assert_eq!(judged.window_wanted, wanted, "a screen seat's own width, not routing's");
    assert_eq!(judged.window.asked(), wanted);
    assert_eq!(report.verdict(), Some(Verdict::Rise));
    assert_eq!(report.clears_rise_floor, Some(true));
    assert_eq!(report.rows_to_next_judgment(), Some(JUDGED_EVERY_ROWS), "a full window owes a full cadence");
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
        let row = one(seat, &roots, None, None, 1_000, 0);
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
    let quiet = one(seat, &roots, None, Some(&auto), 1_000, 0);
    assert_eq!(quiet.stand, Stand::Recording);
    assert!(!quiet.applies, "auto starts recording");

    write(
        home.path(),
        seat.ledger,
        &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5}), json!({"at": 2, (TRANSITION.canonical): ROSE})],
    );
    let raised = one(seat, &roots, None, Some(&auto), 1_000, 0);
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
    let row = one(seat, &roots, None, None, 1_000, 0);
    assert!(matches!(row.verdict(), Some(Verdict::Hold(Line::TooFewRows { rows: 1, .. }))));

    // Recall rises too now (t-5806), on its own labels: an empty ledger is a
    // window short of every row, not a seat nobody judges.
    let quiet = one(&zerocode_core::jev::RECALL, &roots, None, None, 1_000, 0);
    assert!(matches!(quiet.verdict(), Some(Verdict::Hold(Line::TooFewRows { rows: 0, .. }))), "{:?}", quiet.verdict());
    // An orchestration seat is judged by the table on its own rows: thin here.
    write(home.path(), zerocode_core::jev::SUMMON.ledger, &[json!({"at": 1, "outcome": "answered", "elapsedMs": 5, "agreed": true})]);
    let summon = one(&zerocode_core::jev::SUMMON, &roots, None, None, 1_000, 0);
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
    let report = one(seat, &roots, None, None, i64::MAX / 2, 0);
    let judged = report.judged.as_ref().expect("the routing seat is judged");
    assert_eq!(Some(judged.clone()), super::super::decision_shadow::judge_rows(&rows, None));
    assert_eq!(judged.window.rows, 30, "the window is the last rows the floor can be cleared on");
    assert_eq!(judged.agreement.compared, 90);
    assert_eq!(report.asked_ever, 30);
    assert_eq!(
        report.rows_to_next_judgment(),
        promote::rows_to_next_judgment(&zerocode_core::jev::ROUTING, 30),
        "cadence counts every request, not the week's"
    );
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

/// And the first judgment waits for the window the seat's own floor can be
/// cleared on, not for the cadence alone: a verdict read off a window
/// arithmetic has already decided cannot be full has one thing it can say.
#[test]
fn the_lines_are_judged_once_a_window_and_not_at_the_end_of_every_turn() {
    let work = tempfile::tempdir().expect("tmp");
    let wanted = i64::try_from(
        promote::window_wanted_for(&zerocode_core::jev::ROUTING).expect("routing rises"),
    )
    .expect("a window fits");
    let short: Vec<Value> = (0..wanted - 1).map(answered).collect();
    let ledger = ledger_with(work.path(), &short);
    assert_eq!(
        super::super::decision_shadow::judge_ledger(&ledger, None, 9),
        None,
        "a window one row short was judged anyway"
    );
    let full: Vec<Value> = (0..wanted).map(answered).collect();
    let ledger = ledger_with(work.path(), &full);
    assert!(
        super::super::decision_shadow::judge_ledger(&ledger, None, 9).is_some(),
        "a full window was not judged"
    );
}

#[test]
fn a_verdict_that_changed_nothing_writes_nothing_down() {
    let work = tempfile::tempdir().expect("tmp");
    let wanted = i64::try_from(
        promote::window_wanted_for(&zerocode_core::jev::ROUTING).expect("routing rises"),
    )
    .expect("a window fits");
    let full: Vec<Value> = (0..wanted).map(answered).collect();
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
    let report = one(seat, &roots, None, None, 1_000, 0);
    assert_eq!(report.judged, Some(judged));
    assert_eq!(report.asked_ever, 25, "the cadence counted a control row");
    assert_eq!(
        report.rows_to_next_judgment(),
        promote::rows_to_next_judgment(seat, 25),
        "the cadence counted a control row"
    );
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

/// The trend's buckets: today and the six days before it in the person's
/// zone, oldest first, each counted by the week's counter over its own rows
/// — a row at 23:59:59.999 belongs to its day and the next millisecond to
/// the next (docs/design/jev-dashboard-and-perfection-20260921.md §2 (c)).
#[test]
fn a_seats_days_are_seven_local_days_counted_by_the_weeks_own_counter() {
    let offset = 9 * 3_600;
    let now_ms = 1_789_700_000_000;
    let today = start_of_day_ms(now_ms, offset);
    let rows = [
        json!({"at": today - 1, "outcome": "answered", "elapsedMs": 300, "agreed": true}),
        json!({"at": today, "outcome": "answered", "elapsedMs": 100}),
        json!({"at": today + 5, "outcome": "not_consented"}),
        json!({"at": today - 6 * MS_PER_DAY + 10, "outcome": "answered", "elapsedMs": 50, "agreed": false}),
        json!({"at": today - 7 * MS_PER_DAY, "outcome": "answered", "elapsedMs": 999}),
        json!({"at": today + 3, "label": "k", "agreed": false}),
    ];
    let days = days_of(&rows, now_ms, offset);
    assert_eq!(days.len(), usize::try_from(WINDOW_DAYS).expect("days"));
    assert_eq!(days.last().expect("today").start_ms, today);
    assert_eq!(days[0].start_ms, today - 6 * MS_PER_DAY, "oldest first");
    let last = days.last().expect("today");
    assert_eq!((last.tally.rows, last.tally.answered, last.tally.refused), (2, 1, 1));
    assert_eq!(last.tally.p50_ms, Some(100));
    assert_eq!((last.agreement.compared, last.agreement.agreed), (1, 0), "a label row counts on its own day");
    let yesterday = &days[5];
    assert_eq!(yesterday.tally.rows, 1, "the millisecond before midnight is yesterday's");
    assert_eq!((yesterday.agreement.compared, yesterday.agreement.agreed), (1, 1));
    assert_eq!((days[0].tally.rows, days[0].agreement.compared), (1, 1));
    assert!(days.iter().all(|day| day.tally.p50_ms != Some(999)), "a row past the week is in no day");
}

/// The recent list rides the same rows the numbers were counted from, only
/// when asked, newest first, with the label a later row left.
#[test]
fn the_recent_list_is_read_from_the_same_rows_and_only_when_asked() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::STALL;
    let key = "dp-1@5";
    write(
        home.path(),
        seat.ledger,
        &[
            json!({"at": 5, "stall": key, "dispatch": "dp-1", "worker": "w-2", "outcome": "answered", "elapsedMs": 40,
                   "requests": 1, "chosen": "waiting_on_person", "confidence": 0.7}),
            json!({"at": 6, "stall": "dp-3@6", "outcome": "not_consented", "requests": 0}),
            json!({"at": 9, "label": key, "dispatch": "dp-1", "worker": "w-2", "followed": "worker_done", "agreed": false}),
        ],
    );
    let quiet = one(seat, &roots, None, None, 1_000, 0);
    assert!(quiet.recent.is_empty(), "the card's ask lists nothing");
    assert_eq!(quiet.week.rows, 2);

    let asked = report_with_recent(&roots, None, None, 1_000, 0, 5);
    let stall = asked.iter().find(|row| row.id == seat.id).expect("stall");
    assert_eq!(stall.week.rows, 2, "the same count");
    assert_eq!(stall.recent.len(), 2);
    assert_eq!(stall.recent[0].at, 6, "newest first");
    assert_eq!(stall.recent[0].outcome, "not_consented");
    let labelled = &stall.recent[1];
    assert_eq!(labelled.answered, json!("waiting_on_person"));
    assert_eq!(labelled.followed.as_deref(), Some("worker_done"));
    assert_eq!(labelled.agreed, Some(false));
    assert_eq!(labelled.asked.get("worker"), Some(&json!("w-2")));
    let one_row = report_with_recent(&roots, None, None, 1_000, 0, 1);
    assert_eq!(one_row.iter().find(|row| row.id == seat.id).expect("stall").recent.len(), 1);
    for other in asked.iter().filter(|row| row.id != seat.id) {
        assert!(other.recent.is_empty(), "{} has no rows to list", other.id);
        assert_eq!(other.days.len(), usize::try_from(WINDOW_DAYS).expect("days"), "{} still carries its days", other.id);
    }
}

/// One reader of a seat's rows: the numbers, the days and the recent list
/// are read from the file `rows_of` names, and a screen seat's session
/// copies are read only when the root has no file.
#[test]
fn rows_of_names_the_root_first_and_the_session_copies_only_without_it() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().join("jev")];
    let sessions = home.path().join("sessions");
    let seat = &zerocode_core::jev::BROWSER;
    write(&sessions.join("20260921-000000-1"), seat.ledger, &[json!({"at": 1, "outcome": "answered"})]);
    let (found, rows) = rows_of(seat, &roots, Some(&sessions));
    assert_eq!(found.as_deref(), Some(sessions.join("20260921-000000-1").join(seat.ledger).as_path()));
    assert_eq!(rows.len(), 1);
    write(&roots[0], seat.ledger, &[json!({"at": 2, "outcome": "answered"}), json!({"at": 3, "outcome": "answered"})]);
    let (found, rows) = rows_of(seat, &roots, Some(&sessions));
    assert_eq!(found.as_deref(), Some(roots[0].join(seat.ledger).as_path()));
    assert_eq!(rows.len(), 2, "the root's rows, not the root's and the copies'");
    assert_eq!(rows_of(seat, &roots[..0], None), (None, Vec::new()));
}

/// A routing row of a turn that declared its attempt, as the active road
/// writes one.
fn attempted(at: i64, attempt: &str) -> Value {
    let mut row = active(at, &format!("task-{at}"), ["large", "low", "analysis"]);
    row["attempt"] = json!(attempt);
    row
}

/// The turn label the host writes when the turn ends (t-5806): the route
/// stood, or a door moved the wire off it first. Written once, only for a
/// turn the seat was asked about, and read by the judge beside the probe's
/// axes.
#[test]
fn a_turn_the_seat_routed_is_labeled_once_with_what_became_of_its_route() {
    use super::super::decision_shadow::{decision_shadow_path, note_route_followed, RouteLabelRow, ROUTE_STOOD};
    let state = tempfile::tempdir().expect("a state home");
    let work = tempfile::tempdir().expect("a workspace");
    let _env = crate::tests::EnvGuard::set(core_types::paths::ZO_STATE_DIR_ENV, &state.path().to_string_lossy());
    let ledger = decision_shadow_path(work.path());
    let dir = ledger.parent().expect("a ledger dir").to_path_buf();
    let name = ledger.file_name().and_then(|name| name.to_str()).expect("a ledger name");
    write(&dir, name, &[attempted(1, "s@1"), attempted(2, "s@1"), attempted(3, "s@2")]);

    // A turn nobody routed leaves no label; an empty attempt asks nothing.
    assert!(!note_route_followed(work.path(), "s@9", None), "a turn the seat never judged was labeled");
    assert!(!note_route_followed(work.path(), "  ", None));
    assert!(read_rows(&ledger).len() == 3);

    // The route stood: the label agrees. Written once.
    assert!(note_route_followed(work.path(), "s@1", None));
    assert!(!note_route_followed(work.path(), "s@1", Some(runtime::SwitchTrigger::Quota)), "a second label for one turn");
    // Another turn's quota wall unseated its route: the label disagrees and
    // names the door.
    assert!(note_route_followed(work.path(), "s@2", Some(runtime::SwitchTrigger::Quota)));

    let labels: Vec<RouteLabelRow> = super::super::shadow_ledger::read_shadow_rows(&ledger);
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert_eq!((labels[0].label.as_str(), labels[0].attempt.as_str()), ("s@1", "s@1"));
    assert_eq!((labels[0].followed.as_str(), labels[0].agreed), (ROUTE_STOOD, true));
    assert_eq!((labels[1].attempt.as_str(), labels[1].followed.as_str(), labels[1].agreed), ("s@2", "quota", false));
    // A label is not a request: the counter leaves it out of every window.
    let rows = read_rows(&ledger);
    assert_eq!(rows.iter().filter(|row| asked_something(row).is_some()).count(), 3);
    for key in zerocode_core::jev::summary::LEDGER_KEYS {
        let row = rows.last().expect("the label");
        if let Some(read) = key.read(row) {
            assert_eq!(Some(read), row.get(key.canonical), "`{}` read from a spelling the label does not write", key.canonical);
        }
    }
}

/// Which doors unseat a route: a wall, a refusal, a shed tier, the person —
/// not a leg borrowing a client, not the step governor's rung.
#[test]
fn a_route_is_unseated_by_the_forced_doors_and_the_person_and_nothing_else() {
    use runtime::SwitchTrigger;
    let unseating: Vec<SwitchTrigger> = SwitchTrigger::ALL
        .into_iter()
        .filter(|trigger| super::super::decision_shadow::route_unseated_by(*trigger))
        .collect();
    assert_eq!(
        unseating,
        [SwitchTrigger::Person, SwitchTrigger::Quota, SwitchTrigger::Refusal, SwitchTrigger::Starvation]
    );
}

/// The judge reads a turn label beside the probe's axes: one comparison per
/// label of an attempt the window holds, and none for a turn whose rows have
/// left the window. The screen reads the same number.
#[test]
fn a_turn_label_is_one_comparison_in_the_window_of_the_turn_it_grades() {
    use zerocode_core::jev::promote::Agreement;
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::ROUTING;
    let mut rows: Vec<Value> = (0..25).map(|at| attempted(at, &format!("s@{}", at / 5))).collect();
    let label = |at: i64, attempt: &str, agreed: bool| {
        json!({"at": at, "label": attempt, "attempt": attempt, "followed": if agreed { "stood" } else { "quota" }, "agreed": agreed})
    };
    rows.push(label(100, "s@0", true));
    rows.push(label(101, "s@4", false));
    rows.push(label(102, "s@77", true));
    write(home.path(), seat.ledger, &rows);

    let judged = super::super::decision_shadow::judge_rows(&rows, None).expect("routing is judged");
    assert_eq!(judged.window.rows, 25, "a label was counted as a request");
    assert_eq!(judged.agreement, Agreement { compared: 2, agreed: 1 }, "two labels of held turns, one of a turn gone");
    assert_eq!(judged.control_rows, 0, "a label was counted as a control row");
    let report = one(seat, &roots, None, None, 1_000, 0);
    assert_eq!(report.judged, Some(judged));
    assert_eq!(report.asked_ever, 25);
    // The week counts every mark, held turn or not: three labels, two agreed.
    assert_eq!(report.agreement_week, Agreement { compared: 3, agreed: 2 });
}

/// A seat's week of marks is counted beside its judged window (t-5806): the
/// recall seat's labels are its agreement in both, and the week keeps the
/// marks the window has let go of.
#[test]
fn a_seats_week_of_marks_is_counted_beside_its_judged_window() {
    use zerocode_core::jev::promote::{Agreement, Line, Verdict};
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::RECALL;
    write(
        home.path(),
        seat.ledger,
        &[
            json!({"at": 1, "outcome": "answered", "elapsed_ms": 5, "applied": false}),
            json!({"at": 2, "label": "1:2", "query": 1, "notes": 2, "applied": false, "agreed": true, "rank": 0}),
            json!({"at": 3, "label": "3:4", "query": 3, "notes": 4, "applied": true, "agreed": false}),
            // Older than the week: not this week's mark.
            json!({"at": -1_000_000_000_000_i64, "label": "5:6", "query": 5, "notes": 6, "applied": true, "agreed": false}),
        ],
    );
    let report = one(seat, &roots, None, None, 1_000, 0);
    assert!(matches!(report.verdict(), Some(Verdict::Hold(Line::TooFewRows { rows: 1, .. }))), "{:?}", report.verdict());
    assert_eq!(report.judged.as_ref().map(|judged| judged.agreement), Some(Agreement { compared: 2, agreed: 1 }));
    assert_eq!(report.agreement_week, Agreement { compared: 2, agreed: 1 });
    assert_eq!((report.week.rows, report.asked_ever), (1, 1), "a label was counted as a request");
}

/// What the labels and the judge cost on real ledgers (t-5806) — run
/// deliberately, with `--ignored --nocapture` and `ZO_JEV_LEDGER_SOURCE`
/// naming a project's `state/smart-router` folder: it copies that folder's
/// routing (measured: 22 KB, 36 rows) and recall (1.4 MB, 1,005 rows)
/// ledgers into a scratch state directory and times the tail read a route
/// label pays, the whole-file read the judge pays, and the label append, each
/// a hundred times. Numbers, not a claim: the report reads them. The folder
/// comes from the environment because a checked-in path would name a person's
/// machine.
#[test]
#[ignore = "replays this machine's real ledgers; run deliberately"]
fn measure_what_a_label_and_a_judge_cost_on_this_machines_ledgers() {
    use std::time::Instant;
    use super::super::decision_shadow::{decision_shadow_path, note_route_followed};
    let Some(source) = std::env::var_os("ZO_JEV_LEDGER_SOURCE").map(std::path::PathBuf::from) else {
        eprintln!("ZO_JEV_LEDGER_SOURCE names no smart-router folder; nothing measured");
        return;
    };
    let source = source.as_path();
    if !source.join(zerocode_core::jev::ROUTING.ledger).is_file() {
        eprintln!("no real ledgers under {}; nothing measured", source.display());
        return;
    }
    let state = tempfile::tempdir().expect("a state home");
    let work = tempfile::tempdir().expect("a workspace");
    let _env = crate::tests::EnvGuard::set(core_types::paths::ZO_STATE_DIR_ENV, &state.path().to_string_lossy());
    let routing = decision_shadow_path(work.path());
    let recall = super::super::rerank_shadow::rerank_shadow_path(work.path());
    fs::create_dir_all(routing.parent().expect("a dir")).expect("dir");
    fs::copy(source.join(zerocode_core::jev::ROUTING.ledger), &routing).expect("copy routing");
    fs::copy(source.join(zerocode_core::jev::RECALL.ledger), &recall).expect("copy recall");
    let rows = read_rows(&routing);
    let attempt = rows
        .iter()
        .rev()
        .find_map(|row| row.get("attempt").and_then(Value::as_str))
        .expect("a routing row with an attempt")
        .to_string();
    let runs = 100;
    let timed = |name: &str, mut body: Box<dyn FnMut()>| {
        let started = Instant::now();
        for _ in 0..runs {
            body();
        }
        let each = started.elapsed() / runs;
        eprintln!("{name}: {} µs each over {runs} runs", each.as_micros());
    };
    let bytes = |path: &std::path::Path| fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    eprintln!("routing ledger {} B / {} rows; recall ledger {} B / {} rows", bytes(&routing), rows.len(), bytes(&recall), read_rows(&recall).len());
    // The route label: a tail read of the ledger and, the first time, one
    // append; every later call finds the label standing and appends nothing.
    let cwd = work.path().to_path_buf();
    let key = attempt.clone();
    timed(
        "note_route_followed (tail read, label standing after the first)",
        Box::new(move || {
            let _ = note_route_followed(&cwd, &key, None);
        }),
    );
    let held = rows.clone();
    timed("decision_shadow::judge_rows (routing, in memory)", Box::new(move || {
        let _ = super::super::decision_shadow::judge_rows(&held, None);
    }));
    let ledger = routing.clone();
    timed("read_rows (routing, whole file)", Box::new(move || {
        let _ = read_rows(&ledger);
    }));
    let ledger = recall.clone();
    timed("read_rows (recall, whole file)", Box::new(move || {
        let _ = read_rows(&ledger);
    }));
    let ledger = recall.clone();
    timed("rerank_shadow::judge_ledger (recall, whole file read + judge when due)", Box::new(move || {
        let _ = super::super::rerank_shadow::judge_ledger(&ledger, 1_800_000_000_000);
    }));
    let ledger = recall.clone();
    timed("append_shadow_row (one recall label)", Box::new(move || {
        let _ = super::super::shadow_ledger::append_shadow_row(
            &ledger,
            &json!({"at": 1, "label": "1:2", "query": 1, "notes": 2, "applied": true, "agreed": true, "rank": 0}),
            super::super::shadow_ledger::SHADOW_LEDGER_MAX_BYTES,
        );
    }));
}
