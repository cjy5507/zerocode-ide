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
//! carry an agent's id, the room its provider has left, and this ledger's own
//! record of it — how many times the coordinators here chose it, what became
//! of that work, how long it took, and what its newest summons was for
//! ([`AgentRecord`]). There is still no table of which agent is good at what,
//! for the same reason `agent-list` refuses to keep one: such a table is
//! stale the day a vendor ships. A record is the opposite of that table —
//! nobody types it, it is derived from the summonses that happened, and it
//! says "this window did this" and never "this agent is that". What an agent
//! IS, is the judge's to know; what this machine has SEEN of it is the
//! judge's to be told, because on 2026-09-21 it was not: eleven of seventeen
//! disagreements named an agent this ledger had never summoned at all.
//!
//! **What the state is.** The shape of the summons, not the summons: the head
//! of its brief, how long the whole brief was, the three placement facts the
//! ledger already knows, and the task's own attempt history. The head is what
//! bands a task — it says what is wanted before it starts listing the
//! constraints it is wanted under — and it is the only text that leaves, cut
//! to the use's cap here as well as at the door, so what a row records beside
//! an answer is what was asked about. Which is also why the head is taken
//! from the TASK and not from the summons' prompt where there is a task
//! ([`crate::orchestration`]'s `summon_words`): a prompt opens with the
//! house's standing rules, and on the later half of 2026-09-21 those rules
//! filled all 1,200 characters of the cap, so the question was asked about
//! work it had not been shown.
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
const INSTRUCTIONS: &str = "An agent is about to be summoned to carry out the work described in `brief`, in a pane of its own. Choose which of the agents offered should be the one. Judge what the work asks for — how hard it is, how much of a codebase it has to hold at once, how many files it will touch, whether it has to measure something and report numbers, and whether earlier attempts on it already failed — against what each agent is, and against how much of its provider's quota this machine has already spent. Every agent offered can be started right now. Each option also carries this machine's own record of that agent: how often this window summoned it, how much of that work reached `worker_done`, and how long it took. That record is the only measured evidence here about how an agent actually does, and how much of it there is counts as much as what it says: a record built on a handful of summonses is weak evidence about the next one, however clean it looks, while one built on hundreds is strong. An agent with no record has not been shown to carry work like this, which is not the same as having been shown to.";

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
///
/// The count is said AGAINST the offered set's own total, not alone. A bare
/// "16 times" reads as experience and a bare "266 times" reads as more of
/// the same; "16 of 536" and "266 of 536" are the sixteen-fold difference
/// they actually are. Measured: with the counts bare, the replay named a
/// five-summons agent over a 266-summons one on fifteen of fifty-four rows
/// (t-5873).
const HISTORY_SOME: &str = " This window has summoned it {launched} of the {between} times it summoned any of these agents; its newest summonses here were for: {briefs}";

/// What an option adds about an agent this ledger has never summoned.
///
/// It says the absence, rather than leaving a silent option to read as a
/// clean slate. 2026-09-21 is why: `qwen-code` had carried nothing on this
/// machine and the judgment named it for eleven of seventeen disagreements,
/// against a `claude` this ledger had summoned 265 times.
const HISTORY_NONE: &str =
    " This window has never summoned it, so nothing this machine measured says how it does.";

/// What an option adds about how the work this ledger gave the agent WENT —
/// the other half of the hindsight: a count of summonses says the
/// coordinators kept choosing it, and says nothing about what came back.
///
/// Only the summonses whose work has ENDED are in it. An open dispatch has
/// no outcome yet, and folding it in either direction would be a number
/// about nothing.
const RECORD_SOME: &str = " Of the {carried} of those whose work has ended, {finished} reached worker_done, a median {minutes} minutes of work each.";

/// What an option says where an agent's summonses here carried no written
/// task at all — a pane somebody opened to work with by hand.
const NO_TASK: &str = "panes summoned with no task";

/// What an option adds about an agent this ledger has summoned but whose
/// work has not ended — a first summons still running is not a record.
const RECORD_PENDING: &str =
    " None of that work has ended yet, so how it goes is still unmeasured here.";

/// How much of a newest task's title an option carries.
pub const SUMMON_RECENT_BRIEF_CHAR_CAP: usize = 160;

