//! Which agent a summons should be carried by.
//!
//! `worker-start` takes three words — `--agent`, `--model`, `--effort` — and
//! nothing judges them. Every other launch fact is checked against a measured
//! table before a row is minted: the agent is in the catalog, its CLI takes
//! the dial, its provider still has room. Those checks answer "could this be
//! started"; none of them answers "should this one carry THIS work", and so
//! the answer has been the coordinator's typing. On run-4275 that is how all
//! twenty-two workers were decided.
//!
//! So it is a closed choice, and the set it is closed over is not built here:
//! it is the set the quota gate already assembles to name the agents still
//! holding room when it refuses one
//! (`crate::orchestration::summonable`) — installed, and not at a wall that
//! window would act on. An agent whose quota is spent is not an option,
//! because an option nobody could carry out is not a closed choice; it is a
//! suggestion. One whose spent number is too old to refuse on IS an option:
//! the gate would summon it, so the choice has to be able to name it.
//!
//! **What this window says about an agent, and what it does not.** The criteria
//! carry an agent's id and the room its provider has left, and nothing else.
//! There is no table here of which agent is good at what, for the same reason
//! `agent-list` refuses to keep one: such a table is stale the day a vendor
//! ships, and availability is the only thing a machine can keep honestly. What
//! an agent IS, is the judge's to know — that is the whole reason this is a
//! judgment rather than a lookup.
//!
//! **What the state is.** The shape of the summons, not the summons: the head
//! of its brief, how long the whole brief was, and the three placement facts
//! the ledger already knows. The head is what bands a task — it says what is
//! wanted before it starts listing the constraints it is wanted under — and
//! it is the only text that leaves, cut to the use's cap here as well as at
//! the door, so what a row records beside an answer is what was asked about.
//!
//! **What the state deliberately leaves out** is the agent the coordinator
//! typed. A question that shows the answer somebody already wrote down is not
//! a second opinion; the row's whole value is that the two were arrived at
//! separately.
//!
//! Nothing acts on the answer. [`crate::jev::SUMMON`] offers no mode that
//! applies: the coordinator's own three words summon every worker, and the
//! judgment is a row beside them.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::jev::choice;
use crate::jev::{Cap, SUMMON_BRIEF_CHAR_CAP};

/// The one question's name — the caller's key for reading the answer back.
/// The endpoint never shows a question's name to the model.
const QUESTION: &str = "summon";

/// The words of the question. They are ours: a summons's own text reaches the
/// model as state, never as an instruction.
const INSTRUCTIONS: &str = "An agent is about to be summoned to carry out the work described in `brief`, in a pane of its own. Choose which of the agents offered should be the one. Judge what the work asks for — how hard it is, how much of a codebase it has to hold at once, how many files it will touch, whether it has to measure something and report numbers — against what each agent is, and against how much of its provider's quota this machine has already spent. Every agent offered can be started right now.";

/// What an option says about an agent whose gauge this window has read.
const ROOM_READ: &str = "{agent}. {spent}% of its {window} quota is already spent on this machine.";

/// What an option says about an agent this window has read no gauge for. It
/// is still summonable — unread is not a wall — and saying so is different
/// from claiming it is empty.
const ROOM_UNREAD: &str =
    "{agent}. This machine has read no quota gauge for it, so how much room it has is unknown.";

/// What an option adds about the summonses this ledger has carried for the
/// agent — hindsight the window has for free: which agent its coordinators
/// actually chose, and for what. The count is the ledger's, bounded by
/// retention; the words are the newest task's title.
const HISTORY_SOME: &str = " This window has summoned it {launched} times before; its newest summons here was for: {brief}";

/// What an option adds about an agent this ledger has never summoned.
const HISTORY_NONE: &str = " This window has never summoned it.";

/// How much of the newest task's title an option carries.
pub const SUMMON_RECENT_BRIEF_CHAR_CAP: usize = 160;

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 5] = ["brief", "briefChars", "worktree", "replaces", "task"];

