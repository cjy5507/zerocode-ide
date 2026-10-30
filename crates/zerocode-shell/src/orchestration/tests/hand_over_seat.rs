//! t-84850 r4: whose pane a 「대화」 may end is the ledger's to say.
//!
//! The hand-over types the pane's exit command, so it is refused on a pane
//! the ledger drives through its screen. It used to find a worker's pane in
//! the window's map of its own nested runs, which holds none of the
//! ledger's workers: a worker the ledger seated was a person's pane to it,
//! and its CLI would have been told to exit. Driven through a private
//! window's production doors, as `restore.rs` is — the window's nested-run
//! map is not there at all, so only the ledger can refuse.

use super::restore::{Restoring, a_run, a_worker};
use super::*;
use crate::cmd::wire::{COORDINATOR_PANE_STAYS, WORKER_PANE_STAYS, ledger_pane_refusal};

/// A worker the ledger seated keeps its pane; so does the run's coordinator;
/// a person's own pane beside them is handed over.
#[test]
fn a_pane_the_ledger_seated_a_worker_in_is_refused_and_a_persons_pane_is_not() {
    const LEADER: u32 = 284_850;
    const WORKER: u32 = 284_851;
    const PERSON: u32 = 284_859;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "hand-over-seat");
    let (_task, _worker, term) = a_worker(&host, &team, "--agent claude", None);

    assert_eq!(
        ledger_pane_refusal(term),
        Some(WORKER_PANE_STAYS),
        "a pane the ledger seated a worker in would be told to exit"
    );
    assert_eq!(
        ledger_pane_refusal(LEADER),
        Some(COORDINATOR_PANE_STAYS),
        "the run's coordinator pane would be told to exit"
    );
    assert_eq!(
        ledger_pane_refusal(PERSON),
        None,
        "a person's own pane is refused its hand-over"
    );
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
}
