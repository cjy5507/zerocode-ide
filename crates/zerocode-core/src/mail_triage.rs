//! Which letters in a coordinator's inbox want it now (t-9471,
//! `crate::jev::MAIL_TRIAGE`): the question the window asks Jev about every
//! letter a run's coordinator is handed, the rule it is graded against today
//! — the letter's kind alone ([`kind_rule`]) — and the label the coordinator's
//! own next acts write afterwards ([`Mailroom::label`]).
//!
//! The question carries a letter's structure and nothing it says: its kind,
//! the kind of address that sent it, the worker and task it concerns and
//! where that task stands, its priority, whether it waits on an answer, how
//! deep in a conversation it sits, how long it has waited, whether the
//! coordinator has been handed it, how many letters about the same thing came
//! before it, whether the coordinator is mid-turn and how many questions wait
//! on it. No body, subject or payload leaves: a letter's words wait for a
//! redactor that knows a person's name, which none does yet.
//!
//! The label reads the ledger alone. What the coordinator did next is its own
//! acts — the receipts its `--retry-request` verbs filed, oldest first —
//! counted from the moment the letter was handed over ([`Start`]). The
//! inbox's own bookkeeping — a `check`, and the acknowledgement it carries —
//! is not an act: the mail pointer has the coordinator acknowledge every batch
//! within seconds whatever it holds (on this machine, 2026-09-26: the 107
//! batches that carried the day's 172 letters were each followed by the
//! coordinator's next `check` receipt at a median of 11 s and a p90 of 33 s,
//! 106 of them on an empty inbox), so an acknowledgement says the letter was
//! read and nothing about what it asked of anybody. A letter is handled by
//! the first act that names what it is about ([`handled_by`]); handled within
//! [`ANSWER_NOW_WITHIN_ACTS`] acts it wanted the coordinator now, handled
//! later it could wait, and a letter no act named for [`NO_NEED_AFTER_ACTS`]
//! acts needed no look. Nothing here touches the network, the clock, a pane
//! or a file.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value, json};

use crate::jev::choice::{self, ChoiceRefusal};
use crate::jev::noul::{self, NoulRefusal};
use crate::orchestration::{
    Delivery, Dispatch, Doing, Ledger, Message, MessageKind, Run, VERBS, WORKER_ADDRESS_PREFIX,
    worker_address,
};

/// The closed choice's name. The endpoint does not show a question's name to
/// the model, so this is the caller's key and nothing more.
const QUESTION: &str = "triage";

/// The Noul's name, asked in the same request.
const URGENT: &str = "urgent";

/// The words of the question. The letter reaches the model as state, never as
/// an instruction — and never as its words, which are not in it.
const INSTRUCTIONS: &str = "A coordinator agent runs a team of worker agents and reads its mail between the things it does. `state` describes one letter that has just reached its inbox, never the letter's words: `kind` is what the letter is — `question` (its sender waits for an answer), `worker_done` (a worker reports its task finished and waits for review), `status` (a worker's news), or one of the notices the orchestration writes itself about a worker (`went_quiet`, `worker_died`, `quota_walled`, `classifier_declined`, `deadlocked`, `handover`, `resumed`, `model_deviated`, `account_switched`) — `from` is the kind of address that sent it (`worker`, `ledger` for the orchestration's own notices, `pane`, `run`, `home`, `remote`), `worker` and `task` the worker and the task it concerns, `taskStatus` where that task stands, `priority` the priority its sender set, `awaitsAnswer` whether it is a question nobody has answered yet, `threadDepth` how many replies deep it sits, `ageSeconds` how long it has waited, `delivered` whether the coordinator has been handed it yet, `repeats` how many earlier letters of the same kind about the same worker and task came in the day before it, `coordinatorBusy` whether the coordinator is in the middle of a turn (null when unknown), and `openQuestions` how many questions put to the coordinator wait for an answer. Choose when the coordinator should deal with this letter.";

/// The words of the Noul asked beside the choice.
const URGENT_INSTRUCTIONS: &str = "Should this letter be the very next thing the coordinator deals with, ahead of every other letter and every other piece of work?";

