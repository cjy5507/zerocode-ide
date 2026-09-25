//! A seat beside recall for something that wants to see what a turn is about to
//! read — and, in the one mode a person switches on, to settle the order it
//! reads it in.

use core_types::{ConversationMessage, MemoryHit};

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
    /// `attempt` belongs to the turn; asynchronous evidence must keep this
    /// identity rather than borrowing another turn in the same project.
    fn settle(&self, attempt: &str, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit>;

    /// What the turn did since the seat last heard of it ([`TurnProgress`]),
    /// told by the runtime at each request's boundary and once more when the
    /// turn ends. A seat that grades nothing ignores it.
    fn observe(&self, _attempt: &str, _progress: TurnProgress<'_>) {}
}

/// What a turn did after a recall, as the runtime hands it to the seat while
/// the turn goes (t-6264).
///
/// The seat learns it from the runtime and not from the transcript at the
/// turn's end, because by then the transcript may no longer hold it: a
/// compaction — mid-turn, or the one after the turn's last answer — replaces
/// what the turn read with a summary, and a request that failed takes its
/// messages back. So each boundary hands over what the turn appended since the
/// last one, before any compaction can take it.
#[derive(Debug, Clone, Copy)]
pub struct TurnProgress<'a> {
    /// What the turn appended since the seat last heard.
    pub appended: &'a [ConversationMessage],
    /// Whether the request that carried the seat's last recall was answered:
    /// the model was sent what the recall put in front of it and replied. A
    /// request the context budget refused, one a gateway turned away, one
    /// whose stream failed, and one whose reply was taken back showed the
    /// model nothing.
    pub answered: bool,
    /// Whether the turn is over and ended on its own terms — the model's
    /// answer, or a budget that closed it — so that what the seat has heard is
    /// the whole of it. A turn that failed or was cancelled never says so.
    pub ended: bool,
}
