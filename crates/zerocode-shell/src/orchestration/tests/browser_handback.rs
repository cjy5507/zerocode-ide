//! t-67064 (7): the person hands a work browser back, and the agent holding
//! it is told without anybody typing at it. The window has the ledger file
//! one `status` from itself at the seat in that terminal — a worker's carries
//! its task and dispatch, a coordinator's none — and the mail pointer types
//! its one line at the resting pane, as it does for any letter. The same
//! hand-back told twice is one letter, and a pane no run seats is told
//! nothing.

use super::*;
use zerocode_core::orchestration::MessageKind;

const RECEIPT: &str = "browser-handback:browser-4:9";
const BODY: &str = "작업 브라우저 browser-4: 사람이 다시 맡겼습니다 — mode agent, workspaceRevision 9. 이어서 하세요: zerocode-browser marks browser-4로 다시 보고 이어 가세요";

/// The letters filed under the hand-back's receipt in `run`.
fn told(run: &str) -> Vec<zerocode_core::orchestration::MessageRow> {
    the_rows()
        .messages
        .into_iter()
        .filter(|mail| mail.run == run && mail.subject.as_str() == RECEIPT)
        .collect()
}

/// The lines typed at `term` once its pane rests and two beats pass.
fn pointed_at(host: &Pointing, term: u32) -> Vec<(u32, String)> {
    super::super::pane_turn_ended(term, 50, false, clock());
    super::super::tick(host, &[], clock());
    super::super::tick(host, &[], clock());
    host.typed()
        .into_iter()
        .filter(|(at, _)| *at == term)
        .collect()
}

#[test]
fn a_work_browser_handed_back_tells_its_worker_once_with_its_task_and_points_at_its_pane() {
    const LEADER: u32 = 67_064;
    const WORKER: u32 = 67_065;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-handback-{LEADER}");
    let (run_id, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let host = Pointing::default();

    let first = super::super::tell_seat_of_term(WORKER, BODY, RECEIPT, clock());
    let again = super::super::tell_seat_of_term(WORKER, BODY, RECEIPT, clock());
    let rows = the_rows();
    let dispatch = rows
        .workers
        .iter()
        .find(|row| row.id == worker)
        .and_then(|row| row.dispatch.clone())
        .expect("the worker's open dispatch");
    let task = rows
        .dispatches
        .iter()
        .find(|row| row.id == dispatch)
        .map(|row| row.task.clone())
        .expect("its task");
    let letters = told(&run_id);
    let pointed = pointed_at(&host, WORKER);
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);

    assert!(
        first && again,
        "the seat at the worker's terminal is told: {first} {again}"
    );
    assert_eq!(
        letters.len(),
        1,
        "one hand-back, one letter: {:?}",
        letters.len()
    );
    let letter = &letters[0];
    assert_eq!(
        (
            letter.from.as_str(),
            letter.to.clone(),
            letter.kind,
            letter.body.as_str()
        ),
        (
            zerocode_core::orchestration::LEDGER_ITSELF,
            zerocode_core::orchestration::worker_address(&worker),
            MessageKind::Status,
            BODY
        ),
        "a status from the ledger itself, in the window's own words"
    );
    assert_eq!(
        (letter.task.as_deref(), letter.dispatch.as_deref()),
        (Some(task.as_str()), Some(dispatch.as_str())),
        "the letter carries the work the worker is on"
    );
    assert_eq!(
        pointed,
        vec![
            (WORKER, zerocode_core::orchestration::pointer_text(1)),
            (WORKER, "\r".to_string()),
        ],
        "the resting pane hears of its letter by the pointer, once"
    );
}

#[test]
fn a_work_browser_handed_back_tells_a_coordinator_without_a_task_and_points_at_its_pane() {
    const LEADER: u32 = 67_066;
    const WORKER: u32 = 67_067;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-handback-{LEADER}");
    let (run_id, _worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let host = Pointing::default();

    let said = super::super::tell_seat_of_term(LEADER, BODY, RECEIPT, clock());
    let letters = told(&run_id);
    let pointed = pointed_at(&host, LEADER);
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);

    assert!(said, "the coordinator's seat is told");
    assert_eq!(
        letters.len(),
        1,
        "one hand-back, one letter: {:?}",
        letters.len()
    );
    let letter = &letters[0];
    assert_eq!(
        (
            letter.from.as_str(),
            letter.to.clone(),
            letter.kind,
            letter.task.as_deref(),
            letter.dispatch.as_deref()
        ),
        (
            zerocode_core::orchestration::LEDGER_ITSELF,
            format!("run:{run_id}"),
            MessageKind::Status,
            None,
            None
        ),
        "a coordinator's letter names no task it is not on"
    );
    assert_eq!(
        pointed,
        vec![
            (LEADER, zerocode_core::orchestration::pointer_text(1)),
            (LEADER, "\r".to_string()),
        ],
        "the resting coordinator hears of its letter by the pointer, once"
    );
}

#[test]
fn a_pane_no_run_seats_is_told_nothing() {
    const NOBODY: u32 = 67_068;
    let _window = the_window();
    assert!(!super::super::tell_seat_of_term(
        NOBODY,
        BODY,
        RECEIPT,
        clock()
    ));
}