/// What the Noul's yes means.
const URGENT_YES: &str = "Its very next action should be about this letter: somebody is stopped until it acts, and every minute it waits costs.";

/// What the Noul's no means.
const URGENT_NO: &str = "At least one other thing can come first without harm.";

/// The state's keys, in the order the fingerprint reads them.
pub const STATE_KEYS: [&str; 13] = [
    "kind",
    "from",
    "worker",
    "task",
    "taskStatus",
    "priority",
    "awaitsAnswer",
    "threadDepth",
    "ageSeconds",
    "delivered",
    "repeats",
    "coordinatorBusy",
    "openQuestions",
];

/// The version of the words in this module. Bump it when any of them changes,
/// or when the label they are graded by changes: a judgment read under one
/// wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to
/// [`crate::jev::rubric_fingerprint`].
pub const MAIL_TRIAGE_RUBRIC_VERSION: u32 = 1;

/// When a coordinator should deal with one letter — the question's closed
/// answer space, and the label's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triage {
    /// Before anything else.
    AnswerNow,
    /// After what it is doing now.
    CanWait,
    /// Reading it is all it asks.
    NoNeed,
}

impl Triage {
    /// Every answer, in the order the question offers them.
    pub const ALL: [Self; 3] = [Self::AnswerNow, Self::CanWait, Self::NoNeed];

    /// The option's name — the word a ledger row keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::AnswerNow => "answer_now",
            Self::CanWait => "can_wait",
            Self::NoNeed => "no_need",
        }
    }

    /// What the option means, written as the situation the letter's facts
    /// show.
    const fn means(self) -> &'static str {
        match self {
            Self::AnswerNow => {
                "Deal with it before anything else: somebody is blocked until the coordinator acts — a question its asker waits on, a finished task that waits for review before the next one can start, a worker that died or hit a wall and needs a replacement, a decision only the coordinator can make."
            }
            Self::CanWait => {
                "It needs the coordinator, but not before what it is doing now: news it will act on later — a status report that changes a plan, a notice about a worker that is still making progress, a receipt of something the orchestration already did on the coordinator's behalf."
            }
            Self::NoNeed => {
                "Nothing to do beyond reading it: a repeat of a notice the coordinator has already seen about the same worker, a routine heartbeat, a notice that resolves itself, chatter that waits on nobody."
            }
        }
    }

    /// The answer an option names, if it names one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|triage| triage.word() == word)
    }
}

/// The rule the seat is graded against: the letter's kind alone — the
/// cheapest reader that could stand in for it, and the one a person reading
/// the desk applies by eye. It reads no age: a letter's wait says how busy
/// the coordinator was, not what the letter asks of it.
///
/// - Now: a question (its asker waits), a finished task (its review gates the
///   next one), a decision put to the coordinator, an escalation, and the
///   notices that leave a worker stopped until somebody acts — its death,
///   its quota wall, a classifier's decline, a ring of waiting.
/// - Later: news the coordinator acts on in its own time — a worker's status,
///   a merge that is ready, a handoff or a dispatch, and the orchestration's
///   receipts of what it did for the coordinator (a standing order's
///   handover, a model its CLI switched to).
/// - No look: a silence (the orchestration tells it again every five
///   minutes while it lasts), a heartbeat, a continuation it typed itself, an
///   account it moved.
#[must_use]
pub const fn kind_rule(kind: MessageKind) -> Triage {
    match kind {
        MessageKind::Question
        | MessageKind::WorkerDone
        | MessageKind::DecisionGate
        | MessageKind::Escalation
        | MessageKind::WorkerDied
        | MessageKind::QuotaWalled
        | MessageKind::ClassifierDeclined
        | MessageKind::Deadlocked => Triage::AnswerNow,
        MessageKind::Status
        | MessageKind::MergeReady
        | MessageKind::Handoff
        | MessageKind::Dispatch
        | MessageKind::Handover
        | MessageKind::ModelDeviated => Triage::CanWait,
        MessageKind::WentQuiet
        | MessageKind::Heartbeat
        | MessageKind::Resumed
        | MessageKind::AccountSwitched => Triage::NoNeed,
    }
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    todo!("t-9471")
}

