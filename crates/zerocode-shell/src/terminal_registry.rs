//! The window's terminal pool, sharded so one shell's parse never taxes
//! another shell's keystroke.
//!
//! The pool used to be one `Mutex<HashMap<TermId, PtyHandle>>`, and that one
//! lock was the whole of the typing lag it caused: the pump parsed every
//! terminal's output under it in sequence — measured at 3.40ms average and
//! 6.39ms worst per 256KB TUI burst, summed over every open terminal — while
//! the synchronous `term_text`/`term_key` commands waited on the macOS main
//! thread for the entire round, which is what the reported 20–45ms input
//! spikes were.
//!
//! Now the registry is two tiers:
//!
//! - a **map lock**, held only long enough to insert, remove, or clone the
//!   `Arc` of an entry — never across a parse, a write, a resize, or any
//!   other lock;
//! - a **terminal lock** per shell (`Arc<Mutex<PtyHandle>>`), which is what
//!   the pump, input, snapshot and resize paths actually hold while they
//!   work.
//!
//! So a keystroke for terminal B waits, at worst, for B's own current parse —
//! never for A's, and never for the whole pool.
//!
//! # Lock order
//!
//! The rules every path through this window keeps, and the concurrency tests
//! below pin:
//!
//! 1. **The map lock is a leaf.** It is private to this module, and no method
//!    here touches a terminal lock — or anything else — while holding it.
//!    Callers can never hold it at all: every public method returns before
//!    its guard is released only by way of plain data or an `Arc` clone.
//! 2. **A terminal lock is a leaf for its holder.** Code holding one must not
//!    take the map lock (do not call back into this registry), another
//!    terminal's lock, or `AppState::deliveries`. The pump takes terminal
//!    locks strictly one at a time.
//! 3. **`AppState::deliveries` may be taken before a terminal lock** — the
//!    pump holds it across its per-terminal walk so a settled delivery and
//!    the next queued prompt swap under one guard, exactly as they did under
//!    the single pool lock — but never the other way around.
//!
//! # Exit removal
//!
//! Removal on exit goes through [`TerminalRegistry::remove_exact`], which
//! takes the handle the caller actually observed and removes the entry only
//! if it is still that same shell. The pump works from a snapshot
//! ([`TerminalRegistry::entries`]), and between its snapshot and its removal
//! the id can be retired and reopened — the float keeps [`crate::FLOAT_TERM`]
//! across respawns — so an unconditional remove could take a live
//! replacement's entry with a dead shell's evidence.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use zerocode_lane::PtyHandle;

use crate::TermId;

/// One live shell, shared between the registry and whoever is working on it.
///
/// The `Arc` is the shard: cloning it out of the map is the only thing the
/// map lock is held for, and the `Mutex` inside is the only lock the real
/// work — parse, write, snapshot, resize — is done under.
pub type HeldTerminal = Arc<Mutex<PtyHandle>>;

/// Lock one terminal for work, riding out a poisoned lock the way every
/// other guard in this window does: the pty holds plain state that is valid
/// at every step, so continuing is strictly better than a window that stops
/// painting.
pub fn lock_pty(held: &HeldTerminal) -> MutexGuard<'_, PtyHandle> {
    held.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The window's terminal pool. See the module documentation for the two-tier
/// locking this type exists to enforce.
#[derive(Default)]
pub struct TerminalRegistry {
    held: Mutex<HashMap<TermId, HeldTerminal>>,
}

impl TerminalRegistry {
    fn map(&self) -> MutexGuard<'_, HashMap<TermId, HeldTerminal>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take a shell into the pool. A previous holder of the same id — the
    /// float's respawn — is replaced, and the replaced shell ends when its
    /// last observer lets go of it.
    pub fn insert(&self, term: TermId, pty: PtyHandle) {
        self.map().insert(term, Arc::new(Mutex::new(pty)));
    }

    /// Remove a shell deliberately, whoever it is by now. The handle comes
    /// back so the caller can read the final screen — under the terminal
    /// lock, after this map guard is already gone.
    pub fn remove(&self, term: &TermId) -> Option<HeldTerminal> {
        self.map().remove(term)
    }

