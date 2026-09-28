//! Which connection a Jev question rides.
//!
//! Two programs ask Jev: the window (`crates/zerocode-shell/src/systemone.rs`)
//! and zo (`zo-ide/crates/api/src/systemone.rs`). They are separate cargo
//! workspaces on two `reqwest` majors, so they cannot share a client — only
//! the rule for when a client is kept and when it is let go, which is this
//! crate.
//!
//! A question that got a response, whatever its status, leaves its client and
//! the connection it pooled for the next question: a response proves the
//! socket carries. A question that ended with nothing from the server — its
//! deadline passed, or its socket broke — lets that client go, pool and all,
//! so the next question opens a connection of its own.
//!
//! HTTP/2 is why this has to be said. A stream that runs out of time is reset
//! and its connection stays pooled, and the pool hands that one connection to
//! every question behind it, often enough that it never idles out. A
//! connection pinned to a server that had stopped answering then answered
//! nothing for a whole run: R11 q8 (2026-09-29) asked 58 questions and heard
//! none, while new connections to the same service answered two or three times
//! in five. HTTP/1.1 already closes a connection whose request was dropped;
//! there the rule changes nothing.
//!
//! Nothing here asks again. A question that went unanswered stays unanswered,
//! and its seat's stand-in stands; the rule changes only which connection the
//! next question rides.

use std::sync::{Mutex, PoisonError};

/// The client one program asks Jev through: built once and kept while its
/// questions are answered, let go after one that was not.
pub struct Socket<C> {
    build: fn() -> Option<C>,
    held: Mutex<Held<C>>,
}

/// What a socket holds right now.
struct Held<C> {
    /// `None` before the first question, and while no client could be built.
    client: Option<C>,
    /// How many clients this socket has let go.
    generation: u64,
}

/// One question's hold on the client: the client itself, and which of the
/// socket's clients it is — so a question that ends late cannot let go of the
/// client that already replaced its own.
pub struct Lent<C> {
    pub client: C,
    generation: u64,
}

impl<C: Clone> Socket<C> {
    /// A socket whose clients `build` makes. `None` from it is a client that
    /// could not be made, and the next question tries again.
    #[must_use]
    pub const fn new(build: fn() -> Option<C>) -> Self {
        Self {
            build,
            held: Mutex::new(Held {
                client: None,
                generation: 0,
            }),
        }
    }

    /// The client the next question rides: the one held, or — before the
    /// first question — one built now. `None` only when none can be built.
    #[must_use]
    pub fn lend(&self) -> Option<Lent<C>> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.client.is_none() {
            held.client = (self.build)();
        }
        Some(Lent {
            client: held.client.clone()?,
            generation: held.generation,
        })
    }

    /// A question on `lent` ended with nothing from the server: let its client
    /// go, and build the one the next question rides now — at the end of the
    /// question that failed, so the build is never spent inside the next
    /// question's deadline.
    ///
    /// `false`, and nothing done, when `lent`'s client is no longer the one
    /// held: another unanswered question let it go first, and the client
    /// standing now has failed no one.
    pub fn unanswered(&self, lent: &Lent<C>) -> bool {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.generation != lent.generation {
            return false;
        }
        held.generation = held.generation.wrapping_add(1);
        held.client = (self.build)();
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A client that is only a number: the order it was built in, counted
    /// across the process, so two clients are told apart by value.
    fn numbered() -> Option<u64> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Some(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn an_answered_question_leaves_its_client_for_the_next() {
        let socket = Socket::new(numbered);
        let first = socket.lend().expect("a client");
        let second = socket.lend().expect("a client");
        assert_eq!(first.client, second.client, "built once, and kept");
    }

    #[test]
    fn the_question_after_an_unanswered_one_rides_a_new_client() {
        let socket = Socket::new(numbered);
        let first = socket.lend().expect("a client");
        assert!(
            socket.unanswered(&first),
            "the client that failed is let go"
        );
        let next = socket.lend().expect("a client");
        assert_ne!(
            next.client, first.client,
            "the next question rides a client of its own"
        );
        assert_eq!(
            socket.lend().expect("a client").client,
            next.client,
            "and the new client is kept in turn"
        );
    }

    /// Two questions ride one client and both go unanswered: the first to
    /// end lets it go, and the second, ending after the replacement stands,
    /// leaves the replacement alone — it has failed no one.
    #[test]
    fn a_late_unanswered_question_leaves_the_client_that_replaced_its_own() {
        let socket = Socket::new(numbered);
        let early = socket.lend().expect("a client");
        let beside = socket.lend().expect("the same client");
        assert!(socket.unanswered(&early));
        let replacement = socket.lend().expect("a client");
        assert!(
            !socket.unanswered(&beside),
            "a late failure let go of the client that replaced its own"
        );
        assert_eq!(socket.lend().expect("a client").client, replacement.client);
    }

    /// The replacement is built where the question failed; the next question
    /// only takes it.
    #[test]
    fn the_replacement_is_built_when_the_question_fails_not_when_the_next_asks() {
        static BUILT: AtomicU64 = AtomicU64::new(0);
        fn counted() -> Option<u64> {
            Some(BUILT.fetch_add(1, Ordering::Relaxed))
        }
        let socket = Socket::new(counted);
        let first = socket.lend().expect("a client");
        assert!(socket.unanswered(&first));
        let built = BUILT.load(Ordering::Relaxed);
        assert_eq!(built, 2, "the first client, then its replacement at once");
        let _next = socket.lend().expect("the replacement");
        assert_eq!(
            BUILT.load(Ordering::Relaxed),
            built,
            "the next question built nothing"
        );
    }

    /// A client that could not be built is not a client that never will be:
    /// the next question tries again.
    #[test]
    fn a_client_that_could_not_be_built_is_tried_again_by_the_next_question() {
        static TRIED: AtomicU64 = AtomicU64::new(0);
        fn fails_once() -> Option<u64> {
            (TRIED.fetch_add(1, Ordering::Relaxed) > 0).then_some(1)
        }
        let socket = Socket::new(fails_once);
        assert!(socket.lend().is_none(), "nothing to lend yet");
        assert_eq!(socket.lend().map(|lent| lent.client), Some(1));
    }
}