/// One letter, as the question carries it.
#[derive(Debug, Clone, Copy)]
pub struct MailLook<'a> {
    pub kind: MessageKind,
    /// The kind of address that sent it — the head of its `from`.
    pub from: &'a str,
    pub worker: Option<&'a str>,
    pub task: Option<&'a str>,
    pub task_status: Option<&'a str>,
    pub priority: &'a str,
    pub awaits_answer: bool,
    pub thread_depth: usize,
    pub age_ms: i64,
    pub delivered: bool,
    pub repeats: usize,
    pub coordinator_busy: Option<bool>,
    pub open_questions: usize,
}

/// One question, ready for the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct MailAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`: the choice and the Noul.
    pub questions: Value,
}

/// A validated answer: the choice, its spread and confidence, and the Noul's
/// probability of yes.
#[derive(Debug, Clone, PartialEq)]
pub struct MailRead {
    pub triage: Triage,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
    pub urgent: f64,
}

/// Every way an answer fails to be one: its choice's rule or its Noul's. Both
/// discard the answer whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailRefusal {
    Triage(ChoiceRefusal),
    Urgent(NoulRefusal),
}

impl MailRefusal {
    /// The word a ledger row writes for this refusal — the broken rule's own.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Triage(refusal) => refusal.token(),
            Self::Urgent(refusal) => refusal.token(),
        }
    }
}

impl From<ChoiceRefusal> for MailRefusal {
    fn from(refusal: ChoiceRefusal) -> Self {
        Self::Triage(refusal)
    }
}

impl From<NoulRefusal> for MailRefusal {
    fn from(refusal: NoulRefusal) -> Self {
        Self::Urgent(refusal)
    }
}

/// The question one letter asks: the choice, and the Noul beside it in the
/// same request — the state is charged once and an answer's output is free.
#[must_use]
pub fn ask(_look: &MailLook<'_>) -> MailAsk {
    todo!("t-9471")
}

impl MailAsk {
    /// What the endpoint's `answers` map says about this question. One broken
    /// rule in either head discards the answer whole.
    ///
    /// # Errors
    ///
    /// [`MailRefusal`] names which rule the answer broke.
    pub fn read(&self, _answers: &Value) -> Result<MailRead, MailRefusal> {
        todo!("t-9471")
    }
}

/* ---- the label ---------------------------------------------------------- */

/// How many of the coordinator's acts after a letter was handed over may
/// come before the act that handles it for the letter to have wanted the
/// coordinator now: three.
///
/// Measured on the one letter the coordinator treats as blocking by
/// construction — a question, whose asker waits: over this machine's ledger
/// in the week to 2026-09-26 the coordinator answered 130 of the 151
/// questions put to it, the answer being its first act after the question
/// was handed over for 94 of them, within two for 112 and within three for
/// 118 (90.8%; p50 1, p90 3). The acts before an answer are the ones a
/// sitting takes — the question that arrived with it answered first, a word
/// to another worker — so three is what "now" costs in acts, batches and
/// all. Each label row keeps the act count and the time beside the word, so
/// the line can be drawn again without a second label.
pub const ANSWER_NOW_WITHIN_ACTS: usize = 3;

/// How many of the coordinator's acts may pass with nothing handling a
/// letter before it is read as one that needed no look: thirty.
///
/// The ninety-fifth percentile of the act at which the coordinator handled
/// what it handled, over the same week: 767 letters handled, at a median act
/// of 2, p90 18, p95 30 — the long end is the review of a finished task,
/// whose `task-update` lands after the review it waited for (p90 26). A
/// letter nothing named for as long as nineteen in twenty handled ones took
/// is more likely one that asked for nothing than one still in hand; about
/// an hour and a half of the coordinator's work at this machine's pace.
pub const NO_NEED_AFTER_ACTS: usize = 30;