    /// Remove a shell **only if it is still the one the caller observed**.
    ///
    /// This is the exit path's remove: the pump decides a shell has died
    /// while working from a snapshot, and by the time it removes, the id may
    /// belong to a replacement. Compared by `Arc` identity, which is exactly
    /// "the same entry" and never a judgement about contents.
    pub fn remove_exact(&self, term: TermId, observed: &HeldTerminal) -> bool {
        let mut map = self.map();
        if map
            .get(&term)
            .is_some_and(|held| Arc::ptr_eq(held, observed))
        {
            map.remove(&term);
            return true;
        }
        false
    }

    /// The shell behind an id, as a handle to lock after this call returns.
    pub fn handle(&self, term: TermId) -> Option<HeldTerminal> {
        self.map().get(&term).cloned()
    }

    /// Whether an id currently names a live shell.
    pub fn contains_key(&self, term: &TermId) -> bool {
        self.map().contains_key(term)
    }

    /// Whether an id still names the very shell the caller observed earlier.
    /// The frame road uses this: a delta read from a shell that has since
    /// been replaced must not land on the replacement's screen.
    pub fn still_holds(&self, term: TermId, observed: &HeldTerminal) -> bool {
        self.map()
            .get(&term)
            .is_some_and(|held| Arc::ptr_eq(held, observed))
    }

    /// Every live id, as plain data — the liveness sets the board and the
    /// sweeps build.
    pub fn terms(&self) -> Vec<TermId> {
        self.map().keys().copied().collect()
    }

    /// Every live shell with its id — the pump's snapshot. The map lock is
    /// held for the clones and nothing else; whatever happens to the pool
    /// afterwards, these handles stay honest about which shell each one is,
    /// which is what [`Self::remove_exact`] and [`Self::still_holds`] spend.
    pub fn entries(&self) -> Vec<(TermId, HeldTerminal)> {
        let mut out = Vec::new();
        self.entries_into(&mut out);
        out
    }

    /// The same snapshot, refilled into a vector the caller keeps.
    ///
    /// For the one caller that asks sixty times a second: the pump's round
    /// wants a fresh answer, not a fresh allocation, and a window with a
    /// dozen panes was buying a vector per pane per display frame to learn
    /// nothing new about most of them. The map lock still covers the clones
    /// and nothing else — [`Self::entries`]' whole contract — and the buffer
    /// is emptied before it is filled, so a caller can never read a pane that
    /// left the pool before this call.
    pub fn entries_into(&self, out: &mut Vec<(TermId, HeldTerminal)>) {
        out.clear();
        out.extend(
            self.map()
                .iter()
                .map(|(term, held)| (*term, Arc::clone(held))),
        );
    }

    /// Every live shell without its id — the settings retell walk.
    pub fn handles(&self) -> Vec<HeldTerminal> {
        self.map().values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalRegistry, lock_pty};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use zerocode_lane::{PtyHandle, PtyTransport, PtyTransportError};
    use zerocode_pty::{Pumped, Terminal};

    /// How long a test waits for something that must happen at once before
    /// calling it a deadlock. Generous because CI machines stall, and spent
    /// only on failure — every passing run returns the moment the thing
    /// happens. No schedule in these tests depends on real time; the gates
    /// below do the sequencing.
    const DEADLOCK: Duration = Duration::from_secs(10);

    /// A transport whose pump announces itself and then parks until the test
    /// says otherwise — the deterministic stand-in for "this terminal is
    /// mid-flood-parse". Everything else it answers without blocking, and
    /// what is written to it is kept for the test to read back.
    struct FakeTransport {
        terminal: Terminal,
        /// `None` pumps without parking — the quiet terminal beside a flood.
        gate: Option<(Sender<()>, Receiver<()>)>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    struct FakeTerminal {
        pumping: Receiver<()>,
        release: Sender<()>,
    }

