//! t-57717: which conversation is a finished worker's — the ledger's half of
//! the window not bringing one back off its stored record on its own.
//!
//! On 2026-10-15 and 16 three finished workers' conversations came back as
//! agent panes nobody had called (t-54563, t-55838, t-56480), each on the
//! first visit to its checkout after a restart: the window read the
//! conversation off its stored layout and asked the ledger only whether it
//! was a sleeper. These are the answers it asks for now, on the core bench.

use super::*;

const SESSION: &str = "57717a0c-0000-4000-8000-000000000001";

fn session(id: &str) -> ProviderSession {
    ProviderSession {
        key: SessionKey::SessionId,
        id: id.to_string(),
        transcript_path: None,
    }
}

/// A run with one claude worker whose pane reported `id` — answers the
/// worker and its pane.
fn a_worker_on(bench: &mut Bench, id: &str) -> (String, String) {
    let task = bench.json("task-create --spec keep-going")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), "/wt/t-57717"));
    assert!(
        bench
            .ledger
            .worker_session_reported(("team-1", &pane), session(id))
    );
    (worker, pane)
}

fn set_state(bench: &mut Bench, worker: &str, state: WorkerState) {
    let at = bench.ledger.locate(worker).expect("the worker");
    bench.ledger.runs[at.0].workers[at.1].state = state;
}

fn finished(bench: &Bench, id: &str) -> Option<String> {
    bench
        .ledger
        .finished_worker_of("claude", id)
        .map(|worker| worker.id.clone())
}

fn carried_by(bench: &Bench, id: &str) -> Option<(String, WorkerState)> {
    bench
        .ledger
        .worker_of_conversation("claude", id)
        .map(|worker| (worker.id.clone(), worker.state))
}

/// The lifecycle the three cases walked: a worker at work, asleep across a
/// restart, then released. Only the last is a finished worker's
/// conversation; at every step the ledger names the row it is.
#[test]
fn a_released_workers_conversation_is_finished_and_a_working_or_sleeping_ones_is_not() {
    let mut bench = Bench::new();
    bench.json("run-create --name finished");
    let (worker, _pane) = a_worker_on(&mut bench, SESSION);

    assert_eq!(
        carried_by(&bench, SESSION),
        Some((worker.clone(), WorkerState::Active))
    );
    assert_eq!(finished(&bench, SESSION), None, "a worker at work");

    assert_eq!(bench.ledger.window_exiting(2_000).sleeping, 1);
    assert_eq!(
        carried_by(&bench, SESSION),
        Some((worker.clone(), WorkerState::Sleeping))
    );
    assert_eq!(
        finished(&bench, SESSION),
        None,
        "a sleeper is its coordinator's to seat again"
    );

    bench.json(&format!("worker-stop --worker {worker} --reason handed-in"));
    assert_eq!(
        carried_by(&bench, SESSION),
        Some((worker.clone(), WorkerState::Released))
    );
    assert_eq!(finished(&bench, SESSION), Some(worker));
}

/// Every state a row ends in is finished with its conversation; the three
/// that carry it on are not — the decision as one table.
#[test]
fn every_ended_state_is_finished_and_the_three_that_carry_on_are_not() {
    let mut bench = Bench::new();
    bench.json("run-create --name states");
    let (worker, _pane) = a_worker_on(&mut bench, SESSION);
    for state in WorkerState::ALL {
        set_state(&mut bench, &worker, state);
        let carries_on = matches!(
            state,
            WorkerState::Active | WorkerState::Sleeping | WorkerState::Orphaned
        );
        assert_eq!(
            finished(&bench, SESSION).is_some(),
            !carries_on,
            "{} read the wrong way",
            state.as_str()
        );
        assert_eq!(
            carried_by(&bench, SESSION).map(|(id, _)| id),
            Some(worker.clone()),
            "{} names no row",
            state.as_str()
        );
    }
}

/// A pane the person took is theirs (`taken_over`): its conversation is
/// never a finished worker's, whatever its row ended in — the ledger lets go
/// of it without touching it (`worker-abandon`), and the person may still be
/// in it.
#[test]
fn a_conversation_whose_pane_the_person_took_is_theirs() {
    let mut bench = Bench::new();
    bench.json("run-create --name taken");
    let (worker, pane) = a_worker_on(&mut bench, SESSION);
    assert!(bench.ledger.worker_taken_over(("team-1", &pane)));
    bench.json(&format!("worker-abandon --worker {worker} --reason theirs"));
    let row = bench
        .ledger
        .worker_of_conversation("claude", SESSION)
        .expect("the row");
    assert_eq!(row.id, worker);
    assert!(row.taken_over);
    assert_eq!(row.state, WorkerState::ReleaseUnknown);
    assert_eq!(finished(&bench, SESSION), None);
}

/// One conversation carried by two rows — an earlier attempt released, a
/// later one at work in the same conversation: the live row answers, and
/// nothing is finished until it ends too; then the newer row is named.
#[test]
fn a_conversation_another_row_carries_on_is_not_finished() {
    let mut bench = Bench::new();
    bench.json("run-create --name carried");
    let (first, _) = a_worker_on(&mut bench, SESSION);
    bench.json(&format!("worker-stop --worker {first} --reason retried"));
    let (second, _) = a_worker_on(&mut bench, SESSION);

    assert_eq!(
        carried_by(&bench, SESSION),
        Some((second.clone(), WorkerState::Active))
    );
    assert_eq!(finished(&bench, SESSION), None);

    bench.json(&format!("worker-stop --worker {second} --reason done"));
    assert_eq!(finished(&bench, SESSION), Some(second));
}

/// A conversation no worker reported is nobody's — and another agent's
/// conversation spelled the same is not this one.
#[test]
fn a_conversation_no_worker_reported_is_nobodys() {
    let mut bench = Bench::new();
    bench.json("run-create --name nobody");
    let (worker, _) = a_worker_on(&mut bench, SESSION);
    bench.json(&format!("worker-stop --worker {worker} --reason done"));

    assert_eq!(
        bench
            .ledger
            .worker_of_conversation("claude", "57717a0c-0000-4000-8000-0000000000ff")
            .map(|row| row.id.clone()),
        None
    );
    assert_eq!(
        bench
            .ledger
            .finished_worker_of("codex", SESSION)
            .map(|row| row.id.clone()),
        None
    );
}
