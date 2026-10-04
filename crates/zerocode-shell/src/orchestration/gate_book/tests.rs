use super::*;
use zerocode_core::hook::{Phase, Tool};

const NOW: i64 = 1_800_000_000_000;

/// The tests that touch the window's one book or its settings take this, so that
/// one turning the gate off is never the cause of another's count coming up
/// short.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn activity(phase: Phase) -> Activity {
    Activity {
        verb: Tool::Other("tool".into()),
        target: None,
        phase,
        reads: Vec::new(),
        writes: Vec::new(),
        vcs: Vec::new(),
        cwd: None,
        call: None,
    }
}

fn payload(index: usize) -> String {
    format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{{"file_path":"/repo/f{index}.rs"}}}}"#
    )
}

fn call(book: &mut GateBook, term: u32, token: &str, index: usize) {
    book.pane(term, token, NOW)
        .book
        .note_activity(&activity(Phase::Started), &payload(index));
}

fn steps(book: &mut GateBook, term: u32) -> u32 {
    book.pane_mut(term)
        .map_or(0, |pane| pane.book.metrics().steps)
}

#[test]
fn an_agents_tool_calls_are_counted_in_its_pane_and_a_new_launch_starts_over() {
    let mut book = GateBook::default();
    for index in 0..3 {
        call(&mut book, 7, "launch-1", index);
    }
    assert_eq!(steps(&mut book, 7), 3);
    assert_eq!(steps(&mut book, 8), 0, "another pane has none of them");
    call(&mut book, 7, "launch-2", 9);
    assert_eq!(
        steps(&mut book, 7),
        1,
        "a pane that launched again starts over"
    );
}

#[test]
fn a_pane_the_beat_made_first_adopts_the_launch_the_hooks_name_without_starting_over() {
    let mut book = GateBook::default();
    book.pane(7, "", NOW).book.note_cost(Some(1.0));
    call(&mut book, 7, "launch-1", 0);
    let metrics = book.pane_mut(7).expect("a record").book.metrics();
    assert_eq!(metrics.steps, 1);
    assert_eq!(metrics.spent_usd, Some(1.0), "what the beat read is kept");
}

#[test]
fn another_attempt_of_a_pane_starts_the_counts_over_and_keeps_its_meter() {
    let mut book = GateBook::default();
    {
        let pane = book.pane(7, "launch-1", NOW);
        pane.for_attempt("dp-1");
        pane.meter = Some(Meter::new("claude"));
        pane.book.note_cost(Some(2.0));
    }
    call(&mut book, 7, "launch-1", 0);
    {
        let pane = book.pane_mut(7).expect("a record");
        pane.for_attempt("dp-1");
        assert_eq!(
            pane.book.metrics().steps,
            1,
            "the same attempt keeps its counts"
        );
        pane.for_attempt("dp-2");
        assert_eq!(pane.book.metrics().steps, 0);
        assert_eq!(pane.book.spent_usd(), None);
        assert!(pane.meter.is_some(), "the transcript's place is the pane's");
    }
}

#[test]
fn the_book_holds_a_bounded_number_of_panes_and_forgets_the_least_recent() {
    let mut book = GateBook::default();
    for term in 0..u32::try_from(PANES_MAX + 40).expect("fits") {
        book.pane(term, "launch", NOW + i64::from(term));
    }
    assert_eq!(book.panes.len(), PANES_MAX);
    assert!(
        book.pane_mut(0).is_none(),
        "the first heard is the first forgotten"
    );
    assert!(
        book.pane_mut(u32::try_from(PANES_MAX + 39).expect("fits"))
            .is_some()
    );
}

#[test]
fn what_a_model_call_cost_goes_into_the_worker_its_task_and_the_day() {
    let mut book = GateBook::default();
    book.pane(7, "launch", NOW);
    book.pane(8, "launch", NOW);
    book.add_costs(
        7,
        "t-1",
        &[CallCost::Usd(1.5), CallCost::Unpriced, CallCost::Usd(0.5)],
        NOW,
    );
    book.add_costs(8, "t-1", &[CallCost::Usd(3.0)], NOW);
    book.add_costs(9, "t-1", &[CallCost::Usd(100.0)], NOW);
    let seven = book.pane_mut(7).expect("a record").book.metrics();
    assert_eq!(seven.spent_usd, Some(2.0));
    assert_eq!(seven.unpriced_calls, 1);
    assert!(
        (book.task_spent("t-1") - 5.0).abs() < 1e-9,
        "both workers' calls"
    );
    assert!(
        (book.day_spent(NOW) - 5.0).abs() < 1e-9,
        "and a pane the book never heard of is nobody's spend"
    );
    assert_eq!(book.task_spent("t-2"), 0.0);
}

