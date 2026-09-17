//! A seat beside recall for something that wants to see what a turn is about to
//! read, without being able to change it.

use core_types::MemoryHit;

/// Sees every recall a turn performs — the query, and the hits in the order
/// recall settled — after recall has decided and before anything renders.
///
/// The hits are borrowed and the return is unit: an observer can record them,
/// compare them, or put them to a judgment somewhere else, but it cannot
/// reorder, drop, or add one. What the turn reads is exactly what it would
/// have read with no observer seated. It runs on recall's own blocking thread,
/// so anything slow has to be handed off rather than done here.
pub trait RecallObserver: Send + Sync {
    fn observe(&self, query: &str, hits: &[MemoryHit]);
}