/// How long a letter's label may wait for those acts at all, from when it
/// was written: a day. A coordinator that stopped — its run over, its window
/// closed for the night — acts on nothing, and a letter still unhandled a day
/// on is written as `unhandled` rather than waited on forever. A day stands
/// past 99 in 100 of the ages at which this machine's coordinators were
/// handed their notices (p99 17.95 h, 2026-09-26, `desk.rs`'s own
/// measurement of the same inboxes).
pub const LABEL_HORIZON_MS: i64 = 24 * 60 * 60 * 1_000;

/// What an act can name about a letter — the table [`handled_by`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// The letter itself: the coordinator's reply in its thread — for a
    /// question, its answer ([`Run::answer_to`]'s rule).
    Thread,
    /// The task it concerns: a `task-update`, a `worker-start` for it, a
    /// message about it.
    Task,
    /// The worker it concerns: mail to that worker, a verb naming it.
    Worker,
    /// The attempt it concerns: a retry of it, a message about it.
    Attempt,
}

impl Subject {
    /// Every subject, in the order a label row names them.
    pub const ALL: [Self; 4] = [Self::Thread, Self::Task, Self::Worker, Self::Attempt];
}

/// Which acts handle a letter of `kind` (confirmed by the coordinator,
/// m-10067): a question only its answer; a finished task only an act naming
/// that task — its review's `task-update`, a new attempt at it, a word about
/// it; every other letter any act naming its thread, its task, its worker or
/// its attempt.
#[must_use]
pub const fn handled_by(kind: MessageKind) -> &'static [Subject] {
    match kind {
        MessageKind::Question => &[Subject::Thread],
        MessageKind::WorkerDone => &[Subject::Task],
        _ => &Subject::ALL,
    }
}

/// What a receipt's own answer named, by the ledger's ids and nothing else:
/// the task, the worker, the message and the attempt a verb acted on, as the
/// verb printed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub task: Option<String>,
    pub worker: Option<String>,
    pub message: Option<String>,
    pub dispatch: Option<String>,
    pub retry_of: Option<String>,
}

impl Names {
    /// The ids `answer` — a verb's printed answer — names under the keys the
    /// ledger's verbs print them under. Anything else in it — a status word,
    /// a title, an answer's prose — is not read.
    #[must_use]
    pub fn of_answer(_answer: &Value) -> Self {
        todo!("t-9471")
    }
}

/// The inbox a `check` receipt looked in and what it handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Looked {
    pub address: String,
    pub messages: Vec<String>,
}

/// One receipt, as the label reads it: who filed it, which verb, when, what
/// its answer named, and — for a `check` — the inbox it looked in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed {
    pub caller: Option<String>,
    pub verb: Option<String>,
    pub filed_ms: Option<i64>,
    pub names: Names,
    pub check: Option<Looked>,
    inbox: bool,
}

impl Filed {
    /// One receipt from what it holds: who filed it, the verb, when, the
    /// answer the verb printed — read for its ids and, on a row filed with no
    /// verb, for the page only a look prints — and the inbox a `check` looked
    /// in. One reading for a live ledger's receipts and a replay's copy of
    /// them.
    #[must_use]
    pub fn of_parts(
        _caller: Option<String>,
        _verb: Option<String>,
        _filed_ms: Option<i64>,
        _answer: &Value,
        _check: Option<Looked>,
    ) -> Self {
        todo!("t-9471")
    }

    /// Every receipt `ledger` holds that was filed after `since_ms`, oldest
    /// first as the ledger keeps them.
    #[must_use]
    pub fn of_ledger(_ledger: &Ledger, _since_ms: i64) -> Vec<Self> {
        todo!("t-9471")
    }

    /// Whether the receipt is the inbox's own — a look, or the
    /// acknowledgement a look carries: its verb's class ([`Doing::Inbox`]),
    /// or, on a row an older window filed with no verb (every receipt before
    /// 2026-09-25 17:53 on this machine, t-6742), the answer only a look
    /// gives — a batch handed over, or a peek's or a history's page.
    #[must_use]
    pub const fn reads_the_inbox(&self) -> bool {
        self.inbox
    }
}

