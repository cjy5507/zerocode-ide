//! A seat beside recall for something that wants to see what a turn is about to
//! read — and, in the one mode a person switches on, to settle the order it
//! reads it in.

use core_types::MemoryHit;

/// Sees every recall a turn performs — the query, and the hits in the order
/// recall settled — after recall has decided and before anything renders, and
/// answers with the order the turn reads.
///
/// The seat is handed the hits and hands hits back, so it may REORDER them, and
/// it may hand back FEWER: a hit it can say a turn is better off not reading is
/// one it may leave out. What it may never do is add one, or return a hit recall
/// did not hand it — that would answer a question recall was not asked. The one
/// seat there is drops a hit only on the ground its own contract names, and
/// proves the remainder against what it was given before it hands anything back
/// (`crate::memory::rerank`'s bottom level, and `apply_order`).
///
/// A seat with nothing to say returns what it was given, which is exactly what
/// the turn would have read with no seat here at all (`smart.rerankShadow` in a
/// record-only mode does precisely that).
///
/// It runs on recall's own blocking thread, so anything slow has to be handed
/// off — or, when the mode is one that acts, bounded by a wall the seat owns.
pub trait RecallSeat: Send + Sync {
    fn settle(&self, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit>;
}
