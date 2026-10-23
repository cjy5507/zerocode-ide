//! Close one observed PTY incarnation and keep the process fact durable.
use zerocode_lane::PtyTransport as _;
use zerocode_orchestrator::terminal_exit::{ExitRecord, ExitState, TerminalExitJournal};

use crate::{
    agent_teams::ExitWitness,
    terminal_registry::{HeldTerminal, lock_pty},
};

pub(crate) fn prepare(
    journal: &TerminalExitJournal,
    epoch: &str,
    term: u32,
    held: &HeldTerminal,
) -> Result<ExitRecord, String> {
    let (incarnation, pid) = {
        let pty = lock_pty(held);
        (pty.incarnation(), pty.pid())
    };
    let identity = crate::durable_split::digest(
        b"zerocode.terminal-exit.v1",
        &[
            epoch.as_bytes(),
            &incarnation.to_le_bytes(),
            &term.to_le_bytes(),
        ],
    );
    // An already-requested close keeps its original process witness, even if
    // the group leader exited between calls. Re-reading its PID could record
    // a new program as the old close's target.
    let record = match journal.read(&identity).map_err(|e| e.to_string())? {
        Some(record) => record,
        None => {
            let witness = pid.map(|group| ExitWitness {
                group,
                started: crate::resource_usage::process_start_identity(group).ok(),
            });
            journal
                .begin(&identity, term, witness.as_ref(), crate::now_epoch_ms())
                .map_err(|e| e.to_string())?
        }
    };
    Ok(record)
}

pub(crate) fn close(
    journal: &TerminalExitJournal,
    epoch: &str,
    term: u32,
    held: &HeldTerminal,
) -> Result<ExitState, String> {
    close_with_budget(journal, epoch, term, held, crate::cmd::HAND_OVER_EXIT_WAIT)
}

fn close_with_budget(
    journal: &TerminalExitJournal,
    epoch: &str,
    term: u32,
    held: &HeldTerminal,
    wait: std::time::Duration,
) -> Result<ExitState, String> {
    let record = prepare(journal, epoch, term, held)?;
    let identity = &record.identity;
    if record.state == ExitState::Gone {
        return Ok(ExitState::Gone);
    }
    let signal = {
        let mut pty = lock_pty(held);
        // A refusal on a process already reaped is not a failed kill.
        match pty.try_wait() {
            Ok(Some(_)) => Ok(()),
            _ => pty.kill(),
        }
    };
    if signal.is_err() {
        journal
            .observe(identity, ExitState::Failed, crate::now_epoch_ms())
            .map_err(|e| e.to_string())?;
        return Ok(ExitState::Failed);
    }
    // Record signal delivery, never an invented exit. A crash at this point
    // leaves precisely the intent and uncertainty the next window can read.
    journal
        .observe(identity, ExitState::Unconfirmed, crate::now_epoch_ms())
        .map_err(|e| e.to_string())?;
    let began = std::time::Instant::now();
    loop {
        let child_left = matches!(lock_pty(held).try_wait(), Ok(Some(_)));
        let gone = child_left && group_left(record.witness.as_ref());
        if gone || began.elapsed() >= wait {
            let state = if gone {
                ExitState::Gone
            } else {
                ExitState::Unconfirmed
            };
            journal
                .observe(identity, state, crate::now_epoch_ms())
                .map_err(|e| e.to_string())?;
            return Ok(state);
        }
        std::thread::sleep(crate::cmd::HAND_OVER_EXIT_POLL);
    }
}

fn group_left(witness: Option<&ExitWitness>) -> bool {
    #[cfg(unix)]
    {
        witness.is_none_or(crate::cmd::program_left)
    }
    #[cfg(not(unix))]
    {
        // No process-group concept here. The caller still requires the
        // transport's child-exit receipt; a successful kill alone is false.
        let _ = witness;
        true
    }
}