/// How many of an agent's newest tasks an option names.
///
/// One was not enough. What the coordinators here actually follow is a rule
/// about the KIND of work — measuring goes to one agent, designing to
/// another, a small errand to a third — and a single title is an anecdote
/// where three are a pattern. Measured over the replay (t-5873): one title
/// left thirteen of thirty-three rows naming a five-summons agent for work
/// this window has always given a two-hundred-summons one.
pub const SUMMON_RECENT_BRIEFS: usize = 3;

/// What separates the titles an option lists.
const BRIEF_SEPARATOR: &str = "; ";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 7] = [
    "brief",
    "briefChars",
    "worktree",
    "replaces",
    "task",
    "attempts",
    "failures",
];

/// The version of the words above. Bump it when any of them changes: a
/// judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to [`crate::jev::rubric_fingerprint`],
/// so changing a word without bumping the version is a red test rather than a
/// quiet drift.
pub const SUMMON_CHOICE_RUBRIC_VERSION: u32 = 3;

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
    /// What this ledger's own dealings with the agent came to — built by
    /// [`records`] and by nothing else.
    pub record: AgentRecord,
}

impl Summonable {
    /// What this option says about itself, against the offered set's own
    /// total of summonses ([`HISTORY_SOME`]).
    fn means(&self, between: usize) -> String {
        let room = match (self.spent_percent, self.window) {
            (Some(spent), Some(window)) => ROOM_READ
                .replace("{agent}", &self.id)
                .replace("{spent}", &spent.to_string())
                .replace("{window}", window),
            _ => ROOM_UNREAD.replace("{agent}", &self.id),
        };
        room + &self.record.means(between)
    }
}

/// One summons this ledger carried, flattened to the facts [`records`] reads:
/// which agent it went to, when its pane opened, and what became of the work
/// it was given.
///
/// Public, and `serde`, because the arithmetic has two feeders and may have
/// only one copy of itself: the window's own walk over the ledger
/// ([`crate::orchestration::summonable`]), and the replay that reads the same
/// rows out of the authority store to measure a rubric against summonses
/// that already happened (`tools/summon-replay`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarriedSummons {
    /// The agent the summons landed on.
    pub agent: String,
    /// When its pane opened — what orders "newest".
    pub started_ms: i64,
    /// When the work it was given ended, while it has. `None` is a dispatch
    /// still open, and a summons that carried no task at all.
    pub ended_ms: Option<i64>,
    /// Whether that work reached `worker_done`. `Some(false)` is a dispatch
    /// that ended without a word; `None` is one that has not ended.
    pub succeeded: Option<bool>,
    /// The title of the task it carried, when it carried one.
    pub title: Option<String>,
}

/// What this ledger's own history says about one agent: how often the
/// coordinators here chose it, what became of that work, and how long it
/// took them.
///
/// Every number is this machine's, bounded by retention — a run the sweep
/// has taken is a run this ledger no longer holds — and none of it is a
/// claim about the agent. "This window summoned it 265 times and 121 of
/// those reached `worker_done`" is a fact about this window; "claude is good
/// at Rust" is the table this file refuses to keep.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    /// How many summonses this ledger has carried for it.
    pub launched: usize,
    /// How many of those gave it work that has since ENDED — the only ones
    /// anything is known about.
    pub carried: usize,
    /// How many of THOSE reached `worker_done`.
    pub finished: usize,
    /// The median wall time of the ended ones, in whole minutes.
    pub median_minutes: Option<u64>,
    /// The titles of the tasks its newest summonses here carried, newest
    /// first — what this window keeps finding it fit for, in the
    /// coordinators' own words. At most [`SUMMON_RECENT_BRIEFS`] of them,
    /// each cut to [`SUMMON_RECENT_BRIEF_CHAR_CAP`].
    pub recent_briefs: Vec<String>,
}

