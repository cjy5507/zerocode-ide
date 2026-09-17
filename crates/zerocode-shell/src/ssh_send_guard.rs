//! What a `zerocode-ssh send` owns around the line it types on.
//!
//! The decisions about the line itself — may the words go in, may the Enter
//! follow — are no longer here. They are made where the write happens, by the
//! delivery the pump turns ([`zerocode_pty::ready::Guard`]), because a
//! decision made here and acted on by the pump a readiness wait later was a
//! decision about a line that had since moved (integration review
//! 2026-09-05, items 1 and 2). What stays is what the door still owns around
//! that delivery: the receipt slot it may claim, and the lease that keeps two
//! sends at one pane from crossing.
//!
//! The reason any of it exists is one mistake worth naming: the door used to
//! protect a person's draft by refusing to send CLEAR keys, and that is not
//! protection. It stops the words being erased and does nothing about the
//! other half — a paste lands on them and the Enter submits them together.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::TermId;

/// Take this call's own prompt-submit receipt slot, or none at all.
///
/// Admission and installation in one step over the map, because asking and
/// then taking are two moments and a launch can claim the seat in between —
/// which is how a send came to overwrite the receipt channel a worker
/// mid-birth was waiting on. The answer doubles as the ownership record: a
/// caller releases the slot only if this handed it one.
pub(crate) fn claim_submit_receipt(
    slots: &mut HashMap<TermId, std::sync::mpsc::SyncSender<()>>,
    pane: TermId,
) -> Option<std::sync::mpsc::Receiver<()>> {
    match slots.entry(pane) {
        std::collections::hash_map::Entry::Occupied(_) => None,
        std::collections::hash_map::Entry::Vacant(seat) => {
            let (notify, waiting) = std::sync::mpsc::sync_channel(1);
            seat.insert(notify);
            Some(waiting)
        }
    }
}

/// One send at a time per pane.
///
/// The pane's prompt queue already orders the deliveries themselves; what it
/// does not order is the receipt around each one — the submit slot claimed
/// before the delivery and the acknowledgement waited on after it — and two
/// sends racing at one pane would read each other's. Keyed rather than
/// global: two panes have nothing to do with each other.
///
/// Bounded the same way the human-input tracker is, and for the same reason:
/// a window's terminals come and go, and a map keyed by a growing id has to
/// have a ceiling somewhere. A lease nobody holds is safe to drop.
pub(crate) fn send_lease(pane: TermId) -> Arc<tokio::sync::Mutex<()>> {
    static LEASES: OnceLock<std::sync::Mutex<HashMap<TermId, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    const TRACKED_PANES: usize = 256;
    let leases = LEASES.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut held = leases
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if held.len() >= TRACKED_PANES {
        held.retain(|term, lease| *term == pane || Arc::strong_count(lease) > 1);
    }
    Arc::clone(held.entry(pane).or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The launch's receipt slot is not this call's to take or to give back.
    #[test]
    fn a_receipt_slot_somebody_else_owns_is_neither_taken_nor_released() {
        let mut slots: HashMap<TermId, std::sync::mpsc::SyncSender<()>> = HashMap::new();

        // An empty seat: this call owns what it installs, and the sender left
        // behind is the one that will answer it.
        let mine = claim_submit_receipt(&mut slots, 33).expect("a free seat");
        assert!(slots.contains_key(&33));
        slots
            .get(&33)
            .expect("the installed sender")
            .send(())
            .expect("the slot this call installed answers this call");
        assert!(mine.try_recv().is_ok());

        // A launch already owns this seat: nothing is taken, and — the point —
        // the launch's own sender is left exactly where it was.
        let (launch, launch_waiting) = std::sync::mpsc::sync_channel(1);
        let mut occupied: HashMap<TermId, std::sync::mpsc::SyncSender<()>> = HashMap::new();
        occupied.insert(34, launch);
        assert!(
            claim_submit_receipt(&mut occupied, 34).is_none(),
            "a send stole the receipt channel a worker mid-birth is waiting on"
        );
        occupied
            .get(&34)
            .expect("the launch's sender")
            .send(())
            .expect("the launch slot survived");
        assert!(
            launch_waiting.try_recv().is_ok(),
            "the launch's own receiver stopped hearing its slot"
        );

        // A newer replacement in the same seat is somebody else's too.
        let (newer, _newer_waiting) = std::sync::mpsc::sync_channel(1);
        occupied.insert(34, newer);
        assert!(claim_submit_receipt(&mut occupied, 34).is_none());
        assert!(occupied.contains_key(&34));
    }

    /// Two sends at one pane are one after the other, and two panes are not
    /// each other's business.
    ///
    /// The interleaving this prevents is A's text, B's text, A's Enter — which
    /// submits B's line and leaves B holding A's.
    #[tokio::test]
    async fn two_sends_at_one_pane_do_not_climb_into_each_other() {
        let first = send_lease(35).lock_owned().await;

        // The same pane waits. Asked without blocking, because a test that
        // parked here would have nothing to say when the answer is wrong.
        assert!(
            send_lease(35).try_lock_owned().is_err(),
            "a second send began while the first was mid-transaction"
        );
        // A different pane is a different line.
        assert!(send_lease(36).try_lock_owned().is_ok());

        drop(first);
        assert!(
            send_lease(35).try_lock_owned().is_ok(),
            "the lease outlived the send that held it"
        );
    }
}