#[test]
fn the_whole_days_projection_is_what_the_workers_running_now_are_about_to_spend() {
    let mut book = GateBook::default();
    for term in [7, 8, 9] {
        book.pane(term, "launch", NOW);
        book.add_costs(term, "t-1", &[CallCost::Usd(1.0); 5], NOW);
    }
    // Eight calls ahead at a dollar each, for each of the two running.
    let ahead = book.ahead_of(&[7, 8]);
    assert!((ahead - 16.0).abs() < 1e-9, "{ahead}");
    assert_eq!(book.ahead_of(&[]), 0.0);
    assert_eq!(book.ahead_of(&[42]), 0.0, "a pane nobody heard has no rate");
}

#[test]
fn only_the_tasks_that_cost_least_are_forgotten_past_the_bound() {
    let mut book = GateBook::default();
    book.pane(7, "launch", NOW);
    book.add_costs(7, "t-dear", &[CallCost::Usd(1_000.0)], NOW);
    for index in 0..TASKS_MAX + 20 {
        book.add_costs(7, &format!("t-{index}"), &[CallCost::Usd(0.01)], NOW);
    }
    assert!(book.tasks.len() <= TASKS_MAX);
    assert!(
        book.task_spent("t-dear") > 999.0,
        "the dear task is what a budget is about"
    );
}

#[test]
fn the_day_is_taken_for_saving_once_per_change() {
    let mut book = GateBook::default();
    assert!(book.take_day_for_saving().is_none());
    book.pane(7, "launch", NOW);
    book.add_costs(7, "t-1", &[CallCost::Usd(2.0)], NOW);
    let saved = book.take_day_for_saving().expect("it changed");
    assert!((saved.spent(NOW) - 2.0).abs() < 1e-9);
    assert!(book.take_day_for_saving().is_none(), "and has not since");
    let mut restarted = GateBook::default();
    restarted.restore_day(saved);
    assert!((restarted.day_spent(NOW) - 2.0).abs() < 1e-9);
}

#[test]
fn a_gate_that_is_off_hears_nothing_and_one_that_is_on_counts() {
    let _only = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let before = settings();
    set_settings(Settings {
        mode: Mode::Off,
        ..before
    });
    note_hook(9_001, "launch", &activity(Phase::Started), &payload(0), NOW);
    assert!(
        book().pane_mut(9_001).is_none(),
        "an off gate keeps no record"
    );
    set_settings(Settings {
        mode: Mode::Notify,
        ..before
    });
    note_hook(9_001, "launch", &activity(Phase::Started), &payload(0), NOW);
    assert_eq!(
        book().pane_mut(9_001).map(|pane| pane.book.metrics().steps),
        Some(1)
    );
    set_settings(before);
}

#[test]
fn a_days_spend_is_written_down_and_read_back_and_what_is_not_a_day_is_none() {
    let dir = tempfile::tempdir().expect("a config root");
    let file = dir.path().join("nested").join("gate-day-spend.json");
    assert!(load_day(&file).is_none(), "no file, no day");
    let mut day = DaySpend::default();
    day.add(NOW, 3.5);
    write_day(&file, &day);
    let back = load_day(&file).expect("the day as it was written");
    assert!((back.spent(NOW) - 3.5).abs() < 1e-9);
    std::fs::write(&file, "not json at all").expect("a damaged file");
    assert!(
        load_day(&file).is_none(),
        "a damaged file is no day, and does not stop the window"
    );
}

#[test]
fn what_the_window_only_learned_of_is_in_the_workers_ring_and_in_no_total() {
    let mut book = GateBook::default();
    book.pane(7, "launch", NOW);
    book.seed_costs(7, &[CallCost::Usd(1.0); 5]);
    assert!(
        (book.ahead_of(&[7]) - 8.0).abs() < 1e-9,
        "the projection knows how dear the worker's calls are from the first beat"
    );
    assert_eq!(book.task_spent("t-1"), 0.0);
    assert_eq!(book.day_spent(NOW), 0.0);
    let metrics = book.pane_mut(7).expect("a record").book.metrics();
    assert_eq!(metrics.spent_usd, None);
    assert_eq!(metrics.unpriced_calls, 0);
    book.seed_costs(9, &[CallCost::Usd(1.0)]);
    assert!(
        book.pane_mut(9).is_none(),
        "a pane the book never heard of gets no record from it"
    );
}