impl AgentRecord {
    /// What an option says about this record — the history, and then what
    /// came of it.
    fn means(&self, between: usize) -> String {
        if self.launched == 0 {
            return HISTORY_NONE.to_string();
        }
        let briefs = match self.recent_briefs.is_empty() {
            true => NO_TASK.to_string(),
            false => self
                .recent_briefs
                .iter()
                .map(|brief| crate::jev::door::cut(brief, Cap::Chars(SUMMON_RECENT_BRIEF_CHAR_CAP)))
                .collect::<Vec<String>>()
                .join(BRIEF_SEPARATOR),
        };
        let history = HISTORY_SOME
            .replace("{launched}", &self.launched.to_string())
            .replace("{between}", &between.to_string())
            .replace("{briefs}", &briefs);
        let outcome = match (self.carried, self.median_minutes) {
            (0, _) | (_, None) => RECORD_PENDING.to_string(),
            (carried, Some(minutes)) => RECORD_SOME
                .replace("{carried}", &carried.to_string())
                .replace("{finished}", &self.finished.to_string())
                .replace("{minutes}", &minutes.to_string()),
        };
        history + &outcome
    }
}

/// How many milliseconds make a minute — the unit a median is reported in,
/// because a coordinator reads "41 minutes" and not "2,460,000".
const MS_A_MINUTE: i64 = 60_000;

/// What each agent's summonses came to, folded out of `carried`.
///
/// The one place this arithmetic lives. `launched` counts every summons;
/// `carried` and `finished` count only the ones whose work has ENDED, because
/// an open dispatch has no outcome and folding it in either direction would
/// be a number about nothing. The median is the nearest-rank p50 the Jev
/// summary already computes ([`crate::jev::summary::percentile`]).
#[must_use]
pub fn records(carried: &[CarriedSummons]) -> BTreeMap<String, AgentRecord> {
    let mut titles: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    let mut minutes: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut records: BTreeMap<String, AgentRecord> = BTreeMap::new();
    for summons in carried {
        let record = records.entry(summons.agent.clone()).or_default();
        record.launched += 1;
        if let (Some(ended), Some(succeeded)) = (summons.ended_ms, summons.succeeded) {
            record.carried += 1;
            record.finished += usize::from(succeeded);
            let held = ended.saturating_sub(summons.started_ms) / MS_A_MINUTE;
            minutes
                .entry(summons.agent.clone())
                .or_default()
                .push(u64::try_from(held).unwrap_or(0));
        }
        if let Some(title) = summons.title.clone() {
            titles
                .entry(summons.agent.clone())
                .or_default()
                .push((summons.started_ms, title));
        }
    }
    for (agent, mut held) in minutes {
        held.sort_unstable();
        if let Some(record) = records.get_mut(&agent) {
            record.median_minutes = crate::jev::summary::percentile(&held, 0.50);
        }
    }
    for (agent, mut held) in titles {
        // Newest first, and a title repeated by a retry named once: three
        // attempts on one task are one thing this window found the agent fit
        // for, not three.
        held.sort_unstable_by(|left, right| right.0.cmp(&left.0));
        let mut newest = Vec::new();
        for (_, title) in held {
            if !newest.contains(&title) {
                newest.push(title);
            }
            if newest.len() == SUMMON_RECENT_BRIEFS {
                break;
            }
        }
        if let Some(record) = records.get_mut(&agent) {
            record.recent_briefs = newest;
        }
    }
    records
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
    /// How many attempts this task has already had — dispatches on it that
    /// ended, counted before this summons opens one.
    ///
    /// The difficulty grade the LEDGER can give honestly. There is no table
    /// here that bands work by its words, for the same reason there is none
    /// that bands agents: it would be a belief. Whether this particular task
    /// has already defeated somebody is a measurement.
    pub attempts: usize,
    /// The task's consecutive failures, as [`crate::orchestration::Task`]
    /// counts them — three ends a task rather than dispatching a fourth.
    pub failures: u32,
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
        RECORD_SOME,
        RECORD_PENDING,
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
    // The denominator every option's history is said against: the summonses
    // this ledger gave the agents offered HERE. Read off the options rather
    // than the whole ledger, so a question's own numbers add up inside it
    // and an agent nobody could summon today cannot move them.
    let between: usize = summonable.iter().map(|agent| agent.record.launched).sum();
    let mut criteria = Map::new();
    let mut offered = Vec::new();
    for agent in summonable {
        criteria.insert(agent.id.clone(), Value::from(agent.means(between)));
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
        ("attempts".to_string(), Value::from(look.attempts)),
        ("failures".to_string(), Value::from(look.failures)),
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