/// One act of the coordinator's: when, which verb, what its answer named,
/// and the message it wrote, when it wrote one.
#[derive(Debug, Clone, Copy)]
pub struct Act<'a> {
    pub at_ms: i64,
    pub verb: Option<&'a str>,
    pub names: &'a Names,
    pub message: Option<&'a Message>,
}

/// When the coordinator was handed a letter, and how the label knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// The window saw it in the batch the coordinator held open: the batch's
    /// own stamp.
    Opened(i64),
    /// A `check` receipt handed it over.
    Checked(i64),
    /// Neither: the moment it was written, which is no later than the
    /// hand-over.
    Created(i64),
}

impl Start {
    /// The moment.
    #[must_use]
    pub const fn at(self) -> i64 {
        match self {
            Self::Opened(at) | Self::Checked(at) | Self::Created(at) => at,
        }
    }

    /// The word a label row names its start by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Opened(_) => "opened",
            Self::Checked(_) => "checked",
            Self::Created(_) => "created",
        }
    }
}

/// Why a letter's label carries no mark — the words a label row writes under
/// the summary's `notCompared`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotCompared {
    /// A newer letter of the same kind about the same worker and task came
    /// before anything handled this one: whatever handles the thing now
    /// answers the newer letter.
    Superseded,
    /// The day ran out before the coordinator had acted enough to say.
    Unhandled,
}

impl NotCompared {
    /// Every reason, in the order a reader lists them.
    pub const ALL: [Self; 2] = [Self::Superseded, Self::Unhandled];

    /// The word a label row keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Superseded => "superseded",
            Self::Unhandled => "unhandled",
        }
    }
}

/// What a letter's label says the coordinator did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labeled<'a> {
    /// When it dealt with the letter.
    pub truth: Triage,
    /// Whether the very first act after the hand-over handled it.
    pub urgent: bool,
    /// Which act that was, counted from the hand-over — or the act at which
    /// the letter was given up as needing no look.
    pub after_actions: usize,
    /// And how long after the hand-over it came.
    pub after_ms: i64,
    /// The verb of the act that handled it; `None` for a letter nothing
    /// handled.
    pub handled_by: Option<&'a str>,
}

/// One run's mail and its coordinator's acts, read once: the view the label
/// and the question read, built alike off a live ledger ([`Self::of_run`])
/// and off a replay's copy of one ([`Self::new`]).
#[derive(Debug)]
pub struct Mailroom<'a> {
    address: String,
    messages: &'a [Message],
    dispatches: &'a [Dispatch],
    open: Option<&'a Delivery>,
    acts: Vec<Act<'a>>,
    index: HashMap<&'a str, usize>,
    checked: HashMap<&'a str, i64>,
    pending: HashSet<&'a str>,
}

impl<'a> Mailroom<'a> {
    /// The view of one run's mail: its address, its seat's actor, its
    /// messages and attempts in the ledger's order, the batch its coordinator
    /// holds open, the ids still waiting to be handed over, and the receipts
    /// to read the coordinator's acts from.
    #[must_use]
    pub fn new(
        _address: String,
        _seat_actor: Option<&str>,
        _messages: &'a [Message],
        _dispatches: &'a [Dispatch],
        _open: Option<&'a Delivery>,
        _pending: &[&'a str],
        _receipts: &'a [Filed],
    ) -> Self {
        todo!("t-9471")
    }

    /// The view of `run` as the ledger holds it now.
    #[must_use]
    pub fn of_run(_run: &'a Run, _receipts: &'a [Filed]) -> Self {
        todo!("t-9471")
    }

    /// The run's own address — its coordinator's inbox.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The coordinator's acts, oldest first.
    #[must_use]
    pub fn acts(&self) -> &[Act<'a>] {
        &self.acts
    }

    /// Every letter to the coordinator, oldest first: the mail its inbox
    /// holds that it did not write itself.
    pub fn letters(&self) -> impl Iterator<Item = &'a Message> + '_ {
        let address = self.address.clone();
        self.messages
            .iter()
            .filter(move |one| one.to == address && one.from != address)
    }

    /// The message `id`, when the run holds one.
    #[must_use]
    pub fn message(&self, id: &str) -> Option<&'a Message> {
        self.index.get(id).map(|at| &self.messages[*at])
    }

