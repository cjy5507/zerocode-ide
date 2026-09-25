//! t-7812: the seams the restore roads added to the host — the window's own
//! answer that a pane already holds a conversation, and the conversation a
//! reseated pane is written into its table — asked through the same private
//! window as `restore.rs`.

use super::restore::{
    Restoring, a_run, a_worker, census_without_commands, deaths_of, row, the_coordinator_returns,
    the_window_goes,
};
use super::*;
use std::collections::HashMap;

/// [`Restoring`], with the window's pane table: which conversation a live
/// pane already holds, and what the reseat wrote into it.
pub(super) struct WithTable {
    pub(super) inner: Restoring,
    pub(super) standing: Mutex<HashMap<String, u32>>,
    carried: Mutex<Vec<(u32, String)>>,
}

impl Host for WithTable {
    fn split(
        &self,
        team: &str,
        leader_term: u32,
        from_term: u32,
        pane: &str,
        direction: zerocode_core::agent_teams::Direction,
        command: &str,
        token: &str,
    ) -> Option<u32> {
        self.inner.split(
            team,
            leader_term,
            from_term,
            pane,
            direction,
            command,
            token,
        )
    }
    fn send(&self, term: u32, text: &str) -> bool {
        self.inner.send(term, text)
    }
    fn capture(&self, term: u32) -> Option<String> {
        self.inner.capture(term)
    }
    fn focus(&self, term: u32) -> bool {
        self.inner.focus(term)
    }
    fn close(&self, term: u32) {
        self.inner.close(term);
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        self.inner.actor_for(term)
    }
    fn conversation_standing(
        &self,
        _agent: &str,
        session: &zerocode_core::ProviderSession,
    ) -> Option<u32> {
        self.standing.lock().unwrap().get(&session.id).copied()
    }
    fn carry_session(&self, term: u32, session: &zerocode_core::ProviderSession) {
        self.carried
            .lock()
            .unwrap()
            .push((term, session.id.clone()));
    }
}

pub(super) fn with_table(
    checkout: &std::path::Path,
    first: u32,
    new_leader: u32,
    old_leader: u32,
) -> WithTable {
    WithTable {
        inner: Restoring::new(checkout, first, (new_leader, test_actor(old_leader))),
        standing: Mutex::new(HashMap::new()),
        carried: Mutex::new(Vec::new()),
    }
}

/// The reseated pane IS the worker's conversation from the moment its seat
/// is durable: the window's table is told so then, not at the agent's first
/// report, so a door that asks in between is sent to this pane.
#[test]
fn a_reseated_pane_is_written_into_the_windows_table_as_its_conversation() {
    const OLD_LEADER: u32 = 278_300;
    const WORKER: u32 = 278_301;
    const NEW_LEADER: u32 = 278_310;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = with_table(checkout.path(), WORKER, NEW_LEADER, OLD_LEADER);
    let (team, _run) = a_run(&host, OLD_LEADER, "table");
    let (_task, worker, term) = a_worker(
        &host.inner,
        &team,
        "--agent codex",
        Some("session-t7812-table"),
    );
    the_window_goes(&census_without_commands, &[term]);
    host.inner.cut.lock().unwrap().clear();
    assert_eq!(
        the_coordinator_returns(&host, "table", OLD_LEADER, NEW_LEADER),
        1
    );
    let reseated = host.inner.cuts()[0].0;
    assert_eq!(
        *host.carried.lock().unwrap(),
        vec![(reseated, "session-t7812-table".to_string())]
    );
    assert_eq!(row(&worker).state, WorkerState::Active);
    crate::agent_teams::forget_term(reseated);
    crate::agent_teams::forget_term(NEW_LEADER);
}

/// A conversation a live pane already holds — a person reopened it from
/// the sidebar before the coordinator came back, in another checkout, so no
/// witness could seat it — is not cut a second pane by the reseat: a second
/// process is a second writer on one transcript. The worker waits for the
/// grace, which is the coordinator's to hear about.
#[test]
fn a_conversation_a_live_pane_already_holds_gets_no_second_pane() {
    const OLD_LEADER: u32 = 278_320;
    const WORKER: u32 = 278_321;
    const HOLDER: u32 = 278_329;
    const NEW_LEADER: u32 = 278_330;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = with_table(checkout.path(), WORKER, NEW_LEADER, OLD_LEADER);
    let (team, _run) = a_run(&host, OLD_LEADER, "held");
    let (_task, worker, term) = a_worker(
        &host.inner,
        &team,
        "--agent codex",
        Some("session-t7812-held"),
    );
    the_window_goes(&census_without_commands, &[term]);
    host.inner.cut.lock().unwrap().clear();
    host.standing
        .lock()
        .unwrap()
        .insert("session-t7812-held".to_string(), HOLDER);
    assert_eq!(
        the_coordinator_returns(&host, "held", OLD_LEADER, NEW_LEADER),
        0
    );
    assert!(host.inner.cuts().is_empty(), "{:?}", host.inner.cuts());
    assert_eq!(row(&worker).state, WorkerState::Sleeping);
    assert!(deaths_of(&worker).is_empty());
    crate::agent_teams::forget_term(NEW_LEADER);
}