    impl FakeTransport {
        /// A terminal whose pump parks, inserted into the pool; the returned
        /// channels start and end the parse, deterministically.
        fn gated(registry: &TerminalRegistry, term: crate::TermId) -> FakeTerminal {
            let (entered, pumping) = channel();
            let (release, may_finish) = channel();
            registry.insert(
                term,
                PtyHandle::new(Self {
                    terminal: Terminal::new(4, 20),
                    gate: Some((entered, may_finish)),
                    written: Arc::new(Mutex::new(Vec::new())),
                }),
            );
            FakeTerminal { pumping, release }
        }

        /// A terminal that never blocks, inserted into the pool.
        fn quiet(registry: &TerminalRegistry, term: crate::TermId) -> Arc<Mutex<Vec<Vec<u8>>>> {
            let written = Arc::new(Mutex::new(Vec::new()));
            registry.insert(
                term,
                PtyHandle::new(Self {
                    terminal: Terminal::new(4, 20),
                    gate: None,
                    written: Arc::clone(&written),
                }),
            );
            written
        }
    }

    impl PtyTransport for FakeTransport {
        fn pump(&mut self) -> Pumped {
            if let Some((entered, may_finish)) = &self.gate {
                let _ = entered.send(());
                let _ = may_finish.recv();
            }
            Pumped {
                bytes: 0,
                ended: false,
                answered: false,
                unanswered_since: None,
            }
        }

        fn terminal(&self) -> &Terminal {
            &self.terminal
        }

        fn terminal_mut(&mut self) -> &mut Terminal {
            &mut self.terminal
        }

        fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError> {
            self.written.lock().unwrap().push(bytes.to_vec());
            Ok(())
        }

        fn resize(&mut self, _rows: u16, _cols: u16) -> Result<(), PtyTransportError> {
            Ok(())
        }

        fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
            Ok(None)
        }

        fn kill(&mut self) -> Result<(), PtyTransportError> {
            Ok(())
        }
    }

    /// Park a pump inside the given terminal's parse, holding that
    /// terminal's lock, until the returned handle is joined.
    fn parked_pump(
        registry: &TerminalRegistry,
        term: crate::TermId,
        gate: &FakeTerminal,
    ) -> std::thread::JoinHandle<()> {
        let held = registry.handle(term).expect("the terminal is in the pool");
        let pump = std::thread::spawn(move || {
            let mut pty = lock_pty(&held);
            let _ = pty.pump();
        });
        gate.pumping
            .recv_timeout(DEADLOCK)
            .expect("the parse never started");
        pump
    }

    /// Run `work` on its own thread and require it to finish promptly while
    /// whatever the caller staged is still standing — the shape of every
    /// deadlock assertion here.
    fn completes_or_deadlock(work: impl FnOnce() + Send + 'static, complaint: &str) {
        let (done, finished) = channel();
        let worker = std::thread::spawn(move || {
            work();
            let _ = done.send(());
        });
        finished.recv_timeout(DEADLOCK).expect(complaint);
        worker.join().expect("the probed work panicked");
    }

    /// The claim the sharding was built for: while terminal 1 is mid-parse —
    /// its own lock held, the way the pump holds it through a 256KB burst —
    /// a keystroke still acquires terminal 2's handle AND its lock and
    /// delivers its bytes. Under the old single pool lock this exact
    /// schedule waited out the parse.
    #[test]
    fn input_reaches_its_terminal_while_another_floods() {
        let registry = Arc::new(TerminalRegistry::default());
        let flooding = FakeTransport::gated(&registry, 1);
        let written = FakeTransport::quiet(&registry, 2);

        let pump = parked_pump(&registry, 1, &flooding);

        let typed_registry = Arc::clone(&registry);
        completes_or_deadlock(
            move || {
                let held = typed_registry.handle(2).expect("terminal 2 is in the pool");
                lock_pty(&held)
                    .write_input(b"echo hi\r")
                    .expect("the fake transport takes input");
            },
            "a keystroke for an idle terminal waited on another terminal's \
             flood parse — the sharding regressed to one lock",
        );
        assert_eq!(
            written.lock().unwrap().as_slice(),
            &[b"echo hi\r".to_vec()],
            "the keystroke was not delivered to its terminal"
        );

        flooding
            .release
            .send(())
            .expect("the parked pump is still waiting");
        pump.join().expect("the pump thread ends");
    }

