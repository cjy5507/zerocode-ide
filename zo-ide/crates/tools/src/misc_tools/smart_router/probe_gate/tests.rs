//! What the gate ledger promises: the moment an answer changed, the reason in
//! words, and no line at all while nothing changes.

use std::ffi::OsString;
use std::sync::{MutexGuard, PoisonError};

use super::*;
use crate::misc_tools::smart_router::shadow_ledger::read_shadow_rows;

/// A workspace whose ledger lands under a directory of this test's own.
///
/// `shadow_ledger_path` does not write inside `cwd` — it maps `cwd` to a slug
/// under the person's zo home, so a bare `tempdir()` isolates the *slug* and
/// nothing else, and the rows land in the real `~/.zo/projects`. `ZO_STATE_DIR`
/// is the redirect that makes the isolation real. It is process-wide, so the
/// crate's environment lock is held for the life of the case, which is the
/// order every other holder in this crate keeps.
struct Workspace {
    cwd: tempfile::TempDir,
    /// Held for its `Drop`, and read by the case that needs a path inside it.
    state: tempfile::TempDir,
    previous: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl Workspace {
    fn new() -> Self {
        let lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let cwd = tempfile::tempdir().expect("a workspace");
        let state = tempfile::tempdir().expect("a state dir");
        let previous = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV);
        std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, state.path());
        forget(cwd.path());
        Self {
            cwd,
            state,
            previous,
            _lock: lock,
        }
    }

    fn path(&self) -> &Path {
        self.cwd.path()
    }

    fn rows(&self) -> Vec<ProbeGateRow> {
        read_shadow_rows(&shadow_ledger_path(self.path(), PROBE_GATE_LEDGER))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        forget(self.cwd.path());
        match self.previous.take() {
            Some(value) => std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, value),
            None => std::env::remove_var(core_types::paths::ZO_STATE_DIR_ENV),
        }
    }
}

/// A run of identical verdicts is one row, and the run ending is the next.
///
/// This is the whole design: 161 turns that all declined for the same reason
/// are one fact — *it started here, and this is why* — not 161 lines that
/// bury it.
#[test]
fn only_a_change_of_answer_is_written_down() {
    let workspace = Workspace::new();

    for _ in 0..5 {
        note(workspace.path(), ProbeGate::VerdictUnread);
    }
    let written = workspace.rows();
    assert_eq!(written.len(), 1, "a run is one row: {written:?}");
    assert_eq!(written[0].gate, ProbeGate::VerdictUnread);
    assert_eq!(written[0].was, None, "the first row replaced nothing");
    assert!(written[0].at > 0);

    /* The run ends. A file that only ever recorded declines could never show
     * a workspace recovering, so `admitted` is a verdict like any other. */
    note(workspace.path(), ProbeGate::Admitted);
    note(workspace.path(), ProbeGate::Admitted);
    let written = workspace.rows();
    assert_eq!(written.len(), 2, "the change is the second row: {written:?}");
    assert_eq!(written[1].gate, ProbeGate::Admitted);
    assert_eq!(
        written[1].was,
        Some(ProbeGate::VerdictUnread),
        "a row names the answer it replaced, so a reader needs one line and not two"
    );
}

/// Two workspaces do not answer for each other.
///
/// The memo that keeps an unchanged verdict off the disk is keyed by
/// workspace. Held as one value instead, a process serving two directories
/// would read one's answer as the other's and call a real change "unchanged" —
/// losing exactly the transition this ledger exists to keep.
#[test]
fn one_workspaces_answer_is_not_anothers() {
    let first = Workspace::new();
    let second = tempfile::tempdir().expect("a second workspace");
    forget(second.path());

    note(first.path(), ProbeGate::VerdictUnread);
    note(second.path(), ProbeGate::VerdictUnread);
    note(first.path(), ProbeGate::VerdictUnread);

    assert_eq!(first.rows().len(), 1, "the first workspace wrote once");
    let other = read_shadow_rows::<ProbeGateRow>(&shadow_ledger_path(
        second.path(),
        PROBE_GATE_LEDGER,
    ));
    assert_eq!(other.len(), 1, "the second wrote its own first row");
    assert_eq!(
        other[0].was, None,
        "the second workspace had no previous answer — it must not inherit the first's"
    );
    forget(second.path());
}

/// The row says what it means without the source open.
///
/// The reason this ledger exists is that answering "why did the judgment
/// never run?" took an afternoon of reading `turn.rs`. A row carrying only
/// `verdict_unread` would have saved the afternoon and spent an hour.
#[test]
fn a_row_carries_the_sentence_a_reader_needs() {
    let workspace = Workspace::new();
    note(workspace.path(), ProbeGate::VerdictUnread);

    let written = workspace.rows();
    assert_eq!(written[0].means, ProbeGate::VerdictUnread.means());
    assert!(
        written[0].means.contains("exec implementer"),
        "the sentence must name the condition, not restate the token: {}",
        written[0].means
    );

    /* Every verdict says something, including the good one. */
    for gate in [
        ProbeGate::Admitted,
        ProbeGate::NotWorthIt,
        ProbeGate::ClassifierOff,
        ProbeGate::SettingsUnavailable,
        ProbeGate::VerdictUnread,
    ] {
        assert!(!gate.means().is_empty(), "{gate:?} explains nothing");
        assert_ne!(
            gate.means(),
            gate.token(),
            "{gate:?} restates its token instead of explaining it"
        );
    }
}

/// The ledger's word and the telemetry's word are the same word.
///
/// `turn.rs` used to spell each reason as a literal beside its own `attest_`
/// call. Two vocabularies for one decision is how a ledger and a counter come
/// to disagree about what happened.
#[test]
fn the_token_is_the_one_the_attestation_uses() {
    let workspace = Workspace::new();
    note(workspace.path(), ProbeGate::NotWorthIt);

    let line = std::fs::read_to_string(shadow_ledger_path(workspace.path(), PROBE_GATE_LEDGER))
        .expect("the ledger");
    assert!(
        line.contains("\"gate\":\"not_worth_it\""),
        "the wire word must be the attested word: {line}"
    );
    /* The four the gates in `turn.rs` can attest, spelled once here. */
    assert_eq!(ProbeGate::NotWorthIt.token(), "not_worth_it");
    assert_eq!(ProbeGate::ClassifierOff.token(), "classifier_off");
    assert_eq!(ProbeGate::VerdictUnread.token(), "verdict_unread");
    assert_eq!(
        ProbeGate::SettingsUnavailable.token(),
        "settings_unavailable"
    );
}

/// A ledger that cannot be written does not fail the turn.
///
/// This is a diagnostic; a turn must never be worse off for one. The state
/// directory is pointed at a path that cannot become a directory, which is
/// how a read-only or occupied home actually fails.
#[test]
fn a_ledger_that_cannot_be_written_is_silent_rather_than_loud() {
    let workspace = Workspace::new();
    let blocked = workspace.state.path().join("file-where-a-dir-belongs");
    std::fs::write(&blocked, b"not a directory").expect("a file in the way");
    std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, &blocked);

    note(workspace.path(), ProbeGate::ClassifierOff);

    assert!(
        workspace.rows().is_empty(),
        "nothing was written, and nothing panicked"
    );
}