/// The version of the words above. Bump it when any of them changes: a
/// judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to [`crate::jev::rubric_fingerprint`],
/// so changing a word without bumping the version is a red test rather than a
/// quiet drift.
pub const SUMMON_CHOICE_RUBRIC_VERSION: u32 = 2;

/// The fewest options that make a choice. One agent is not a question, and a
/// question asked where there was nothing to decide is a row that says the
/// judgment agreed when it never chose.
pub const FEWEST_OPTIONS: usize = 2;

/// The row's key for why it carries no `agreed` mark
/// ([`crate::jev::summary::AGREED`]).
///
/// A word rather than a flag: `agreed` is left unwritten for more than one
/// reason — a summons the seat itself chose for has no coordinator's word to
/// agree with — and a reader looking at a mark-less row deserves to be told
/// which. The judge needs nothing from it: it counts the rows that carry a
/// mark, so a row without one is already out of every comparison. This is for
/// whoever asks WHY.
pub const NOT_COMPARED_KEY: &str = "notCompared";

/// [`NOT_COMPARED_KEY`]'s word for a summons whose own agent was not among the
/// options — [`SummonAsk::offered`] says whether it was.
///
/// 2026-09-19 18:49 is why the word exists: a row offered five agents and not
/// the codex the summons had landed on, and wrote `agreed: false` — a mismatch
/// with an answer the judgment was never shown. A judgment that could not have
/// named the agent did not disagree about it, and evidence about nothing may
/// not reach the statistics a seat rises on.
pub const NOT_OFFERED: &str = "not_offered";

/// One agent this window could summon this minute, as the quota gate's own
/// look at the machine left it: installed, and not at a wall that gate would
/// refuse on.
///
/// Built by [`crate::orchestration::summonable`] and by nothing else — the
/// same pass that names the agents still holding room when a summons is
/// refused. Counting them a second time here is how the two answers would
/// come out different.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summonable {
    /// The catalog's own id, which is also the option word.
    pub id: String,
    /// How much of its provider's quota is spent, as this window's cache last
    /// read it; `None` when nobody has read a gauge for it.
    pub spent_percent: Option<u8>,
    /// Which window that number describes (`session`, `weekly`, `monthly`),
    /// as the quota table spells it.
    pub window: Option<&'static str>,
    /// How many summonses this ledger has carried for it — every run it still
    /// holds, so the number is bounded by retention and says only "accepted
    /// here that many times", never "good at".
    pub launched: usize,
    /// The title of the task its newest summons here carried, when it
    /// carried one: what this window last found it fit for, in the
    /// coordinator's own words. Cut to [`SUMMON_RECENT_BRIEF_CHAR_CAP`].
    pub recent_brief: Option<String>,
}

impl Summonable {
    /// What this option says about itself.
    fn means(&self) -> String {
        let room = match (self.spent_percent, self.window) {
            (Some(spent), Some(window)) => ROOM_READ
                .replace("{agent}", &self.id)
                .replace("{spent}", &spent.to_string())
                .replace("{window}", window),
            _ => ROOM_UNREAD.replace("{agent}", &self.id),
        };
        let history = match (self.launched, self.recent_brief.as_deref()) {
            (0, _) => HISTORY_NONE.to_string(),
            (launched, brief) => HISTORY_SOME
                .replace("{launched}", &launched.to_string())
                .replace(
                    "{brief}",
                    &crate::jev::door::cut(
                        brief.unwrap_or("a pane summoned with no task"),
                        Cap::Chars(SUMMON_RECENT_BRIEF_CHAR_CAP),
                    ),
                ),
        };
        room + &history
    }
}

/// The shape of one summons — what the ledger can honestly report about the
/// work without handing over the whole of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummonLook<'a> {
    /// The summons' own words, cut to [`SUMMON_BRIEF_CHAR_CAP`] here whether
    /// or not the caller cut them already.
    pub brief: &'a str,
    /// Characters of the WHOLE brief, which the cut throws away: a brief four
    /// times the cap and one that fits are different sizes of ask, and after
    /// the cut they look alike.
    pub brief_chars: usize,
    /// Whether the summons cuts a worktree of its own.
    pub worktree: bool,
    /// Whether it replaces an attempt that already ended.
    pub replaces_an_attempt: bool,
    /// Whether it carries a written task, or is a pane summoned to work with.
    pub carries_a_task: bool,
}