/// One oldest local observation per beat. No signal is sent here, including
/// after a restart: a saved PID is evidence to inspect, not kill authority.
pub(crate) fn recheck(journal: &TerminalExitJournal, now_ms: i64) -> Result<(), String> {
    let rows = journal
        .pending(std::num::NonZeroU32::MIN)
        .map_err(|e| e.to_string())?;
    for row in rows {
        let gone = row.witness.as_ref().is_some_and(crate::cmd::program_left);
        journal
            .observe(
                &row.identity,
                if gone {
                    ExitState::Gone
                } else {
                    ExitState::Unconfirmed
                },
                now_ms,
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use zerocode_lane::{PtyHandle, PtyTransport, PtyTransportError};
    use zerocode_orchestrator::workflow_store::WorkflowStore;
    use zerocode_pty::{Pumped, Terminal};

    #[derive(Default)]
    struct Control {
        identity: String,
        signals: usize,
        refused: bool,
        ended: bool,
        stays_alive: bool,
    }
    struct Transport {
        screen: Terminal,
        control: Arc<Mutex<Control>>,
        journal: TerminalExitJournal,
    }
    impl PtyTransport for Transport {
        fn pump(&mut self) -> Pumped {
            Pumped {
                bytes: 0,
                ended: false,
                answered: false,
                unanswered_since: None,
            }
        }
        fn terminal(&self) -> &Terminal {
            &self.screen
        }
        fn terminal_mut(&mut self) -> &mut Terminal {
            &mut self.screen
        }
        fn write_input(&mut self, _: &[u8]) -> Result<(), PtyTransportError> {
            Ok(())
        }
        fn resize(&mut self, _: u16, _: u16) -> Result<(), PtyTransportError> {
            Ok(())
        }
        fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
            Ok(self.control.lock().unwrap().ended.then_some(0))
        }
        fn kill(&mut self) -> Result<(), PtyTransportError> {
            let mut control = self.control.lock().unwrap();
            let record = self
                .journal
                .read(&control.identity)
                .unwrap()
                .expect("intent precedes signal");
            assert_ne!(
                record.state,
                ExitState::Gone,
                "never signal an observed exit"
            );
            control.signals += 1;
            if control.refused {
                return Err(zerocode_pty::PtyError::Io(std::io::Error::other(
                    "synthetic kill refusal",
                ))
                .into());
            }
            control.ended = !control.stays_alive;
            Ok(())
        }
    }

    fn journal(root: &std::path::Path) -> TerminalExitJournal {
        let private = root.join("private");
        std::fs::create_dir_all(&private).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        TerminalExitJournal::new(WorkflowStore::open(private.join("authority.sqlite")).unwrap())
    }

    fn terminal(journal: &TerminalExitJournal) -> (HeldTerminal, Arc<Mutex<Control>>) {
        let control = Arc::new(Mutex::new(Control::default()));
        let pty = PtyHandle::new(Transport {
            screen: Terminal::new(4, 20),
            control: Arc::clone(&control),
            journal: journal.clone(),
        });
        control.lock().unwrap().identity = crate::durable_split::digest(
            b"zerocode.terminal-exit.v1",
            &[
                b"test-epoch",
                &pty.incarnation().to_le_bytes(),
                &7u32.to_le_bytes(),
            ],
        );
        (Arc::new(Mutex::new(pty)), control)
    }

    #[test]
    fn a_refused_signal_keeps_the_live_handle_and_a_durable_failure() {
        let root = tempfile::tempdir().unwrap();
        let journal = journal(root.path());
        let (held, control) = terminal(&journal);
        control.lock().unwrap().refused = true;
        assert_eq!(
            close(&journal, "test-epoch", 7, &held).unwrap(),
            ExitState::Failed
        );
        assert_eq!(lock_pty(&held).try_wait().unwrap(), None);
        let state = control.lock().unwrap();
        assert_eq!(state.signals, 1);
        assert!(
            journal
                .read(&state.identity)
                .unwrap()
                .unwrap()
                .signal_failed
        );
    }

    #[test]
    fn a_delivered_signal_is_not_an_exit_receipt() {
        let root = tempfile::tempdir().unwrap();
        let journal = journal(root.path());
        let (held, control) = terminal(&journal);
        control.lock().unwrap().stays_alive = true;
        assert_eq!(
            close_with_budget(&journal, "test-epoch", 7, &held, std::time::Duration::ZERO).unwrap(),
            ExitState::Unconfirmed
        );
        assert_eq!(lock_pty(&held).try_wait().unwrap(), None);
        let state = control.lock().unwrap();
        assert_eq!(
            journal.read(&state.identity).unwrap().unwrap().state,
            ExitState::Unconfirmed
        );
    }

    #[test]
    fn an_unwritable_intent_never_sends_a_signal() {
        let root = tempfile::tempdir().unwrap();
        let journal = journal(root.path());
        let (held, control) = terminal(&journal);
        std::fs::remove_file(root.path().join("private/authority.sqlite")).unwrap();
        assert!(close(&journal, "test-epoch", 7, &held).is_err());
        assert_eq!(control.lock().unwrap().signals, 0);
        assert_eq!(lock_pty(&held).try_wait().unwrap(), None);
    }

    #[test]
    fn an_exit_receipt_is_idempotent_and_a_reused_terminal_number_gets_another_identity() {
        let root = tempfile::tempdir().unwrap();
        let journal = journal(root.path());
        let (old, old_control) = terminal(&journal);
        assert_eq!(
            close(&journal, "test-epoch", 7, &old).unwrap(),
            ExitState::Gone
        );
        assert_eq!(
            close(&journal, "test-epoch", 7, &old).unwrap(),
            ExitState::Gone
        );
        let (_replacement, new_control) = terminal(&journal);
        assert_eq!(old_control.lock().unwrap().signals, 1);
        assert_eq!(new_control.lock().unwrap().signals, 0);
        assert_ne!(
            old_control.lock().unwrap().identity,
            new_control.lock().unwrap().identity
        );
    }

    #[test]
    fn a_recheck_never_retries_a_signal_and_preserves_an_unobservable_process() {
        let root = tempfile::tempdir().unwrap();
        let journal = journal(root.path());
        let (held, control) = terminal(&journal);
        control.lock().unwrap().refused = true;
        close(&journal, "test-epoch", 7, &held).unwrap();
        recheck(&journal, crate::now_epoch_ms()).unwrap();
        let state = control.lock().unwrap();
        assert_eq!(state.signals, 1);
        assert_eq!(
            journal.read(&state.identity).unwrap().unwrap().state,
            ExitState::Failed
        );
    }
}