    /// The map lock is released while a terminal parses: membership
    /// questions, inserts, removals and — critically — an `Arc` clone of the
    /// PARSING terminal's own entry all complete while its terminal lock is
    /// held. This is the deadlock test for lock-order rule 1: if any
    /// registry method held the map lock while touching a terminal lock,
    /// this schedule would hang.
    #[test]
    fn the_map_lock_is_free_while_a_terminal_parses() {
        let registry = Arc::new(TerminalRegistry::default());
        let flooding = FakeTransport::gated(&registry, 7);

        let pump = parked_pump(&registry, 7, &flooding);

        let probed = Arc::clone(&registry);
        completes_or_deadlock(
            move || {
                assert!(probed.contains_key(&7));
                assert_eq!(probed.terms(), vec![7]);
                let _ = FakeTransport::quiet(&probed, 8);
                assert!(probed.remove(&8).is_some());
                // The parsing terminal's own handle clones without waiting
                // for its terminal lock: the map holds the Arc, not the work.
                assert!(probed.handle(7).is_some());
                assert_eq!(probed.entries().len(), 1);
            },
            "a registry operation waited for a terminal lock — the map lock \
             is being held across per-terminal work",
        );

        flooding
            .release
            .send(())
            .expect("the parked pump is still waiting");
        pump.join().expect("the pump thread ends");
    }

    /// The close/respawn race, replayed deterministically: a pump snapshots
    /// an entry, the shell is retired and its id reopened (the float keeps
    /// its id across respawns), and the stale pump then reports the death it
    /// observed. `remove_exact` must refuse — the id's entry is somebody
    /// else now — and the replacement must survive.
    #[test]
    fn exit_removal_takes_only_the_entry_it_observed() {
        let registry = TerminalRegistry::default();
        let _ = FakeTransport::quiet(&registry, 3);
        let observed = registry.handle(3).expect("the first shell is held");
        assert!(registry.still_holds(3, &observed));

        // Retired and reopened between the snapshot and the removal.
        assert!(registry.remove(&3).is_some());
        let _ = FakeTransport::quiet(&registry, 3);
        let replacement = registry.handle(3).expect("the replacement is held");

        assert!(
            !registry.still_holds(3, &observed),
            "a stale handle still counts as the id's current shell"
        );
        assert!(
            !registry.remove_exact(3, &observed),
            "a stale pump removed the replacement with the dead shell's evidence"
        );
        assert!(
            registry.contains_key(&3),
            "the replacement did not survive the stale removal"
        );
        assert!(
            registry.remove_exact(3, &replacement),
            "the entry's own observer could not remove it"
        );
        assert!(!registry.contains_key(&3));
    }

    /// A reused buffer answers exactly what a fresh vector would.
    ///
    /// The pump keeps one across rounds for its capacity, which is only safe
    /// while the refill is a REPLACEMENT: a shell that left the pool between
    /// two rounds must be gone from the answer, or the round would parse a
    /// terminal nobody can address any more and remove a pane on evidence
    /// from a pool that no longer holds it.
    #[test]
    fn a_reused_snapshot_buffer_answers_only_the_current_pool() {
        let registry = TerminalRegistry::default();
        let _ = FakeTransport::quiet(&registry, 1);
        let _ = FakeTransport::quiet(&registry, 2);

        let mut buffer = Vec::new();
        registry.entries_into(&mut buffer);
        let mut seen: Vec<crate::TermId> = buffer.iter().map(|(term, _)| *term).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2]);

        assert!(registry.remove(&1).is_some());
        let capacity = buffer.capacity();
        registry.entries_into(&mut buffer);
        assert_eq!(
            buffer.iter().map(|(term, _)| *term).collect::<Vec<_>>(),
            vec![2],
            "a departed shell survived into the next round's snapshot"
        );
        assert_eq!(
            buffer.capacity(),
            capacity,
            "the refill handed the buffer's capacity back, so every round pays \
             for a fresh one"
        );
    }
}