    /// Whether `letter` still waits to be handed over.
    #[must_use]
    pub fn is_pending(&self, letter: &Message) -> bool {
        self.pending.contains(letter.id.as_str())
    }

    /// Whether `letter` is in the batch the coordinator holds open.
    #[must_use]
    pub fn is_open(&self, letter: &Message) -> bool {
        self.open
            .is_some_and(|batch| batch.messages.iter().any(|id| *id == letter.id))
    }

    /// The worker `letter` concerns: its sender, when a worker sent it, else
    /// the worker of the attempt it names.
    #[must_use]
    pub fn worker_of<'m>(&'m self, _letter: &'m Message) -> Option<&'m str> {
        todo!("t-9471")
    }

    /// When the coordinator was handed `letter`: the open batch's stamp the
    /// window saw (`seen_opened`) or the one it stands in now, else the first
    /// `check` receipt that handed it over, else when it was written.
    #[must_use]
    pub fn start_of(&self, _letter: &Message, _seen_opened: Option<i64>) -> Start {
        todo!("t-9471")
    }

    /// How many letters of the same kind about the same worker and task came
    /// in the [`LABEL_HORIZON_MS`] before `letter`.
    #[must_use]
    pub fn repeats(&self, _letter: &Message) -> usize {
        todo!("t-9471")
    }

    /// The label the coordinator's acts wrote for `letter` handed over at
    /// `start`, by `now_ms` — or `None` while they have not said yet.
    #[must_use]
    pub fn label(
        &self,
        _letter: &Message,
        _start: Start,
        _now_ms: i64,
    ) -> Option<Result<Labeled<'a>, NotCompared>> {
        todo!("t-9471")
    }

    /// Whether `act` handles `letter`, by the table ([`handled_by`]).
    #[must_use]
    pub fn handles(&self, _letter: &Message, _act: &Act<'_>) -> bool {
        todo!("t-9471")
    }

    /// When a newer letter of the same kind about the same worker and task
    /// arrived after `letter`, if one did — never for a question, which has
    /// its own answer.
    #[must_use]
    pub fn newer_at(&self, _letter: &Message) -> Option<i64> {
        todo!("t-9471")
    }

    /// The question `letter` asks, at `now_ms`: its facts off `run`, whether
    /// the coordinator is mid-turn (`coordinator_busy`, the window's to say)
    /// and how many questions wait on it (`open_questions`,
    /// [`Self::open_questions`], read once per look at a run).
    #[must_use]
    pub fn look(
        &'a self,
        _run: &'a Run,
        _letter: &'a Message,
        _now_ms: i64,
        _coordinator_busy: Option<bool>,
        _open_questions: usize,
    ) -> MailLook<'a> {
        todo!("t-9471")
    }

    /// How many questions put to the coordinator wait for an answer that can
    /// still reach their asker — the desk's 「답할 우편」 rule
    /// ([`Run::awaits_answer`], [`Run::question_is_answerable`]).
    #[must_use]
    pub fn open_questions(&self, _run: &Run) -> usize {
        todo!("t-9471")
    }
}

/// The label a run of `acts` writes for a letter handed over at `start` and
/// written at `created_ms`, by `now_ms`: the first act after the start that
/// `handles` (by its place in `acts`) decides it, a newer letter of the same
/// subject arriving at `newer_at` before any did supersedes it, and
/// [`NO_NEED_AFTER_ACTS`] acts with none, or [`LABEL_HORIZON_MS`] with too few
/// to say, close it. One engine for the rule the seat is graded by and any
/// other a replay puts beside it.
pub fn label_over<'a>(
    _acts: &[Act<'a>],
    _start: Start,
    _newer_at: Option<i64>,
    _created_ms: i64,
    _now_ms: i64,
    _handles: impl Fn(usize, &Act<'a>) -> bool,
) -> Option<Result<Labeled<'a>, NotCompared>> {
    todo!("t-9471")
}

#[cfg(test)]
mod tests;