/// One question and the set its answer is judged against — the two travel
/// together so they cannot drift apart at a call site.
#[derive(Debug, Clone, PartialEq)]
pub struct SummonAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
    /// The agents offered, in the order they were offered.
    offered: Vec<String>,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct SummonPick {
    /// The agent id the judgment chose — always one of the offered set.
    pub chosen: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory. The criteria are two
/// SENTENCE SHAPES rather than sentences — each option fills one with its own
/// id and numbers — so the fingerprint covers the wording and not the machine
/// that happened to be asked about.
#[must_use]
pub fn rubric_words() -> String {
    [
        INSTRUCTIONS,
        ROOM_READ,
        ROOM_UNREAD,
        HISTORY_SOME,
        HISTORY_NONE,
        &STATE_KEYS.join(","),
    ]
    .join("\n")
}

/// This summons' brief, shaped to the use's own cap
/// ([`crate::jev::brief_shape`]).
#[must_use]
pub fn brief_shape(brief: &str) -> (String, usize) {
    crate::jev::brief_shape(brief, Cap::Chars(SUMMON_BRIEF_CHAR_CAP))
}

/// The question this summons asks of the agents it could actually start.
///
/// `None` when fewer than [`FEWEST_OPTIONS`] agents are summonable: a choice
/// between one thing is not a choice, and a request that carries one would
/// buy a row saying the judgment agreed with a decision it never made.
#[must_use]
pub fn ask(look: &SummonLook<'_>, summonable: &[Summonable]) -> Option<SummonAsk> {
    if summonable.len() < FEWEST_OPTIONS {
        return None;
    }
    let mut criteria = Map::new();
    let mut offered = Vec::new();
    for agent in summonable {
        criteria.insert(agent.id.clone(), Value::from(agent.means()));
        offered.push(agent.id.clone());
    }
    let state = Value::Object(Map::from_iter([
        (
            "brief".to_string(),
            Value::from(crate::jev::door::cut(
                look.brief,
                Cap::Chars(SUMMON_BRIEF_CHAR_CAP),
            )),
        ),
        ("briefChars".to_string(), Value::from(look.brief_chars)),
        ("worktree".to_string(), Value::from(look.worktree)),
        (
            "replaces".to_string(),
            Value::from(look.replaces_an_attempt),
        ),
        ("task".to_string(), Value::from(look.carries_a_task)),
    ]));
    let questions = choice::asked(QUESTION, INSTRUCTIONS, criteria);
    Some(SummonAsk {
        state,
        questions,
        offered,
    })
}

impl SummonAsk {
    /// The agents this question offered, in order.
    #[must_use]
    pub fn options(&self) -> &[String] {
        &self.offered
    }

    /// Whether `agent` was one of them — asked of the agent a summons really
    /// landed on, before a row may say the two answers agreed or did not.
    ///
    /// The answer is `false` where the two halves of a summons disagree about
    /// what this machine can carry: a row then says [`NOT_OFFERED`] under
    /// [`NOT_COMPARED_KEY`] instead of a mark.
    #[must_use]
    pub fn offered(&self, agent: &str) -> bool {
        self.offered.iter().any(|offered| offered == agent)
    }

    /// What `answers` says about the question that was asked.
    ///
    /// # Errors
    ///
    /// [`crate::jev::choice::ChoiceRefusal`] names the first rule the answer
    /// broke. An agent this machine has but this question did not offer — one
    /// at its wall — is already refused by the reader, which is judged
    /// against the offered set and nothing wider.
    pub fn read(&self, answers: &Value) -> Result<SummonPick, crate::jev::choice::ChoiceRefusal> {
        let offered: BTreeSet<String> = self.offered.iter().cloned().collect();
        let choice = crate::jev::choice::read(answers, QUESTION, &offered)?;
        Ok(SummonPick {
            chosen: choice.chosen,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
        })
    }
}

#[cfg(test)]
mod tests;
