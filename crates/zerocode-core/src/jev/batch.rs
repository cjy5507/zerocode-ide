//! One request that judges many things at once (t-32796): the road a seat
//! takes when it holds several INDEPENDENT items to judge — a coordinator's
//! batch of letters (`crate::mail_triage`), later a batch of finished
//! workers' results — and asking a model about them one by one would give back
//! everything asking them side by side buys.
//!
//! One Jev request carries one state and a map of questions over it, and the
//! state is charged once however many questions read it: input tokens only,
//! output free (`docs.typesafe.ai/models`; the same fact a guard question
//! beside a seat's own rides on, [`crate::jev::noul`]). So what every item
//! shares — the facts that hold for the whole batch — stands once in the
//! state; what each item is stands once, as an entry of a list under the
//! seat's [`Judgment::items_key`]; and each item is asked its own closed
//! questions by its place in that list (`letters[3]`), the way every seat
//! that asks about a list does ([`crate::browser_read`],
//! [`crate::jev::claim`]).
//!
//! **The words of the judgment do not stand in the state.** A question is
//! judged on its own words — its instructions and the descriptions of its
//! options, which the endpoint takes whole with each question and cannot share
//! between them (`docs.typesafe.ai/primitives/choice`) — and a rule left in
//! the state is read as data. The mail triage first said its explanation of the
//! fields and its options' meanings once in the state, a line of each option in
//! the questions, and agreed with what the coordinator did next in 192 of 406
//! comparisons, where the per-letter road agreed in 216 and the same words in
//! each letter's question in 218 (the real model, 2026-10-04). A seat on this
//! road pays its words for each item: what the road saves is the requests, and
//! the wait for them, not the words.
//!
//! **Stage one: independent verdicts.** A request of this road asks nothing
//! BETWEEN items. Each question is built from one item's place and the words
//! the seat shares, never from another item's answer — no answer exists when
//! the request is built, and [`Judgment::questions`] is handed none — and
//! each item is read through [`Answers`], a view of the reply that holds that
//! item's own questions and no others, so one item's verdict can be read
//! neither from nor into its neighbour's. A reply that breaks the closed
//! answer's rules for one item is that item's refusal and nobody else's.
//!
//! **Stage two** — a comparison across items: which of two results is
//! better, which of several is a duplicate — needs the others' verdicts, so
//! it is a SECOND request built from the first's answers over the survivors
//! only, never one more question of the first. Nothing here asks it.
//!
//! The cap ([`Judgment::cap`]) is the seat's, named with its reason where the
//! seat names its numbers; above it the items are cut EVENLY
//! ([`super::shard`]) and the requests are meant to leave side by side, a few
//! at a time, so a batch waits for the slowest request of each wave and not
//! for its requests one after another.
//!
//! **The seam for the next seat.** The review of a finished worker's result
//! is the second user of this road (t-32796 phase 2): its deterministic
//! checks (t-26587) settle what code can settle first — a failing check is a
//! verdict nobody asks a model for — and the items left, their evidence as
//! facts and the task's own words as the shared part, ride one request, each
//! read as a verdict with a confidence against that seat's own floor, which
//! is what says pass or redo. That seat implements [`Judgment`] and nothing
//! else here changes. It is not built here: the checks it waits for are not
//! landed.

use std::ops::Range;

use serde_json::{Map, Value};

use super::shard;

/// What every question name of a batch opens with: a letter, then the item's
/// place in the WHOLE batch, then — for every question but the item's first —
/// a separator and the question's suffix. One name per question, unique across
/// every request of one batch, and the place an answer is read back by.
const QUESTION_PREFIX: &str = "q";

/// What stands between an item's place and a suffix, so that a name reads back
/// one way only: the digits up to the first separator are the place. Without
/// it a suffix that starts with a digit would make another item's name (`q1`
/// and `2` are `q12`, item 12's own).
const SUFFIX_SEPARATOR: char = '/';

