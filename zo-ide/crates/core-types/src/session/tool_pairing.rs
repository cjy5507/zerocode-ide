//! Which tool call a tool result answers, when a provider reuses call ids.
//!
//! A result names its call only by id. Anthropic and OpenAI mint ids that are
//! unique for the conversation, so "the call with this id" is one call. Gemini
//! does not: on 2026-09-11 it issued `call_18194` to a `bash` call and, 36 turns
//! later, to a `read_file` call. With repeats, "answered" is a property of each
//! OCCURRENCE of an id, not of the id — and two places have to agree on which
//! occurrence each result answers: the orphan seal (which occurrences get a
//! synthetic result) and the Gemini wire (which wire id each `functionResponse`
//! carries). This is the one rule both walk.
//!
//! The rule: a result answers the earliest still-unanswered call with its id
//! in the NEWEST message that made a call with that id. A newer message calling
//! the same id abandons the older message's unanswered calls — results follow
//! the call they answer, so an answer after the newer call is the newer call's.
//! Inside one message, results answer its calls in call order (the order zo
//! runs and records them).

use std::collections::HashMap;

/// A walk over a history's calls and results, in history order.
///
/// The caller numbers its own call occurrences (any index into its own table)
/// and hands them in through [`Self::call`]; [`Self::result`] says which of
/// those numbers a result answers.
#[derive(Debug, Default)]
pub struct ToolCallPairing<'a> {
    open: HashMap<&'a str, OpenCalls>,
}

/// The calls one id has in the newest message that called it.
#[derive(Debug)]
struct OpenCalls {
    /// The message those calls are in.
    message: usize,
    /// The caller's occurrence numbers, in call order.
    calls: Vec<usize>,
    /// How many of `calls` are already answered (they are answered in order).
    answered: usize,
}

impl<'a> ToolCallPairing<'a> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A call with `id` in message `message`, numbered `occurrence` by the caller.
    pub fn call(&mut self, message: usize, id: &'a str, occurrence: usize) {
        let open = self.open.entry(id).or_insert(OpenCalls {
            message,
            calls: Vec::new(),
            answered: 0,
        });
        if open.message != message {
            // A newer message reused the id: the older calls' answers, if any
            // were still coming, can no longer be told apart from this one's.
            *open = OpenCalls {
                message,
                calls: Vec::new(),
                answered: 0,
            };
        }
        open.calls.push(occurrence);
    }

    /// The occurrence a result for `id` answers, or `None` when no call with
    /// that id is waiting for one (a result with no call, or one result too many).
    pub fn result(&mut self, id: &str) -> Option<usize> {
        let open = self.open.get_mut(id)?;
        let occurrence = *open.calls.get(open.answered)?;
        open.answered += 1;
        Some(occurrence)
    }
}

#[cfg(test)]
mod tests {
    use super::ToolCallPairing;

    #[test]
    fn unique_ids_pair_the_one_call_each_names() {
        let mut pairing = ToolCallPairing::new();
        pairing.call(1, "a", 0);
        pairing.call(1, "b", 1);
        assert_eq!(pairing.result("b"), Some(1));
        assert_eq!(pairing.result("a"), Some(0));
        assert_eq!(pairing.result("a"), None, "one answer per call");
        assert_eq!(pairing.result("never-called"), None);
    }

    #[test]
    fn a_newer_message_reusing_an_id_takes_the_answers_that_follow_it() {
        let mut pairing = ToolCallPairing::new();
        pairing.call(1, "x", 0);
        pairing.call(3, "x", 1);
        assert_eq!(pairing.result("x"), Some(1));
        assert_eq!(pairing.result("x"), None, "the abandoned call gets nothing");
    }

    #[test]
    fn one_message_repeating_an_id_is_answered_in_call_order() {
        let mut pairing = ToolCallPairing::new();
        pairing.call(1, "x", 0);
        pairing.call(1, "x", 1);
        assert_eq!(pairing.result("x"), Some(0));
        assert_eq!(pairing.result("x"), Some(1));
    }
}