/// The one policy both roads ask (t-7812 E): words only for what the
/// goodbye cut — and read, not spent (t-7812 R2): the note is spent by the
/// wake that hands the words to a pane, and by nothing before it.
#[test]
fn a_wake_hears_only_what_the_goodbye_cut_and_hears_it_once() {
    let root = tempfile::tempdir().expect("a data root");
    let census = restart_census::RestartCensus {
        workers: vec![
            restart_census::WorkerCut {
                worker: "w-turn".to_string(),
                agent: "claude".to_string(),
                term: 1,
                turn: restart_census::Turn::Running,
                commands: Some(Vec::new()),
            },
            restart_census::WorkerCut {
                worker: "w-rest".to_string(),
                agent: "claude".to_string(),
                term: 2,
                turn: restart_census::Turn::Rest,
                commands: Some(Vec::new()),
            },
            restart_census::WorkerCut {
                worker: "w-gate".to_string(),
                agent: "codex".to_string(),
                term: 3,
                turn: restart_census::Turn::Rest,
                commands: Some(vec!["just gate".to_string()]),
            },
        ],
        took_ms: 0,
    };
    restart_census::leave_cut(root.path(), &census, &|_| false).expect("the goodbye's note");
    let turn = crate::restart_nudge_runtime::worker_nudge(root.path(), "w-turn", None)
        .expect("the cut turn is continued");
    assert!(turn.starts_with(crate::RESTART_NUDGE), "{turn}");
    assert!(
        turn.contains(crate::restart_nudge_runtime::RESEATED_NUDGE),
        "{turn}"
    );
    assert_eq!(
        crate::restart_nudge_runtime::worker_nudge(root.path(), "w-rest", None),
        None,
        "an idle worker was told to go on"
    );
    let gate = crate::restart_nudge_runtime::worker_nudge(root.path(), "w-gate", None)
        .expect("the cut gate is named");
    assert!(
        !gate.starts_with(crate::RESTART_NUDGE),
        "a turn that had ended was continued: {gate}"
    );
    assert!(gate.contains("`just gate`"), "{gate}");
    assert_eq!(
        crate::restart_nudge_runtime::worker_nudge(root.path(), "w-turn", None).as_deref(),
        Some(turn.as_str()),
        "reading the words spent them before any pane was told"
    );
    crate::restart_nudge_runtime::nudge_spent(root.path(), "w-turn");
    assert_eq!(
        crate::restart_nudge_runtime::worker_nudge(root.path(), "w-turn", None),
        None,
        "the same words were said twice"
    );
    // No goodbye at all — a crash — is nothing cut.
    let crashed = tempfile::tempdir().expect("another data root");
    assert_eq!(
        crate::restart_nudge_runtime::worker_nudge(crashed.path(), "w-turn", None),
        None
    );
    // An empty continuation is no input at all.
    struct Keys(Mutex<Vec<String>>);
    impl Host for Keys {
        fn split(
            &self,
            _: &str,
            _: u32,
            _: u32,
            _: &str,
            _: zerocode_core::agent_teams::Direction,
            _: &str,
            _: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, _term: u32, text: &str) -> bool {
            self.0.lock().unwrap().push(text.to_string());
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            true
        }
        fn close(&self, _term: u32) {}
    }
    let keys = Keys(Mutex::new(Vec::new()));
    assert!(super::super::deliver_continuation(&keys, 1, "", "w-rest"));
    assert!(
        keys.0.lock().unwrap().is_empty(),
        "an empty continuation was typed"
    );
    assert!(super::super::deliver_continuation(
        &keys, 1, "go on", "w-gate"
    ));
    assert_eq!(*keys.0.lock().unwrap(), vec!["go on".to_string()]);
}