/// The name of the question `suffix` of the item at `item`, counted from the
/// first item of the whole batch — across its requests, not within one. The
/// item's first question has no suffix (`q3`); the others have a word of their
/// own (`q3/urgent`).
#[must_use]
pub fn question_name(item: usize, suffix: &str) -> String {
    if suffix.is_empty() {
        format!("{QUESTION_PREFIX}{item}")
    } else {
        format!("{QUESTION_PREFIX}{item}{SUFFIX_SEPARATOR}{suffix}")
    }
}

/// A seat's side of the batch road: what it shares, what it asks of each
/// item, and how it reads one item's answers.
pub trait Judgment {
    /// What one item's answers come to, once read in shape.
    type Verdict;
    /// Why one item's answers are no verdict — the closed word its row keeps.
    type Refusal;

    /// The most items one request carries: the seat's named cap.
    fn cap(&self) -> usize;

    /// The key of the state the items' facts stand under, in order.
    fn items_key(&self) -> &'static str;

    /// The state every item shares — the facts that hold for the whole batch,
    /// never the words of the judgment, which stand in each item's questions —
    /// written once into each request, beside the items.
    fn shared(&self) -> Map<String, Value>;

    /// One item's questions, with the words of the judgment in each, under a
    /// suffix of their own: none for the first, a word for each of the others,
    /// never the same twice for one item.
    /// `at` is the item's place in THIS request's list of items, which the
    /// questions' words call it by (`numbers[2]`) — the questions' NAMES are
    /// made from its place in the whole batch; `at` is all a question is built
    /// from.
    fn questions(&self, at: usize) -> Vec<(&'static str, Value)>;

    /// What one item's own answers say.
    ///
    /// # Errors
    ///
    /// The seat's refusal for an answer that breaks a rule of its closed
    /// answer: the item is refused and no other.
    fn read(&self, answers: &Answers<'_>) -> Result<Self::Verdict, Self::Refusal>;
}

/// One item's answers, and no other item's: the view of a reply a seat reads
/// an item through, so a reading cannot see a neighbour's verdict.
#[derive(Debug, Clone, Copy)]
pub struct Answers<'a> {
    all: &'a Value,
    item: usize,
}

impl<'a> Answers<'a> {
    /// The reply's answer to this item's question `suffix`, when it gave one.
    #[must_use]
    pub fn get(&self, suffix: &str) -> Option<&'a Value> {
        self.all.get(question_name(self.item, suffix))
    }
}

/// One request of a batch: the state and the questions that leave, and which
/// items they ask about.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The request's `state`: the seat's shared part, and the items' facts
    /// under the seat's [`Judgment::items_key`].
    pub state: Value,
    /// The request's `questions`: every item's own, under [`question_name`].
    pub questions: Value,
    /// The items this request asks about, by their place in the whole batch.
    items: Range<usize>,
}

impl Request {
    /// The items this request asks about, by their place in the whole batch,
    /// in the order its state lists them.
    #[must_use]
    pub fn items(&self) -> Range<usize> {
        self.items.clone()
    }

    /// What a reply's `answers` say about each item this request asked
    /// about, in order — each read by the seat from its own answers alone.
    #[must_use]
    pub fn read<J: Judgment>(
        &self,
        judgment: &J,
        answers: &Value,
    ) -> Vec<Result<J::Verdict, J::Refusal>> {
        self.items
            .clone()
            .map(|item| judgment.read(&Answers { all: answers, item }))
            .collect()
    }
}

/// The requests `facts` — one entry per item, in order — are judged in: the
/// items cut evenly into requests of at most the seat's cap, each carrying
/// the seat's shared part once and its own items' facts, and every item's
/// questions.
#[must_use]
pub fn requests<J: Judgment>(judgment: &J, facts: Vec<Value>) -> Vec<Request> {
    let mut facts = facts.into_iter();
    shard::even_shards(facts.len(), judgment.cap())
        .into_iter()
        .map(|range| {
            let mut state = judgment.shared();
            state.insert(
                judgment.items_key().to_string(),
                Value::Array(facts.by_ref().take(range.len()).collect()),
            );
            let mut questions = Map::new();
            for (at, item) in range.clone().enumerate() {
                for (suffix, question) in judgment.questions(at) {
                    questions.insert(question_name(item, suffix), question);
                }
            }
            Request {
                state: Value::Object(state),
                questions: Value::Object(questions),
                items: range,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
