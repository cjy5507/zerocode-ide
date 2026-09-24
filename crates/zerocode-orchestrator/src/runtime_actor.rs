//! Dormant single-owner runtime for the orchestration ledger authority.
//!
//! The actor owns [`EffectJournal`] on one thread. Callers exchange bounded,
//! typed messages and never receive the journal or its recovery permits. This
//! stage deliberately has no shell/core/JSON wiring; activation belongs to a
//! later cutover that removes the legacy writer in the same commit.
//! Fresh (non-legacy) initialization and legacy import are distinct boot
//! transactions; a fresh ledger never manufactures a legacy digest.
//! Snapshot construction rejects payloads above the journal's 16 MiB ceiling
//! before they can enter the mailbox. Replies share immutable snapshot bytes;
//! the bound covers queued memory, not a hard realtime guarantee for OS or
//! SQLite stalls.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use sha2::{Digest, Sha256};

use crate::effect_journal::{
    AuthorityLease, BeginEffect, EffectJournal, EffectJournalError, EffectPermit, EffectRequest,
    EffectSettlement, HostEffectKind, HostEffectState, OpenOperation,
};
use crate::workflow_store::WorkflowStore;
use zerocode_core::ProviderSession;
use zerocode_core::agent_teams::{Effect, Team};
use zerocode_core::orchestration::{
    Decided, Launcher, Ledger, LedgerProjectionV1, MAX_LIST, MAX_NAME, MAX_PROSE,
    PROJECTION_SCHEMA, RebuildError, ReceiptKey, Sweep, Waiting, WorkerState,
    repair_unattempted_dispatched_tasks, tombstone_receipts_of_taken_back_batches,
    tombstone_unverifiable_legacy_receipts,
};

use crate::ledger_store;

/// At most this many commands may wait for the owner thread. Replies use
/// rendezvous channels and cannot accumulate elsewhere.
pub const MAX_RUNTIME_MAILBOX: usize = 4;

/// How many words one command may carry.
///
/// A verb line is a verb and its flags. Twenty is already past every verb this
/// window has; sixty-four is past any it is likely to grow, and small enough
/// that the arithmetic below stays a number a person can hold.
pub const MAX_COMMAND_WORDS: usize = 64;

/// How many bytes one command may carry, counting every word.
///
/// Derived from what a verb is allowed to WRITE rather than guessed: one piece
/// of prose, one full list of names, and room for the flags that name them.
/// Bounding the total rather than each word is what makes the mailbox's cost a
/// number — `MAX_RUNTIME_MAILBOX * MAX_COMMAND_BYTES` — instead of the count
/// times the largest word, which is how a four-deep mailbox of whole ledger
/// images came to be worth sixty-four megabytes.
pub const MAX_COMMAND_BYTES: usize = MAX_PROSE + MAX_NAME * MAX_LIST + MAX_COMMAND_WORDS * MAX_NAME;

/// How many bytes a presented pane capability may carry.
///
/// The shell mints these as fixed-width random tokens; four kilobytes is past
/// any token this window has ever minted and small enough that a mailbox of
/// waiting commands stays a number. The same ceiling the split lane keeps for
/// the token it carries the other way.
pub const MAX_TOKEN_BYTES: usize = 4 * 1024;

/// One verb, as it arrives at the door.
///
/// The words are the same words a person typed, and the bounds are the door's
/// — `orchestration::plan` still refuses each value for its own reasons once
/// the command is inside. Two nets, and this one exists so that what is
/// WAITING has a size.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanCommand {
    argv: Vec<String>,
    team: String,
    pane: String,
    /// The capability the caller's pane presented, opaque to this crate.
    ///
    /// Carried INTO the actor rather than checked before the send, because the
    /// check and the plan must share one table-lock generation: a respawn
    /// rotates this token, and a token checked outside the lock could be dead
    /// paper by the time the plan runs. [`PaneTable::with_pane_authority`] is
    /// where the implementation verifies it, under its own lock.
    presented: String,
    handover: Option<Box<zerocode_core::orchestration::HandoverPlan>>,
    worker_incarnation: Option<zerocode_core::agent_teams::PaneIncarnation>,
    actor: Option<String>,
    now_ms: i64,
}

impl std::fmt::Debug for PlanCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        /* The verb is the shape of the thing; the rest is what somebody typed
         * — a prompt, a spec, an address — and none of that belongs in a log. */
        formatter
            .debug_struct("PlanCommand")
            .field("verb", &self.argv.first().map_or("<none>", String::as_str))
            .field("team_bytes", &self.team.len())
            .field("words", &self.argv.len())
            .field("bytes", &self.bytes())
            .field("named_caller", &self.actor.is_some())
            .finish()
    }
}

impl PlanCommand {
    /// A command the door will take, or why it will not.
    pub fn checked(
        argv: Vec<String>,
        team: impl Into<String>,
        pane: impl Into<String>,
        presented: impl Into<String>,
        actor: Option<String>,
        now_ms: i64,
    ) -> Result<Self, RuntimeError> {
        let held = Self {
            argv,
            team: team.into(),
            pane: pane.into(),
            presented: presented.into(),
            handover: None,
            worker_incarnation: None,
            actor,
            now_ms,
        };
        if held.argv.is_empty()
            || held.argv.len() > MAX_COMMAND_WORDS
            || held.bytes() > MAX_COMMAND_BYTES
            || held.team.is_empty()
            || held.team.len() > MAX_NAME
            || held.pane.len() > MAX_NAME
            || held.presented.is_empty()
            || held.presented.len() > MAX_TOKEN_BYTES
            || held
                .actor
                .as_ref()
                .is_some_and(|held| held.len() > MAX_NAME)
            || held.now_ms < 0
        {
            return Err(RuntimeError::InvalidInput);
        }
        Ok(held)
    }

    /// Internal standing-order fence, carried through the ordinary verb door.
    pub fn for_handover(
        mut self,
        plan: zerocode_core::orchestration::HandoverPlan,
        worker: zerocode_core::agent_teams::PaneIncarnation,
    ) -> Result<Self, RuntimeError> {
        self.handover = Some(Box::new(plan));
        self.worker_incarnation = Some(worker);
        if self.bytes() > MAX_COMMAND_BYTES {
            return Err(RuntimeError::InvalidInput);
        }
        Ok(self)
    }

    /// Everything this command is carrying, which is what the mailbox holds.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.argv.iter().map(String::len).sum::<usize>()
            + self.team.len()
            + self.pane.len()
            + self.presented.len()
            + self.actor.as_ref().map_or(0, String::len)
            + self.handover.as_ref().map_or(0, |plan| plan.bytes_held())
            + self
                .worker_incarnation
                .as_ref()
                .map_or(0, |worker| worker.capability.len())
    }

    #[must_use]
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    #[must_use]
    pub fn team(&self) -> &str {
        &self.team
    }

    #[must_use]
    pub fn pane(&self) -> &str {
        &self.pane
    }

    /// The presented capability, for the one door that verifies it.
    ///
    /// Never logged: [`PlanCommand`]'s Debug does not name this field at all,
    /// and the privacy test renders a command carrying a secret token to hold
    /// that true.
    #[must_use]
    pub fn presented(&self) -> &str {
        &self.presented
    }

    #[must_use]
    pub fn actor(&self) -> Option<&str> {
        self.actor.as_deref()
    }

    #[must_use]
    pub const fn now_ms(&self) -> i64 {
        self.now_ms
    }
}

/// How a runtime comes up.
///
/// Production has ONE word: [`RuntimeBoot::Cutover`] — the shell would have
/// to pick between fresh, reopen, and import at exactly the moment it knows
/// the least, so the choosing lives here. The JSON itself is never parsed in
/// this crate: the shell reads the person's file and hands a projection
/// across, and the boot validates it the way it validates rows —
/// `Ledger::rebuild` — before a byte of it is written.
pub enum RuntimeBoot {
    /// A ledger this window has never written. Refused if one is already
    /// there. Retired from production by `Cutover`; tests keep it because an
    /// exact input is the cheapest fixture.
    #[cfg(test)]
    Fresh { now_ms: i64 },
    /// The ledger the store already holds, and nothing else accepted.
    #[cfg(test)]
    Reopen,
    /// The one production road in.
    ///
    /// If the store already holds this ledger, the store wins and `legacy`
    /// is not looked at — the file was imported once, and `import_legacy`'s
    /// digest identity is what makes saying so safe. Otherwise `Some`
    /// imports the person's ledger under the digest of their file's bytes,
    /// and `None` initializes fresh through the journal's own door. Either
    /// way the boot image is revision zero, and the first verb is one.
    Cutover {
        legacy: Option<Box<(LedgerProjectionV1, String)>>,
        now_ms: i64,
    },
}

impl std::fmt::Debug for RuntimeBoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(test)]
            Self::Fresh { now_ms } => formatter
                .debug_struct("RuntimeBoot::Fresh")
                .field("now_ms", now_ms)
                .finish(),
            #[cfg(test)]
            Self::Reopen => formatter.write_str("RuntimeBoot::Reopen"),
            /* The projection is the person's whole ledger and the digest
             * names their file; a log learns whether one was offered. */
            Self::Cutover { legacy, now_ms } => formatter
                .debug_struct("RuntimeBoot::Cutover")
                .field("legacy", &legacy.is_some())
                .field("now_ms", now_ms)
                .finish(),
        }
    }
}

/// The one pane table, borrowed under its own lock.
///
/// The actor does not OWN the table — the table's truth is the host's, and
/// the shell keeps it beside the panes it describes. What the actor needs is
/// to stand its reads and its plans under the SAME lock the rest of the
/// window uses, so a seat cannot change meaning between the check and the
/// change. The disk is never touched inside these closures: the ledger's
/// atomicity comes from the actor being one thread, not from this lock.
///
/// The table is not durable, and that is a sentence, not an accident: its
/// truth is the host's, the next boot rebuilds it from the host, and a
/// refused ledger write (`NotDurable`) rewinds the LEDGER only.
pub trait PaneTable: Send {
    /// Run `action` with the named team borrowed mutably under the table
    /// lock — or with `None` if the table holds no such team.
    ///
    /// `&mut`, and not a read borrow that would look sufficient: planning a
    /// `worker-start` calls `Team::take_pane_id`, which ADVANCES the pane
    /// id counter. The counter is an issuer for one namespace — its rule is
    /// "a released id is never lent again" — and a copy of the team would
    /// be a second issuer handing out the same `%2`. Do not simplify this
    /// to `&Team`; it breaks exactly there.
    fn with_team(&self, team: &str, action: &mut dyn FnMut(Option<&mut Team>));
    fn incarnation(
        &self,
        team: &str,
        pane: &str,
    ) -> Option<zerocode_core::agent_teams::PaneIncarnation>;
    /// Resolve which seat a terminal is, under the same lock, and let
    /// `action` settle against that answer before the lock is given back.
    #[allow(clippy::type_complexity)] // One closure parameter; an alias would only move the shape.
    fn with_seat_of_term(&self, term: u32, action: &mut dyn FnMut(Option<(&str, &str)>));
    /// [`with_team`], behind the caller's own proof.
    ///
    /// Run `action` with the team borrowed mutably ONLY if `presented` is the
    /// capability the named pane holds right now — verified under the same
    /// lock the borrow lives in, so a respawn cannot rotate the token between
    /// the check and the plan. Unknown team, unknown pane and a wrong token
    /// are one `None` on purpose: three different answers would tell a caller
    /// which of the three it guessed.
    ///
    /// The implementation must compare in constant time — the shell's
    /// `authorized_team_mut` is the reference — and must not let the token
    /// reach any log. This crate treats it as opaque bytes.
    ///
    /// [`with_team`]: PaneTable::with_team
    fn with_pane_authority(
        &self,
        team: &str,
        pane: &str,
        presented: &str,
        action: &mut dyn FnMut(Option<&mut Team>),
    );
}

/// A rendezvous at the effect boundary, after the actor has taken the request.
/// The caller keeps host observation locks through the reply (retirement), or
/// completes the host action before replying (split). Dropping either end
/// cancels the rendezvous, including while unwinding a host panic.
pub struct EffectFence {
    entered: SyncSender<()>,
    proceed: Receiver<i64>,
}

impl EffectFence {
    fn enter(self) -> Result<i64, RuntimeError> {
        self.entered
            .send(())
            .map_err(|_| RuntimeError::AuthorityRejected)?;
        self.proceed
            .recv()
            .map_err(|_| RuntimeError::AuthorityRejected)
    }
}

struct PendingEffect {
    entered: Receiver<()>,
    proceed: SyncSender<i64>,
    answer: Receiver<Result<RuntimeReply, RuntimeError>>,
}

/// One caller request. Payload bytes remain opaque to this crate.
pub enum RuntimeRequest {
    /// Show me the ledger.
    View,
    /// Carry out one verb.
    Plan(PlanCommand),
    /// A terminal the host closed: settle whatever attempt sat in it.
    ///
    /// These three carry HOST facts, and they are separate from `Plan` on
    /// purpose: `plan`'s input is argv a person typed, and a host fact in
    /// that vocabulary would be a fact a person could forge.
    TerminalGone {
        term: u32,
        screen: Option<String>,
        now_ms: i64,
    },
    /// A turn ended in a seat: write the worker's silence down, once.
    PaneTurnEnded {
        term: u32,
        turn_started_ms: i64,
        interrupted: bool,
        now_ms: i64,
    },
    /// Workers whose live panes the window measured as quiet past the stall
    /// grace. The ledger revalidates lifecycle and reminder cadence.
    QuietSweep {
        stalled: Vec<(String, i64)>,
        now_ms: i64,
    },
    /// Workers the window found at their provider's quota wall — with BOTH
    /// witnesses already in hand (`quota_wall_witness` builds nothing with
    /// one). The ledger revalidates lifecycle and writes one notice per
    /// attempt.
    QuotaWalls {
        walled: Vec<zerocode_core::orchestration::QuotaWallWitness>,
        now_ms: i64,
    },
    /// Walls the wait rung held that lifted while their workers stayed
    /// stopped at them (t-6427) — both witnesses already in hand
    /// (`read_lift` builds nothing with one). The ledger revalidates and
    /// tells each wall once.
    QuotaLifts {
        lifted: Vec<zerocode_core::orchestration::QuotaLift>,
        now_ms: i64,
    },
    /// Workers the window found stopped at a safety classifier's decline —
    /// with BOTH witnesses already in hand (`classifier_decline_witness`
    /// builds nothing with one, t-6747). The ledger revalidates lifecycle and
    /// writes one notice per attempt.
    ClassifierDeclines {
        declined: Vec<zerocode_core::orchestration::ClassifierDeclineWitness>,
        now_ms: i64,
    },
    /// Switches of model the window read in workers' own records — declines
    /// their CLIs answered on the category's route (t-6747). The ledger
    /// writes one row per switch.
    ModelDeviations {
        deviated: Vec<zerocode_core::orchestration::ModelDeviation>,
        now_ms: i64,
    },
    /// What the stall seat read off quiet panes this beat, when the seat
    /// acts. The ledger revalidates lifecycle and writes one notice per
    /// silence.
    StallCauses {
        judged: Vec<zerocode_core::orchestration::StallJudged>,
        now_ms: i64,
    },
    /// The beat reserves one handover before it walks it (§2.3): the
    /// receipt row exists from the first step, so a window that dies
    /// mid-walk leaves a row saying how far it got.
    HandoverBegin {
        plan: Box<zerocode_core::orchestration::HandoverPlan>,
        now_ms: i64,
    },
    /// One step the beat walked, appended to the receipt as it happened.
    HandoverStep {
        run: String,
        handover: String,
        step: zerocode_core::orchestration::HandoverStep,
        now_ms: i64,
    },
    /// The walk is over; the receipt is delivered.
    HandoverSettle {
        run: String,
        handover: String,
        replacement: Option<(String, String)>,
        now_ms: i64,
    },
    /// The beat reserves one transient-error continuation before it types
    /// it (t-4537): the receipt row exists before the words do.
    ResumeBegin {
        plan: Box<zerocode_core::orchestration::ResumePlan>,
        now_ms: i64,
    },
    /// What became of a typed continuation; the receipt is delivered.
    ResumeSettle {
        run: String,
        resume: String,
        outcome: zerocode_core::orchestration::ResumeOutcome,
        now_ms: i64,
    },
    /// The beat's readiness sweep — a fourth host fact: which TERMINALS the
    /// window heard since the last beat (a turn beginning counts, and only
    /// the window sees beginnings), which installed hook scripts left a
    /// failed-delivery marker, and the clock to judge silence by.
    ReadinessSweep {
        spoken: Vec<u32>,
        delivery_failures: Vec<(u32, i64)>,
        now_ms: i64,
    },
    /// Live worker panes the host could not find after its confirmation
    /// window. Worker ids and first-missing stamps are the only facts crossing
    /// the boundary; the ledger owns every routing field it reports.
    PanesMissing {
        missing: Vec<(String, i64)>,
        now_ms: i64,
    },
    /// Workers whose live panes the host can see again.
    PanesSeen { workers: Vec<String>, now_ms: i64 },
    /// What the window's reclaimer found in a dead worker's checkout. Facts
    /// only: whether that resolves the hold the ledger opened at the death
    /// or tells the coordinator is the ledger's call.
    CheckoutExamined {
        worker: String,
        examined: zerocode_core::orchestration::Examined,
        now_ms: i64,
    },
    /// A fact the window observed about a checkout — CI moved on the review
    /// a worktree is on — for the ledger to mail to whoever sits there. The
    /// eighth host fact: `to` is a ledger address the window composed, never
    /// argv a person typed, and the body is the window's own sentence.
    Observation {
        to: String,
        body: String,
        receipt: Option<String>,
        now_ms: i64,
    },
    /// The beat's retention sweep — give up what a finished run no longer
    /// has to cost. Carries only a clock, because the policy and the last
    /// sweep's stamp are the ledger's own.
    RetentionSweep { now_ms: i64 },
    /// A person's hand landed real keys in a terminal — the fifth host
    /// fact. From that moment the pane is theirs, durably.
    PaneTakenOver { term: u32, now_ms: i64 },
    /// The window is on its way out and every pane goes with it (t-3058):
    /// seated workers sleep now, before their panes' own deaths can settle
    /// them.
    WindowExiting { now_ms: i64 },
    /// The window resumed a conversation into a terminal: the checkout it
    /// stands in, the agent, and the provider session id — the witness a
    /// sleeper is seated again by (t-3058). The seat is resolved from the
    /// terminal under the pane table, like every host fact about a pane.
    WorkerPaneResumed {
        term: u32,
        checkout: String,
        agent: String,
        session_id: String,
        now_ms: i64,
    },
    /// A sleeper the grace ran out on: its attempt ends and its death is
    /// announced with the dispatch id a replacement needs (t-3058).
    SleeperExpired { worker: String, now_ms: i64 },
    /// The provider conversation observed in a terminal. The actor resolves
    /// the terminal to its seat while the pane table is locked, so a respawn
    /// cannot redirect the write between lookup and mutation.
    WorkerSessionReported {
        term: u32,
        session: ProviderSession,
        now_ms: i64,
    },
    /// The window reports where a summoned pane actually sits — the
    /// checkout under it, resolved by the split host. The sixth host fact,
    /// and the only road that fact has into the ledger.
    WorkerSeated {
        team: String,
        pane: String,
        checkout: String,
        now_ms: i64,
    },
    /// A coordinator the window restored — a mounted tab, or a `run-use`
    /// from a conversation already bound to the run — sits again in an EMPTY
    /// or vacated seat. The seventh host fact: the window says which leader
    /// pane came back for which run. A held seat is left alone.
    CoordinatorReturned {
        run: String,
        team: String,
        pane: String,
        actor: Option<String>,
        now_ms: i64,
    },
    /// Build a sleeping worker's replacement split under the coordinator
    /// team's live pane-table borrow. This plans only; no ledger row moves
    /// until `WorkerReseated` arrives after the host walk.
    PrepareWorkerReseat {
        run: String,
        worker: String,
        team: String,
        coordinator_pane: String,
        leader_term: u32,
        resume_nudge: String,
    },
    /// The replacement pane exists: move the sleeping worker's durable seat.
    WorkerReseated {
        worker: String,
        team: String,
        pane: String,
        now_ms: i64,
    },
    /// A permanent preflight failure made a sleeping worker impossible to
    /// restore. Close its existing attempt through its dedicated transition.
    FinishSleepingReseat {
        worker: String,
        reason: String,
        now_ms: i64,
    },
    /// A pane the plan asked for never opened: the worker row it minted
    /// goes back, durably — the third host fact, and the rollback half of
    /// the effect round trip.
    SeatNeverOpened { worker: String, now_ms: i64 },
    /// One federation call, home or worker-server side — a single statement
    /// because the ten calls share one shape: ledger law in, durable iff it
    /// moved, and the transport stays outside the actor entirely.
    Federation(FederationCall),
    /// Reserve one host effect against the current rows, in one
    /// transaction. The digests inside arrived from the lane that will walk
    /// the effect; this door only brackets them with authority and
    /// revision.
    PrepareEffect { request: EffectRequest, now_ms: i64 },
    /// Keep this standing order current through the split lease's host action.
    HandoverSplit {
        prepared: Box<zerocode_core::orchestration::PreparedWorkerStart>,
        operation: String,
        fence: EffectFence,
    },
    /// The lane walked (or failed to walk) the effect; write how it ended.
    SettleEffect {
        permit: EffectPermit,
        settlement: EffectSettlement,
    },
    /// This window just came up: every terminal the last window had is gone,
    /// so every live worker settles at once. The fourth host fact — the
    /// window itself is the host saying "none of those screens exist".
    ///
    /// Called once at boot, AFTER recovery has settled any in-flight effect:
    /// the recovery fence guards this door like every other mutation.
    WindowRestarted { now_ms: i64 },
    /// A leader's terminal closed, taking its whole team with it: settle the
    /// team's workers and put its standing orders down. The seat itself goes
    /// through [`TerminalGone`]; this carries the dissolution, which the
    /// table cannot answer for once the team is dropped from it.
    ///
    /// [`TerminalGone`]: RuntimeRequest::TerminalGone
    TeamDissolved { team: String, now_ms: i64 },
    /// A `check --wait` woke up: look once more, and if something arrived,
    /// take the delivery and remember the answer under `receipt` — one
    /// transaction, so a second verb cannot land between the answer and the
    /// record that it was given.
    LookAgain {
        waiting: Box<Waiting>,
        receipt: Option<Box<ReceiptKey>>,
        now_ms: i64,
    },
    /// File the receipt of a decision whose effect has now actually
    /// happened — the close that ended a terminal, the wait that timed out
    /// on its own empty answer. The decision is one this actor handed out;
    /// the shell brings it back once the world matches it.
    ServeReceipt { decided: Box<Decided>, now_ms: i64 },
    HandoverRevoked {
        run: String,
        handover: String,
        now_ms: i64,
    },
    /// Settle a worker only while its actual terminal incarnation still stands.
    WorkerTerminalSettled {
        seat: zerocode_core::orchestration::WorkerSeat,
        term: u32,
        capability: String,
        stop: Option<String>,
        screen: Option<String>,
        handover: Option<Box<zerocode_core::orchestration::HandoverPlan>>,
        fence: Option<EffectFence>,
        now_ms: i64,
    },
    /// A planned release could not prove its caller or target. Settle only
    /// that reservation and file its retry answer in the same transaction.
    ReleaseUnobserved { decided: Box<Decided>, now_ms: i64 },
    /// A release read its screen (or could not): write the worker's end.
    /// `screen: Some` is the shell saying the read completed against the
    /// same incarnation it started on; `None` is `release_unknown` — asked
    /// and not answered.
    ReleaseSettled {
        worker: String,
        screen: Option<String>,
        now_ms: i64,
    },
    /// Remember what a named request was told, for an answer built after the
    /// effect — the release's own JSON, which does not exist until the
    /// screen has been read and the terminal closed.
    RememberServed {
        key: Box<ReceiptKey>,
        stdout: String,
        now_ms: i64,
    },
    /// Settle the ONE in-flight operation this actor recovered at boot.
    ///
    /// The permit for a recovered operation never left the actor — handing
    /// it out would be handing out the journal — so the caller can only
    /// bring the OUTCOME, and the actor pairs it with the permit it already
    /// holds. This is the door that lifts the recovery fence; without it a
    /// window that died mid-effect would refuse every mutation forever.
    SettleRecovered { settlement: EffectSettlement },
}

impl std::fmt::Debug for RuntimeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::View => formatter.write_str("RuntimeRequest::View"),
            Self::Plan(command) => command.fmt(formatter),
            Self::TerminalGone {
                term,
                screen,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::TerminalGone")
                .field("term", term)
                .field(
                    "screen",
                    &format_args!("<{} bytes>", screen.as_deref().map_or(0, str::len)),
                )
                .field("now_ms", now_ms)
                .finish(),
            Self::PaneTurnEnded {
                term,
                turn_started_ms,
                interrupted,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::PaneTurnEnded")
                .field("term", term)
                .field("turn_started_ms", turn_started_ms)
                .field("interrupted", interrupted)
                .field("now_ms", now_ms)
                .finish(),
            Self::QuietSweep { stalled, now_ms } => formatter
                .debug_struct("RuntimeRequest::QuietSweep")
                .field("workers", &stalled.len())
                .field("now_ms", now_ms)
                .finish(),
            // A count: the witness carries the agent's own words.
            Self::QuotaWalls { walled, now_ms } => formatter
                .debug_struct("RuntimeRequest::QuotaWalls")
                .field("workers", &walled.len())
                .field("now_ms", now_ms)
                .finish(),
            // A count, for the same reason: a lift carries the agent's words.
            Self::QuotaLifts { lifted, now_ms } => formatter
                .debug_struct("RuntimeRequest::QuotaLifts")
                .field("workers", &lifted.len())
                .field("now_ms", now_ms)
                .finish(),
            // Counts, for the same reason: both carry the agent's words.
            Self::ClassifierDeclines { declined, now_ms } => formatter
                .debug_struct("RuntimeRequest::ClassifierDeclines")
                .field("workers", &declined.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::ModelDeviations { deviated, now_ms } => formatter
                .debug_struct("RuntimeRequest::ModelDeviations")
                .field("switches", &deviated.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::StallCauses { judged, now_ms } => formatter
                .debug_struct("RuntimeRequest::StallCauses")
                .field("workers", &judged.len())
                .field("now_ms", now_ms)
                .finish(),
            // Ids only: the plan carries a task's spec.
            Self::HandoverBegin { plan, now_ms } => formatter
                .debug_struct("RuntimeRequest::HandoverBegin")
                .field("run", &plan.run)
                .field("dispatch", &plan.dispatch)
                .field("now_ms", now_ms)
                .finish(),
            Self::HandoverStep {
                run,
                handover,
                step,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::HandoverStep")
                .field("run", run)
                .field("handover", handover)
                .field("step", &step.name)
                .field("ok", &step.ok)
                .field("now_ms", now_ms)
                .finish(),
            Self::HandoverSettle {
                run,
                handover,
                replacement,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::HandoverSettle")
                .field("run", run)
                .field("handover", handover)
                .field("replaced", &replacement.is_some())
                .field("now_ms", now_ms)
                .finish(),
            // Ids only: the marker carries the provider's sentence.
            Self::ResumeBegin { plan, now_ms } => formatter
                .debug_struct("RuntimeRequest::ResumeBegin")
                .field("run", &plan.run)
                .field("dispatch", &plan.dispatch)
                .field("attempt", &plan.attempt)
                .field("now_ms", now_ms)
                .finish(),
            Self::ResumeSettle {
                run,
                resume,
                outcome,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::ResumeSettle")
                .field("run", run)
                .field("resume", resume)
                .field("submitted", &outcome.submitted)
                .field("typed", &outcome.typed)
                .field("now_ms", now_ms)
                .finish(),
            // Team and pane ids, not prose — nothing here can spill a word
            // an agent wrote.
            Self::ReadinessSweep {
                spoken,
                delivery_failures,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::ReadinessSweep")
                .field("spoken", spoken)
                .field("delivery_failures", delivery_failures)
                .field("now_ms", now_ms)
                .finish(),
            Self::PanesMissing { missing, now_ms } => formatter
                .debug_struct("RuntimeRequest::PanesMissing")
                .field("workers", &missing.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::CheckoutExamined {
                worker,
                examined,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::CheckoutExamined")
                .field("worker", worker)
                .field("examined", examined)
                .field("now_ms", now_ms)
                .finish(),
            Self::PanesSeen { workers, now_ms } => formatter
                .debug_struct("RuntimeRequest::PanesSeen")
                .field("workers", &workers.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::Observation {
                to, body, now_ms, ..
            } => formatter
                .debug_struct("RuntimeRequest::Observation")
                .field("to", to)
                .field("body_bytes", &body.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::RetentionSweep { now_ms } => formatter
                .debug_struct("RuntimeRequest::RetentionSweep")
                .field("now_ms", now_ms)
                .finish(),
            Self::PaneTakenOver { term, now_ms } => formatter
                .debug_struct("RuntimeRequest::PaneTakenOver")
                .field("term", term)
                .field("now_ms", now_ms)
                .finish(),
            Self::WindowExiting { now_ms } => formatter
                .debug_struct("RuntimeRequest::WindowExiting")
                .field("now_ms", now_ms)
                .finish(),
            Self::WorkerPaneResumed {
                term,
                checkout,
                agent,
                session_id,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::WorkerPaneResumed")
                .field("term", term)
                .field("checkout_bytes", &checkout.len())
                .field("agent", agent)
                .field("session_id_bytes", &session_id.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::SleeperExpired { worker, now_ms } => formatter
                .debug_struct("RuntimeRequest::SleeperExpired")
                .field("worker", worker)
                .field("now_ms", now_ms)
                .finish(),
            Self::WorkerSessionReported {
                term,
                session,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::WorkerSessionReported")
                .field("term", term)
                // A provider id is private restart material. Lengths prove the
                // request is bounded without putting the id in a debug log.
                .field("session_bytes", &session.id.len())
                .field(
                    "transcript_bytes",
                    &session.transcript_path.as_ref().map(String::len),
                )
                .field("now_ms", now_ms)
                .finish(),
            Self::Federation(call) => formatter
                .debug_struct("RuntimeRequest::Federation")
                .field("call", &std::mem::discriminant(call))
                .finish(),
            Self::WorkerSeated {
                team,
                pane,
                checkout,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::WorkerSeated")
                .field("team", &team.len())
                .field("pane", &pane.len())
                .field("checkout", &checkout.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::CoordinatorReturned {
                run,
                team,
                pane,
                actor,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::CoordinatorReturned")
                .field("run", run)
                .field("team", &team.len())
                .field("pane", &pane.len())
                .field("named_actor", &actor.is_some())
                .field("now_ms", now_ms)
                .finish(),
            Self::PrepareWorkerReseat {
                run,
                worker,
                team,
                coordinator_pane,
                leader_term,
                resume_nudge,
            } => formatter
                .debug_struct("RuntimeRequest::PrepareWorkerReseat")
                .field("run_bytes", &run.len())
                .field("worker_bytes", &worker.len())
                .field("team_bytes", &team.len())
                .field("coordinator_pane_bytes", &coordinator_pane.len())
                .field("leader_term", leader_term)
                .field("resume_nudge_bytes", &resume_nudge.len())
                .finish(),
            Self::WorkerReseated {
                worker,
                team,
                pane,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::WorkerReseated")
                .field("worker_bytes", &worker.len())
                .field("team_bytes", &team.len())
                .field("pane_bytes", &pane.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::FinishSleepingReseat {
                worker,
                reason,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::FinishSleepingReseat")
                .field("worker_bytes", &worker.len())
                .field("reason_bytes", &reason.len())
                .field("now_ms", now_ms)
                .finish(),
            /* A worker id is minted from a counter, not typed by a person —
             * but the habit is lengths for anything that names. */
            Self::SeatNeverOpened { worker, now_ms } => formatter
                .debug_struct("RuntimeRequest::SeatNeverOpened")
                .field("worker_bytes", &worker.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::PrepareEffect { request, now_ms } => formatter
                .debug_struct("RuntimeRequest::PrepareEffect")
                .field("request", request)
                .field("now_ms", now_ms)
                .finish(),
            Self::SettleEffect { permit, settlement } => formatter
                .debug_struct("RuntimeRequest::SettleEffect")
                .field("permit", permit)
                .field("settlement", settlement)
                .finish(),
            Self::WindowRestarted { now_ms } => formatter
                .debug_struct("RuntimeRequest::WindowRestarted")
                .field("now_ms", now_ms)
                .finish(),
            Self::TeamDissolved { team, now_ms } => formatter
                .debug_struct("RuntimeRequest::TeamDissolved")
                .field("team_bytes", &team.len())
                .field("now_ms", now_ms)
                .finish(),
            /* An inbox address is a name somebody chose; sizes only, the
             * habit every named thing here keeps. */
            Self::LookAgain {
                waiting,
                receipt,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::LookAgain")
                .field("run_bytes", &waiting.run.len())
                .field("address_bytes", &waiting.address.len())
                .field("kinds", &waiting.kinds.len())
                .field("receipt", &receipt.is_some())
                .field("now_ms", now_ms)
                .finish(),
            /* The decision carries a reply somebody will read on a screen —
             * prose. A log learns that one came back, not what it said. */
            Self::ServeReceipt { decided, now_ms } => formatter
                .debug_struct("RuntimeRequest::ServeReceipt")
                .field("receipt", &decided.receipt.is_some())
                .field("now_ms", now_ms)
                .finish(),
            Self::HandoverRevoked { now_ms, .. } => formatter
                .debug_struct("RuntimeRequest::HandoverRevoked")
                .field("now_ms", now_ms)
                .finish(),
            Self::WorkerTerminalSettled { now_ms, .. } => formatter
                .debug_struct("RuntimeRequest::WorkerTerminalSettled")
                .field("now_ms", now_ms)
                .finish(),
            Self::HandoverSplit { .. } => formatter.write_str("RuntimeRequest::HandoverSplit"),
            Self::ReleaseUnobserved { now_ms, .. } => formatter
                .debug_struct("RuntimeRequest::ReleaseUnobserved")
                .field("now_ms", now_ms)
                .finish(),
            Self::ReleaseSettled {
                worker,
                screen,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::ReleaseSettled")
                .field("worker_bytes", &worker.len())
                .field("screen_bytes", &screen.as_ref().map(String::len))
                .field("now_ms", now_ms)
                .finish(),
            Self::RememberServed {
                key: _,
                stdout,
                now_ms,
            } => formatter
                .debug_struct("RuntimeRequest::RememberServed")
                .field("key", &"<receipt-key>")
                .field("stdout_bytes", &stdout.len())
                .field("now_ms", now_ms)
                .finish(),
            Self::SettleRecovered { settlement } => formatter
                .debug_struct("RuntimeRequest::SettleRecovered")
                .field("settlement", settlement)
                .finish(),
        }
    }
}

/// Typed answer corresponding to one [`RuntimeRequest`].
///
/// Deliberately neither `Clone` nor comparable: an [`Effect`] answer carries
/// an [`EffectPermit`], and a permit is one-use paper — a reply that could
/// be cloned would be a capability that could.
///
/// [`Effect`]: RuntimeReply::Effect
/// The federation verbs, as the actor takes them. Reads carry no clock;
/// mutations do, and are written through before they answer.
#[derive(Debug, Clone)]
pub enum FederationCall {
    EnsureRun {
        home: String,
        now_ms: i64,
    },
    Attach {
        run: String,
        dispatch: String,
        home: String,
        worker: String,
        now_ms: i64,
    },
    Pull {
        dispatch: String,
        home: String,
        after_seq: i64,
        limit: usize,
    },
    Ack {
        dispatch: String,
        home: String,
        through_seq: i64,
        settlements: Vec<zerocode_core::orchestration::Settlement>,
        now_ms: i64,
    },
    Import {
        dispatch: String,
        home: String,
        items: Vec<zerocode_core::orchestration::RelayItem>,
        now_ms: i64,
    },
    Stop {
        dispatch: String,
        home: String,
        now_ms: i64,
    },
    StopUnknown {
        dispatch: String,
        home: String,
        now_ms: i64,
    },
    Absorb {
        run: String,
        dispatch: String,
        items: Vec<zerocode_core::orchestration::RelayItem>,
        now_ms: i64,
    },
    Outbox {
        run: String,
        dispatch: String,
    },
    /// The attach never reached the server whole: unwind the reservation.
    AbortRemote {
        run: String,
        dispatch: String,
        task_preimage: zerocode_core::orchestration::Task,
        now_ms: i64,
    },
    /// Every live remote seat on the home side, for the relay loop.
    HomeSeats,
    Exported {
        run: String,
        dispatch: String,
        through_seq: i64,
        now_ms: i64,
    },
}

/// What one federation call answered with.
#[derive(Debug, Clone)]
pub enum FederationAnswer {
    Run(String),
    Attached,
    Items(Vec<zerocode_core::orchestration::RelayItem>),
    Cursor(i64),
    Stopped {
        state: zerocode_core::orchestration::AttachmentState,
        worker: Option<String>,
    },
    Done,
    /// `(run, dispatch, server, absorbed_seq)` for every open federated
    /// dispatch — the cursor rides along so the relay loop pulls past it
    /// without a second question.
    Seats(Vec<(String, String, String, i64)>),
    /// The ledger's own sentence, carried whole so the transport can hand
    /// it to the far side verbatim — a refusal is an answer, not a fault.
    Refused(String),
}

#[derive(Debug)]
pub enum RuntimeReply {
    /// The verb was carried out AND made durable. A decision that could not be
    /// written never becomes one of these.
    Planned {
        decided: Box<Decided>,
        revision: u64,
    },
    /// A host-only reseat plan. The inner refusal is ledger/catalog prose and
    /// changes nothing; the outer runtime result still protects authority.
    ReseatPlanned {
        decided: Result<Box<Decided>, String>,
        revision: u64,
    },
    /// The ledger, to look at.
    View(RuntimeImage),
    /// A host fact was applied — and made durable iff it moved anything.
    /// `moved: false` is a real answer: the seat had nothing left to
    /// settle, and the revision did not move for it.
    Settled { moved: bool, revision: u64 },
    /// A reservation's answer — a handover's or a continuation's: the
    /// receipt row's id, made durable — or `None`, the ledger's own refusal
    /// (the attempt ended, a person took the pane, the ceiling), with nothing
    /// written.
    Reserved { id: Option<String>, revision: u64 },
    /// What the journal said about a reserved effect.
    Effect { begun: BeginEffect, revision: u64 },
    /// What the restart sweep did, durably.
    Swept {
        swept: zerocode_core::orchestration::Restarted,
        revision: u64,
    },
    /// Whom a resumed pane turned out to be — the sleeper seated again by
    /// that witness, durably, or nobody (t-3058).
    Witnessed {
        worker: Option<String>,
        revision: u64,
    },
    /// What a woken wait found: an answer with its delivery taken and its
    /// receipt remembered, or nothing — still waiting, nothing written.
    Looked {
        found: Option<Box<Decided>>,
        revision: u64,
    },
    /// How a released worker ended, in the ledger's own word for it.
    Release { state: WorkerState, revision: u64 },
    /// What a federation call answered, beside the revision it speaks for.
    Federation {
        answer: FederationAnswer,
        revision: u64,
    },
    /// What a retention sweep gave up, beside the revision it speaks for.
    /// Counts only — a sweep never carries a word anybody wrote.
    Retention { swept: Sweep, revision: u64 },
}

impl RuntimeReply {
    /// Which revision this answer speaks for.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        match self {
            Self::Planned { revision, .. }
            | Self::ReseatPlanned { revision, .. }
            | Self::Settled { revision, .. }
            | Self::Effect { revision, .. }
            | Self::Swept { revision, .. }
            | Self::Looked { revision, .. }
            | Self::Release { revision, .. }
            | Self::Federation { revision, .. }
            | Self::Retention { revision, .. }
            | Self::Reserved { revision, .. }
            | Self::Witnessed { revision, .. } => *revision,
            Self::View(image) => image.revision,
        }
    }
}

/// Non-secret description of an effect that must be reconciled after reopen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeRecovery {
    kind: HostEffectKind,
    state: HostEffectState,
    attempt: u64,
    prepared_revision: u64,
}

impl RuntimeRecovery {
    fn from_permit(permit: &EffectPermit) -> Self {
        Self {
            kind: permit.kind(),
            state: permit.state(),
            attempt: permit.attempt(),
            prepared_revision: permit.prepared_revision(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> HostEffectKind {
        self.kind
    }

    #[must_use]
    pub const fn state(&self) -> HostEffectState {
        self.state
    }

    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    #[must_use]
    pub const fn prepared_revision(&self) -> u64 {
        self.prepared_revision
    }
}

/// Cloneable read image. Debug exposes sizes and states, never bytes, digests,
/// operation ids, host bindings, or the private authority path.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeImage {
    revision: u64,
    projection: Arc<LedgerProjectionV1>,
    recoveries: Vec<RuntimeRecovery>,
    repairs: Vec<String>,
}

impl std::fmt::Debug for RuntimeImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        /* Counts and states. What the rows SAY — a name somebody typed, an
         * agent's report, an address — never appears here. */
        formatter
            .debug_struct("RuntimeImage")
            .field("revision", &self.revision)
            .field("runs", &self.projection.runs.len())
            .field("tasks", &self.projection.tasks.len())
            .field("workers", &self.projection.workers.len())
            .field("messages", &self.projection.messages.len())
            .field("receipts", &self.projection.served.len())
            .field("recovery_count", &self.recoveries.len())
            .field("repair_count", &self.repairs.len())
            .field("recoveries", &self.recoveries)
            .finish()
    }
}

impl RuntimeImage {
    /// Which revision these rows are.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// The rows themselves, to look at.
    #[must_use]
    pub fn projection(&self) -> &LedgerProjectionV1 {
        &self.projection
    }

    #[must_use]
    pub fn recoveries(&self) -> &[RuntimeRecovery] {
        &self.recoveries
    }

    /// Repairs committed by this boot, for the window's diagnostic log.
    #[must_use]
    pub fn repairs(&self) -> &[String] {
        &self.repairs
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("the runtime mailbox capacity is outside its bound")]
    InvalidMailboxCapacity,
    #[error("the orchestration ledger has not been imported")]
    MissingLedger,
    #[error("the runtime actor is closed")]
    Closed,
    #[error("the runtime actor panicked and stopped")]
    Panicked,
    #[error("the runtime actor thread could not be created")]
    SpawnFailed,
    #[error("the authority store is unavailable")]
    StoreUnavailable,
    #[error(
        "the authority store failed storage integrity validation (SQLite rows, byte count, or table digests)"
    )]
    StoreCorrupt,
    #[error("the authority store's ledger content violates an invariant: {0}")]
    LedgerInvariant(RebuildError),
    #[error("the runtime request is invalid")]
    InvalidInput,
    #[error("the runtime does not know this team")]
    UnknownTeam,
    /// One answer for unknown team, unknown pane and wrong capability — the
    /// same single sentence the shell has always given, so a caller cannot
    /// probe which of the three it guessed.
    #[error("stale or unauthorized agent pane")]
    UnauthorizedSeat,
    #[error("the seat this worker sits in is open on the host")]
    SeatStillOpen,
    #[error("the runtime revision is stale or non-monotonic")]
    RevisionMismatch,
    #[error("the legacy import conflicts with the durable authority")]
    ImportConflict,
    #[error("the fresh ledger conflicts with the durable authority")]
    InitializationConflict,
    #[error("host-effect recovery must finish before another mutation")]
    RecoveryRequired,
    /// A pane is being cut on the lane RIGHT NOW — a walk to wait out, not a
    /// crash to recover. Kept apart from [`RuntimeError::RecoveryRequired`]
    /// so a caller can wait in line instead of telling the user a recovery
    /// story that is not true.
    #[error("another pane is still being cut — ask again in a moment")]
    EffectInFlight,
    #[error("more than one host effect is in flight for one ledger")]
    MultipleInFlightEffects,
    #[error("the runtime timestamp would move backwards")]
    TimestampRegression,
    #[error("the durable authority changed outside its owner actor")]
    AuthorityChanged,
    #[error("the durable authority refused this transition")]
    AuthorityRejected,
    /// The verb was carried out in memory and the store would not take it.
    ///
    /// Deliberately NOT fatal. The runtime keeps its thread and refuses every
    /// request — reads included — until it can read the store again, because
    /// its ledger is ahead of the disk by exactly the change that did not
    /// land. A passing fault should not cost a restart, and an actor that
    /// closed on one would take the pane table and the launcher with it.
    #[error("the ledger could not be made durable, and nothing will be answered until it can")]
    NotDurable,
}

impl RuntimeError {
    /// Whether this ends the actor, or is answered and survived.
    ///
    /// [`Self::StoreUnavailable`] is NOT fatal, and the write road next door
    /// says why: a refused write poisons the runtime and the next request
    /// takes the disk's word for it, "so a passing failure does not cost a
    /// restart". A store that could not be OPENED deserves the same reading,
    /// and more easily — a connection that never opened read nothing and
    /// wrote nothing, so this actor's state is exactly what it was. Treating
    /// it as fatal promoted a passing condition into a permanent one: a full
    /// disk stopped one sqlite open, the actor returned, and the ledger
    /// stayed shut after the disk was freed, with the window still running
    /// and the database intact (2026-09-18 — `integrity_check: ok`, WAL 0
    /// bytes, nothing left to reopen it but a restart nobody but a person can
    /// order).
    ///
    /// Continuing would be a lie when stored bytes or ledger content are
    /// invalid, ownership changed, or two effects are in flight at once.
    const fn fatal(&self) -> bool {
        matches!(
            self,
            Self::StoreCorrupt
                | Self::LedgerInvariant(_)
                | Self::AuthorityChanged
                | Self::MultipleInFlightEffects
                | Self::Panicked
        )
    }
}

enum ActorCommand {
    Request {
        /* Boxed for the mailbox: `PrepareEffect` carries an
         * `EffectRequest`, and the box keeps every waiting command the size
         * of a pointer instead of the size of the widest variant. */
        request: Box<RuntimeRequest>,
        reply: SyncSender<Result<RuntimeReply, RuntimeError>>,
    },
    Shutdown {
        reply: SyncSender<Result<RuntimeImage, RuntimeError>>,
    },
    #[cfg(test)]
    Hold {
        entered: SyncSender<()>,
        release: Receiver<()>,
        finished: SyncSender<()>,
    },
    #[cfg(test)]
    Panic,
    #[cfg(test)]
    Close,
}

#[derive(Debug, Clone)]
enum TerminalState {
    Running,
    Closed,
    Failed(RuntimeError),
    Panicked,
}

struct Terminal {
    state: Mutex<TerminalState>,
    changed: Condvar,
}

impl Terminal {
    fn new() -> Self {
        Self {
            state: Mutex::new(TerminalState::Running),
            changed: Condvar::new(),
        }
    }

    fn finish(&self, state: TerminalState) {
        let mut current = self.state.lock().unwrap_or_else(|held| held.into_inner());
        *current = state;
        self.changed.notify_all();
    }

    fn failure(&self) -> RuntimeError {
        let mut current = self.state.lock().unwrap_or_else(|held| held.into_inner());
        while matches!(*current, TerminalState::Running) {
            current = self
                .changed
                .wait(current)
                .unwrap_or_else(|held| held.into_inner());
        }
        match &*current {
            TerminalState::Running => unreachable!(),
            TerminalState::Closed => RuntimeError::Closed,
            TerminalState::Failed(error) => error.clone(),
            TerminalState::Panicked => RuntimeError::Panicked,
        }
    }
}

/// Handle to one single-owner runtime thread.
pub struct RuntimeActor {
    sender: Option<SyncSender<ActorCommand>>,
    join: Option<JoinHandle<Result<RuntimeImage, RuntimeError>>>,
    terminal: Arc<Terminal>,
}

impl std::fmt::Debug for RuntimeActor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RuntimeActor(<single-owner-authority>)")
    }
}

impl RuntimeActor {
    /// Start the one thread that owns this ledger.
    ///
    /// The pane table and the launcher come in here rather than with each
    /// command: they move WITH the ledger, because the road moves all three
    /// together and a copy of either living outside would be a second answer
    /// to the same question.
    pub fn start(
        store: &WorkflowStore,
        ledger_id: impl Into<String>,
        boot: RuntimeBoot,
        mailbox_capacity: usize,
        panes: Box<dyn PaneTable>,
        launcher: Box<dyn Launcher + Send>,
    ) -> Result<Self, RuntimeError> {
        if mailbox_capacity == 0 || mailbox_capacity > MAX_RUNTIME_MAILBOX {
            return Err(RuntimeError::InvalidMailboxCapacity);
        }
        let ledger_id = ledger_id.into();
        let store = store.clone();
        let (sender, receiver) = mpsc::sync_channel(mailbox_capacity);
        // Rendezvous replies prevent processed ledger images from accumulating
        // outside the bounded request mailbox when caller threads are paused.
        let (boot_sender, boot_receiver) = mpsc::sync_channel(0);
        let terminal = Arc::new(Terminal::new());
        let thread_terminal = Arc::clone(&terminal);
        let join = thread::Builder::new()
            .name("zerocode-runtime-actor".to_string())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    actor_main(
                        store,
                        ledger_id,
                        boot,
                        panes,
                        launcher,
                        receiver,
                        boot_sender,
                    )
                }));
                match outcome {
                    Ok(Ok(image)) => {
                        thread_terminal.finish(TerminalState::Closed);
                        Ok(image)
                    }
                    Ok(Err(error)) => {
                        thread_terminal.finish(TerminalState::Failed(error.clone()));
                        Err(error)
                    }
                    Err(_) => {
                        thread_terminal.finish(TerminalState::Panicked);
                        Err(RuntimeError::Panicked)
                    }
                }
            })
            .map_err(|_| RuntimeError::SpawnFailed)?;
        let mut actor = Self {
            sender: Some(sender),
            join: Some(join),
            terminal,
        };
        match boot_receiver.recv() {
            Ok(Ok(_)) => Ok(actor),
            Ok(Err(error)) => {
                actor.sender.take();
                let _ = actor.join_actor();
                Err(error)
            }
            Err(_) => {
                let error = actor.terminal.failure();
                actor.sender.take();
                let _ = actor.join_actor();
                Err(error)
            }
        }
    }

    pub fn request(&self, request: RuntimeRequest) -> Result<RuntimeReply, RuntimeError> {
        let (reply, answer) = mpsc::sync_channel(0);
        let command = ActorCommand::Request {
            request: Box::new(request),
            reply,
        };
        let sender = self.sender.as_ref().ok_or(RuntimeError::Closed)?;
        /* A full mailbox WAITS instead of refusing. The bound is backpressure
         * — at most `mailbox_capacity` commands queued, callers past that
         * standing in line — not an answer: a window under a burst of verbs
         * would otherwise scatter refusals across whichever callers lost the
         * race, and every one of them would simply have asked again. The
         * wait is bounded by the actor draining, which it always is: the
         * actor never sends into its own mailbox, and no caller holds the
         * pane table while standing here (the table is the actor's to
         * take). */
        if sender.send(command).is_err() {
            return Err(self.terminal.failure());
        }
        answer
            .recv()
            .unwrap_or_else(|_| Err(self.terminal.failure()))
    }

    fn pending_effect(
        &self,
        request: impl FnOnce(EffectFence) -> RuntimeRequest,
    ) -> Result<PendingEffect, RuntimeError> {
        let (entered, at_effect) = mpsc::sync_channel(0);
        let (proceed, ready) = mpsc::sync_channel(1);
        let (reply, answer) = mpsc::sync_channel(0);
        let request = request(EffectFence {
            entered,
            proceed: ready,
        });
        self.sender
            .as_ref()
            .ok_or(RuntimeError::Closed)?
            .send(ActorCommand::Request {
                request: Box::new(request),
                reply,
            })
            .map_err(|_| self.terminal.failure())?;
        Ok(PendingEffect {
            entered: at_effect,
            proceed,
            answer,
        })
    }

    /// The actor owns the current declaration until `apply` returns. The
    /// callback must not call the actor; pane identity is fenced by the split
    /// lease inside it. No host action runs if the queued authority expired.
    pub fn handover_split<R>(
        &self,
        prepared: zerocode_core::orchestration::PreparedWorkerStart,
        operation: String,
        now_ms: i64,
        apply: impl FnOnce() -> R,
    ) -> Result<R, RuntimeError> {
        let pending = self.pending_effect(|fence| RuntimeRequest::HandoverSplit {
            prepared: Box::new(prepared),
            operation,
            fence,
        })?;
        let applied = if pending.entered.recv().is_ok() {
            let applied = apply();
            let _ = pending.proceed.send(now_ms);
            Some(applied)
        } else {
            None
        };
        pending
            .answer
            .recv()
            .unwrap_or_else(|_| Err(self.terminal.failure()))?;
        applied.ok_or(RuntimeError::AuthorityRejected)
    }

    /// The ledger as it stands.
    pub fn view(&self) -> Result<RuntimeImage, RuntimeError> {
        match self.request(RuntimeRequest::View)? {
            RuntimeReply::View(image) => Ok(image),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Carry out one verb and hand back what was decided.
    ///
    /// What comes back is a decision the store has already accepted. A verb
    /// whose ledger could not be written comes back as an error and its
    /// decision is never seen — the window must not carry out an effect for a
    /// write the disk refused.
    pub fn plan(&self, command: PlanCommand) -> Result<(Box<Decided>, u64), RuntimeError> {
        match self.request(RuntimeRequest::Plan(command))? {
            RuntimeReply::Planned { decided, revision } => Ok((decided, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A terminal the host closed. Answers whether anything settled, and
    /// the revision that answer speaks for.
    pub fn terminal_gone(&self, term: u32, now_ms: i64) -> Result<(bool, u64), RuntimeError> {
        self.terminal_gone_with_archive(term, None, now_ms)
    }

    pub fn terminal_gone_with_archive(
        &self,
        term: u32,
        screen: Option<String>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::TerminalGone {
            term,
            screen,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A turn ended in a seat. Answers whether a silence was written, and
    /// the revision that answer speaks for.
    pub fn pane_turn_ended(
        &self,
        term: u32,
        turn_started_ms: i64,
        interrupted: bool,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::PaneTurnEnded {
            term,
            turn_started_ms,
            interrupted,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The beat's confirmed stall observations. Answers whether a due notice
    /// was written, and the revision that answer speaks for.
    pub fn quiet_sweep(
        &self,
        stalled: Vec<(String, i64)>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::QuietSweep { stalled, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The beat's stall-seat readings. Answers whether a notice was written,
    /// and the revision that answer speaks for; the same reading of the same
    /// silence on the next beat moves nothing.
    pub fn stall_causes(
        &self,
        judged: Vec<zerocode_core::orchestration::StallJudged>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::StallCauses { judged, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The beat's quota-wall witnesses. Answers whether a notice was
    /// written, and the revision that answer speaks for; the same wall on
    /// the next beat is the same fact and moves nothing.
    pub fn quota_walls(
        &self,
        walled: Vec<zerocode_core::orchestration::QuotaWallWitness>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::QuotaWalls { walled, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The beat's lifted walls (t-6427). Answers whether a notice was
    /// written, and the revision that answer speaks for; a wall already told
    /// moves nothing.
    pub fn quota_lifts(
        &self,
        lifted: Vec<zerocode_core::orchestration::QuotaLift>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::QuotaLifts { lifted, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The beat's classifier-decline witnesses (t-6747). Answers whether a
    /// notice was written, and the revision that answer speaks for; the same
    /// decline on the next beat is the same fact and moves nothing.
    pub fn classifier_declines(
        &self,
        declined: Vec<zerocode_core::orchestration::ClassifierDeclineWitness>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::ClassifierDeclines { declined, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The switches of model the beat read (t-6747). Answers whether a row
    /// was written; a switch already written moves nothing.
    pub fn model_deviations(
        &self,
        deviated: Vec<zerocode_core::orchestration::ModelDeviation>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::ModelDeviations { deviated, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Reserve one handover. Answers the receipt row's id, or `None` when
    /// the ledger refused (the attempt ended, a person took the pane, the
    /// ceiling) and nothing was written — the beat plans again next tick.
    pub fn handover_begin(
        &self,
        plan: zerocode_core::orchestration::HandoverPlan,
        now_ms: i64,
    ) -> Result<(Option<String>, u64), RuntimeError> {
        match self.request(RuntimeRequest::HandoverBegin {
            plan: Box::new(plan),
            now_ms,
        })? {
            RuntimeReply::Reserved { id, revision } => Ok((id, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    pub fn handover_revoke(
        &self,
        run: String,
        handover: String,
        now_ms: i64,
    ) -> Result<(), RuntimeError> {
        self.request(RuntimeRequest::HandoverRevoked {
            run,
            handover,
            now_ms,
        })?;
        Ok(())
    }

    /// One walked step onto a handover's receipt.
    pub fn handover_step(
        &self,
        run: String,
        handover: String,
        step: zerocode_core::orchestration::HandoverStep,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::HandoverStep {
            run,
            handover,
            step,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Reserve one transient-error continuation. Answers the receipt row's
    /// id, or `None` when the ledger refused and nothing was written.
    pub fn resume_begin(
        &self,
        plan: zerocode_core::orchestration::ResumePlan,
        now_ms: i64,
    ) -> Result<(Option<String>, u64), RuntimeError> {
        match self.request(RuntimeRequest::ResumeBegin {
            plan: Box::new(plan),
            now_ms,
        })? {
            RuntimeReply::Reserved { id, revision } => Ok((id, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Settle and deliver a continuation's receipt.
    pub fn resume_settle(
        &self,
        run: String,
        resume: String,
        outcome: zerocode_core::orchestration::ResumeOutcome,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::ResumeSettle {
            run,
            resume,
            outcome,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The walk is over: settle and deliver the receipt.
    pub fn handover_settle(
        &self,
        run: String,
        handover: String,
        replacement: Option<(String, String)>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::HandoverSettle {
            run,
            handover,
            replacement,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The beat's readiness sweep. Answers whether anybody was told, and the
    /// revision that answer speaks for. Delivery failures are level-triggered;
    /// the ledger transition makes repeated markers a no-op.
    pub fn readiness_sweep(
        &self,
        spoken: Vec<u32>,
        delivery_failures: Vec<(u32, i64)>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::ReadinessSweep {
            spoken,
            delivery_failures,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Reconcile confirmed missing panes without changing worker lifecycle.
    pub fn panes_missing(
        &self,
        missing: Vec<(String, i64)>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::PanesMissing { missing, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Clear missing-pane episodes whose panes the host sees again.
    pub fn panes_seen(
        &self,
        workers: Vec<String>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::PanesSeen { workers, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The beat's retention sweep.
    ///
    /// Answers what it gave up and the revision that answer speaks for. A
    /// sweep that was not yet due, or that found nothing old enough, is a
    /// no-op: it writes nothing and reports the revision the caller already
    /// knew, so a beat costs a lock and not a disk.
    pub fn checkout_examined(
        &self,
        worker: String,
        examined: zerocode_core::orchestration::Examined,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::CheckoutExamined {
            worker,
            examined,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Mail a window observation to a ledger address (t-2733). `moved` is
    /// whether any run seated a reader for it.
    pub fn observation(
        &self,
        to: String,
        body: String,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        self.observation_once(to, body, None, now_ms)
    }

    pub fn observation_once(
        &self,
        to: String,
        body: String,
        receipt: Option<String>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::Observation {
            to,
            body,
            receipt,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    pub fn retention_sweep(&self, now_ms: i64) -> Result<(Sweep, u64), RuntimeError> {
        match self.request(RuntimeRequest::RetentionSweep { now_ms })? {
            RuntimeReply::Retention { swept, revision } => Ok((swept, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A person's hand landed real keys in a terminal. Answers whether a
    /// worker's pane became theirs, and the revision that answer speaks for.
    pub fn pane_taken_over(&self, term: u32, now_ms: i64) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::PaneTakenOver { term, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The window is exiting: every seated worker sleeps now (t-3058).
    /// Answers what slept, and the revision that answer speaks for.
    pub fn window_exiting(
        &self,
        now_ms: i64,
    ) -> Result<(zerocode_core::orchestration::Restarted, u64), RuntimeError> {
        match self.request(RuntimeRequest::WindowExiting { now_ms })? {
            RuntimeReply::Swept { swept, revision } => Ok((swept, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The window resumed a conversation into `term` (t-3058). Answers the
    /// sleeper that conversation was, seated again durably — or `None` for
    /// a pane that is nobody's sleeper — and the revision that speaks for.
    pub fn worker_pane_resumed(
        &self,
        term: u32,
        checkout: impl Into<String>,
        agent: impl Into<String>,
        session_id: impl Into<String>,
        now_ms: i64,
    ) -> Result<(Option<String>, u64), RuntimeError> {
        match self.request(RuntimeRequest::WorkerPaneResumed {
            term,
            checkout: checkout.into(),
            agent: agent.into(),
            session_id: session_id.into(),
            now_ms,
        })? {
            RuntimeReply::Witnessed { worker, revision } => Ok((worker, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A sleeper nothing resumed within the grace ends, announced (t-3058).
    pub fn sleeper_expired(
        &self,
        worker: impl Into<String>,
        now_ms: i64,
    ) -> Result<u64, RuntimeError> {
        match self.request(RuntimeRequest::SleeperExpired {
            worker: worker.into(),
            now_ms,
        })? {
            RuntimeReply::Settled { revision, .. } => Ok(revision),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The provider conversation observed in a pane, written durably when
    /// that pane belongs to a ledger worker.
    pub fn worker_session_reported(
        &self,
        term: u32,
        session: ProviderSession,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::WorkerSessionReported {
            term,
            session,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// One federation call, ledger law only — the wire stays with the
    /// caller. Mutations are durable before they answer.
    pub fn federation(
        &self,
        call: FederationCall,
    ) -> Result<(FederationAnswer, u64), RuntimeError> {
        match self.request(RuntimeRequest::Federation(call))? {
            RuntimeReply::Federation { answer, revision } => Ok((answer, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The window reports the checkout under a summoned pane. Answers
    /// whether a worker row took the fact, and the revision that answer
    /// speaks for.
    pub fn worker_seated(
        &self,
        team: impl Into<String>,
        pane: impl Into<String>,
        checkout: impl Into<String>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::WorkerSeated {
            team: team.into(),
            pane: pane.into(),
            checkout: checkout.into(),
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// The window says a coordinator's leader pane came back for a run.
    /// Answers whether the seat took it — `false` for a held seat, which is
    /// a twin and not the coordinator — and the revision that speaks for.
    pub fn coordinator_returned(
        &self,
        run: impl Into<String>,
        team: impl Into<String>,
        pane: impl Into<String>,
        actor: Option<String>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::CoordinatorReturned {
            run: run.into(),
            team: team.into(),
            pane: pane.into(),
            actor,
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Plan one sleeping worker's replacement pane under its coordinator's
    /// current team table.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_worker_reseat(
        &self,
        run: impl Into<String>,
        worker: impl Into<String>,
        team: impl Into<String>,
        coordinator_pane: impl Into<String>,
        leader_term: u32,
        resume_nudge: impl Into<String>,
    ) -> Result<Result<Box<Decided>, String>, RuntimeError> {
        match self.request(RuntimeRequest::PrepareWorkerReseat {
            run: run.into(),
            worker: worker.into(),
            team: team.into(),
            coordinator_pane: coordinator_pane.into(),
            leader_term,
            resume_nudge: resume_nudge.into(),
        })? {
            RuntimeReply::ReseatPlanned { decided, .. } => Ok(decided),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Make a successful replacement pane the worker's durable seat.
    pub fn worker_reseated(
        &self,
        worker: impl Into<String>,
        team: impl Into<String>,
        pane: impl Into<String>,
        now_ms: i64,
    ) -> Result<u64, RuntimeError> {
        match self.request(RuntimeRequest::WorkerReseated {
            worker: worker.into(),
            team: team.into(),
            pane: pane.into(),
            now_ms,
        })? {
            RuntimeReply::Settled { revision, .. } => Ok(revision),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Retire a sleeping attempt that cannot ever be restored.
    pub fn finish_sleeping_reseat(
        &self,
        worker: impl Into<String>,
        reason: impl Into<String>,
        now_ms: i64,
    ) -> Result<u64, RuntimeError> {
        match self.request(RuntimeRequest::FinishSleepingReseat {
            worker: worker.into(),
            reason: reason.into(),
            now_ms,
        })? {
            RuntimeReply::Settled { revision, .. } => Ok(revision),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A pane the plan asked for never opened; the worker row goes back.
    pub fn seat_never_opened(
        &self,
        worker: impl Into<String>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::SeatNeverOpened {
            worker: worker.into(),
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Reserve one host effect. What comes back is the journal's word on it.
    pub fn prepare_effect(
        &self,
        request: EffectRequest,
        now_ms: i64,
    ) -> Result<(BeginEffect, u64), RuntimeError> {
        match self.request(RuntimeRequest::PrepareEffect { request, now_ms })? {
            RuntimeReply::Effect { begun, revision } => Ok((begun, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Write how a reserved effect ended.
    pub fn settle_effect(
        &self,
        permit: EffectPermit,
        settlement: EffectSettlement,
    ) -> Result<u64, RuntimeError> {
        match self.request(RuntimeRequest::SettleEffect { permit, settlement })? {
            RuntimeReply::Settled { revision, .. } => Ok(revision),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// This window just came up; settle every worker the last one had — and
    /// keep the ones it can seat again.
    pub fn window_restarted(
        &self,
        now_ms: i64,
    ) -> Result<(zerocode_core::orchestration::Restarted, u64), RuntimeError> {
        match self.request(RuntimeRequest::WindowRestarted { now_ms })? {
            RuntimeReply::Swept { swept, revision } => Ok((swept, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A leader's death took its team; settle the team's side of the ledger.
    pub fn team_dissolved(
        &self,
        team: impl Into<String>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::TeamDissolved {
            team: team.into(),
            now_ms,
        })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A woken wait looks again; `Some` took the delivery and remembered it.
    pub fn look_again(
        &self,
        waiting: Waiting,
        receipt: Option<ReceiptKey>,
        now_ms: i64,
    ) -> Result<(Option<Box<Decided>>, u64), RuntimeError> {
        match self.request(RuntimeRequest::LookAgain {
            waiting: Box::new(waiting),
            receipt: receipt.map(Box::new),
            now_ms,
        })? {
            RuntimeReply::Looked { found, revision } => Ok((found, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// File the receipt of a decision whose effect has now happened.
    pub fn serve_receipt(
        &self,
        decided: Box<Decided>,
        now_ms: i64,
    ) -> Result<(bool, u64), RuntimeError> {
        match self.request(RuntimeRequest::ServeReceipt { decided, now_ms })? {
            RuntimeReply::Settled { moved, revision } => Ok((moved, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    pub fn worker_terminal_settled(
        &self,
        effect: Effect,
        screen: Option<String>,
        now_ms: i64,
    ) -> Result<(WorkerState, u64), RuntimeError> {
        if matches!(
            &effect,
            Effect::WorkerTerminal {
                handover: Some(_),
                ..
            }
        ) {
            return Err(RuntimeError::AuthorityRejected);
        }
        self.worker_terminal_settled_fenced(effect, screen, now_ms, |commit| commit(now_ms))
    }

    /// Ask for current host evidence only after this request reaches the actor.
    /// `observe` calls `commit` once, under the host's activity and quota locks,
    /// and keeps those locks until it returns. It must not re-enter the actor.
    pub fn worker_terminal_settled_fenced(
        &self,
        effect: Effect,
        screen: Option<String>,
        now_ms: i64,
        observe: impl FnOnce(&mut dyn FnMut(i64)),
    ) -> Result<(WorkerState, u64), RuntimeError> {
        let Effect::WorkerTerminal {
            seat,
            incarnation: Some(incarnation),
            stop,
            handover,
            ..
        } = effect
        else {
            return Err(RuntimeError::InvalidInput);
        };
        let pending = self.pending_effect(|fence| RuntimeRequest::WorkerTerminalSettled {
            seat,
            term: incarnation.term,
            capability: incarnation.capability,
            stop,
            screen,
            handover,
            fence: Some(fence),
            now_ms,
        })?;
        self.observe_terminal(pending, observe)
    }

    fn observe_terminal(
        &self,
        pending: PendingEffect,
        observe: impl FnOnce(&mut dyn FnMut(i64)),
    ) -> Result<(WorkerState, u64), RuntimeError> {
        let mut answer = None;
        if pending.entered.recv().is_ok() {
            observe(&mut |at_ms| {
                if answer.is_none() {
                    let _ = pending.proceed.send(at_ms);
                    answer = Some(
                        pending
                            .answer
                            .recv()
                            .unwrap_or_else(|_| Err(self.terminal.failure())),
                    );
                }
            });
        }
        // No observation means refusal. Drop the sender before waiting for
        // the reply so the actor cannot remain parked behind a recovered wall.
        drop(pending.proceed);
        match answer.unwrap_or_else(|| {
            pending
                .answer
                .recv()
                .unwrap_or_else(|_| Err(self.terminal.failure()))
        })? {
            RuntimeReply::Release { state, revision } => Ok((state, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    pub fn release_unobserved(
        &self,
        decided: Box<Decided>,
        now_ms: i64,
    ) -> Result<zerocode_core::agent_teams::Reply, RuntimeError> {
        match self.request(RuntimeRequest::ReleaseUnobserved { decided, now_ms })? {
            RuntimeReply::Planned { decided, .. } => Ok(decided.reply),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// A release's read is over; write how the worker ended.
    pub fn release_settled(
        &self,
        worker: impl Into<String>,
        screen: Option<String>,
        now_ms: i64,
    ) -> Result<(WorkerState, u64), RuntimeError> {
        match self.request(RuntimeRequest::ReleaseSettled {
            worker: worker.into(),
            screen,
            now_ms,
        })? {
            RuntimeReply::Release { state, revision } => Ok((state, revision)),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Remember what a named request was told, for an answer built after
    /// the effect.
    pub fn remember_served(
        &self,
        key: ReceiptKey,
        stdout: impl Into<String>,
        now_ms: i64,
    ) -> Result<u64, RuntimeError> {
        match self.request(RuntimeRequest::RememberServed {
            key: Box::new(key),
            stdout: stdout.into(),
            now_ms,
        })? {
            RuntimeReply::Settled { revision, .. } => Ok(revision),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    /// Settle the one operation recovered at boot; this lifts the fence.
    pub fn settle_recovered(&self, settlement: EffectSettlement) -> Result<u64, RuntimeError> {
        match self.request(RuntimeRequest::SettleRecovered { settlement })? {
            RuntimeReply::Settled { revision, .. } => Ok(revision),
            _ => Err(RuntimeError::AuthorityRejected),
        }
    }

    pub fn shutdown(mut self) -> Result<RuntimeImage, RuntimeError> {
        let Some(sender) = self.sender.take() else {
            return Err(RuntimeError::Closed);
        };
        let (reply, answer) = mpsc::sync_channel(0);
        let sent = sender.send(ActorCommand::Shutdown { reply });
        drop(sender);
        let reply = match sent {
            Ok(()) => answer
                .recv()
                .unwrap_or_else(|_| Err(self.terminal.failure())),
            Err(_) => Err(self.terminal.failure()),
        };
        let joined = self.join_actor();
        match (reply, joined) {
            (_, Err(error)) => Err(error),
            (Err(error), Ok(_)) => Err(error),
            (Ok(_), Ok(image)) => Ok(image),
        }
    }

    fn join_actor(&mut self) -> Result<RuntimeImage, RuntimeError> {
        let Some(join) = self.join.take() else {
            return Err(RuntimeError::Closed);
        };
        join.join().unwrap_or(Err(RuntimeError::Panicked))
    }
}

impl Drop for RuntimeActor {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Everything `verify_authority` compares about one open operation — the
/// A settlement the runtime refused, and the permit if it is still good.
///
/// `permit` is `Some` whenever the row did not move, which is every refusal
/// the journal itself makes: the last fallible step in those doors is the
/// commit, and a commit that fails rolls back, so the row revision the
/// permit names is still the row's own. It is `None` only past that line,
/// where the settlement SUCCEEDED and the bookkeeping after it did not — a
/// different accident, and one no permit can answer.
struct SettleRefused {
    permit: Option<EffectPermit>,
    why: RuntimeError,
}

impl SettleRefused {
    fn spent(why: RuntimeError) -> Box<Self> {
        Box::new(Self { permit: None, why })
    }
}

/// walked one, whose permit left with the caller.
struct WalkMark {
    operation_id: String,
    state: HostEffectState,
    kind: HostEffectKind,
    attempt: u64,
    operation_authority_epoch: u64,
    prepared_revision: u64,
}

impl WalkMark {
    fn of(permit: &EffectPermit) -> Self {
        Self {
            operation_id: permit.operation_id().to_string(),
            state: permit.state(),
            kind: permit.kind(),
            attempt: permit.attempt(),
            operation_authority_epoch: permit.operation_authority_epoch(),
            prepared_revision: permit.prepared_revision(),
        }
    }

    fn matches(&self, durable: &OpenOperation) -> bool {
        durable.operation_id() == self.operation_id
            && durable.state() == self.state
            && durable.kind() == self.kind
            && durable.attempt() == self.attempt
            && durable.operation_authority_epoch() == self.operation_authority_epoch
            && durable.prepared_revision() == self.prepared_revision
    }
}

impl Drop for RuntimeState {
    fn drop(&mut self) {
        /* The window is ending, and the recoveries it never got to settle end
         * with it. That is not a lost permit — the next boot reads the same
         * rows back and is handed them again — but it IS a permit going
         * unspent, and the guard cannot tell "the process is closing" from
         * "somebody forgot" unless this window says which it was. */
        for permit in self.recovery_permits.drain(..) {
            permit.abandon("the window ended before this recovery was settled");
        }
    }
}

struct RuntimeState {
    journal: EffectJournal,
    lease: AuthorityLease,
    store: WorkflowStore,
    ledger_id: String,
    /// The ledger itself, not a picture of it.
    ///
    /// The owner thread holds the typed rows, and BORROWS the pane table
    /// under its own lock — [`orchestration::plan`] moves both, and a COPY
    /// of the table living here was a second answer to the same question:
    /// the actor kept answering for panes the host had already replaced.
    /// One table, one lock, and this handle is how the actor stands under
    /// it.
    ledger: Ledger,
    panes: Box<dyn PaneTable>,
    launcher: Box<dyn Launcher + Send>,
    revision: u64,
    /// The last moment this actor stamped into the store, kept so the next
    /// stamp can never stand behind it.
    ///
    /// The store refuses a timestamp behind its head — rightly, a head that
    /// moves backwards is two clocks arguing — but callers read their clocks
    /// BEFORE the mailbox, so two verbs can arrive with their stamps out of
    /// order through no fault of either. The one writer is the place where
    /// order exists, so the one writer is where the clamp lives: a verb's
    /// rows keep the caller's own time, and only the store's stamp is lifted
    /// to "no earlier than the last write".
    stamp: i64,
    /// The one operation THIS actor handed a permit out for and which is
    /// being walked right now — prepared, not yet settled.
    ///
    /// Kept apart from `recovery_permits` on purpose: both look identical to
    /// the journal (`recoverable` answers Prepared and Unknown either way),
    /// and they mean opposite things. A recovered operation is a crash being
    /// reconciled, and every mutation waits for it. A WALKING operation is a
    /// pane being cut on the lane this actor just authorized — and a window
    /// where one spawn froze every other verb would be a window nobody can
    /// type into for as long as a fork takes. Mutations proceed; only a
    /// second EFFECT is refused, by the journal's own single-flight door.
    walking: Option<WalkMark>,
    /// Why this runtime is refusing everything, once it has had to.
    ///
    /// Set when a decision could not be made durable. From then on the
    /// in-memory ledger is AHEAD of the disk, so answering from it would be
    /// showing somebody an effect that never happened — reads included.
    poison: Option<RuntimeError>,
    recovery_permits: Vec<EffectPermit>,
    repairs: Vec<String>,
}

/// Rebuild one durable generation, repairing only named historical wounds.
///
/// A pre-retention check receipt has no `filed_ms`. If its delivery history
/// disappeared during a disk-full rewrite, its old answer cannot be replayed
/// and its retry key cannot be freed. Core turns exactly those rows into
/// tombstones. A stale derived byte count is accepted only beside at least
/// one such repair; every other mismatch still fails closed. The canonical
/// generation is written before the actor answers its first request. Task
/// repairs never excuse byte-count or digest damage, and neither repair may
/// waive another content invariant or an unsettled host effect.
fn rebuild_stored_ledger(
    journal: &EffectJournal,
    lease: &AuthorityLease,
    mut held: ledger_store::Held,
    now_ms: i64,
) -> Result<(Ledger, u64, Vec<String>), RuntimeError> {
    let receipts = tombstone_unverifiable_legacy_receipts(&mut held.projection);
    if !held.bytes_match && receipts == 0 {
        return Err(RuntimeError::StoreCorrupt);
    }
    let mut repairs = repair_unattempted_dispatched_tasks(&mut held.projection);
    // A store written before the take-back tombstoned its own receipts
    // (2026-09-20) is repaired by the same narrow rule, and says so.
    repairs.extend(tombstone_receipts_of_taken_back_batches(
        &mut held.projection,
    ));
    let ledger = Ledger::rebuild(held.projection).map_err(RuntimeError::LedgerInvariant)?;
    if receipts == 0 && repairs.is_empty() {
        return Ok((ledger, held.revision, repairs));
    }

    journal
        .advance_ledger(lease, held.revision, &ledger.export(), now_ms)
        .map_err(runtime_error)?;
    let revision = held
        .revision
        .checked_add(1)
        .ok_or(RuntimeError::RevisionMismatch)?;
    Ok((ledger, revision, repairs))
}

impl RuntimeState {
    fn boot(
        store: WorkflowStore,
        ledger_id: String,
        boot: RuntimeBoot,
        panes: Box<dyn PaneTable>,
        launcher: Box<dyn Launcher + Send>,
    ) -> Result<Self, RuntimeError> {
        let journal = EffectJournal::new(&store);
        let lease = journal
            .claim_authority(&ledger_id, &random_instance_digest(), boot_now_ms(&boot))
            .map_err(runtime_error)?;
        let connection = store
            .connection()
            .map_err(|_| RuntimeError::StoreUnavailable)?;
        let held = ledger_store::read_repairable(&connection, &ledger_id, PROJECTION_SCHEMA)
            .map_err(runtime_error)?;
        /* Seed the timestamp clamp from the DISK, not from zero. The store
         * refuses a head that moves backwards, so a window reopening after
         * the wall clock stepped back (an NTP correction) would otherwise
         * have every mutation refused until real time caught up with the
         * old head. Boot's own writes ride `boot_now_ms`, so that is the
         * floor when it is the later of the two. */
        let disk_stamp = held
            .as_ref()
            .map_or(0, |held| held.updated_at_ms)
            .max(boot_now_ms(&boot));
        let (ledger, revision, repairs) = match (held, boot) {
            /* A fresh start that adopted somebody's ledger is how work
             * disappears without anybody being told. */
            #[cfg(test)]
            (Some(_), RuntimeBoot::Fresh { .. }) => {
                return Err(RuntimeError::InitializationConflict);
            }
            #[cfg(test)]
            (None, RuntimeBoot::Fresh { now_ms }) => {
                let ledger = Ledger::new();
                ledger_store::write(&connection, &ledger_id, 0, 1, &ledger.export(), now_ms)
                    .map_err(runtime_error)?;
                (ledger, 1, Vec::new())
            }
            #[cfg(test)]
            (None, RuntimeBoot::Reopen) => return Err(RuntimeError::MissingLedger),
            /* The store wins over the file, and `legacy` is not looked at:
             * the file already came in once — that is what the stamped
             * digest means — and reading it AGAIN would let a stale copy
             * argue with every revision written since. */
            #[cfg(test)]
            (Some(held), RuntimeBoot::Reopen) => {
                /* The same validation a file gets anywhere else. Rows that
                 * passed the database's own rules can still add up to a ledger
                 * no verb could have produced. */
                rebuild_stored_ledger(&journal, &lease, held, disk_stamp)?
            }
            (Some(held), RuntimeBoot::Cutover { .. }) => {
                rebuild_stored_ledger(&journal, &lease, held, disk_stamp)?
            }
            (
                None,
                RuntimeBoot::Cutover {
                    legacy: Some(carried),
                    now_ms,
                },
            ) => {
                /* Validated BEFORE it is written, with the same eyes rows
                 * get: a file that adds up to a ledger no verb could have
                 * produced is refused here, not imported and refused on
                 * every later boot. */
                let (initial, legacy_digest) = *carried;
                let ledger = Ledger::rebuild(initial).map_err(|_| RuntimeError::InvalidInput)?;
                journal
                    .import_legacy(&lease, &ledger.export(), &legacy_digest, now_ms)
                    .map_err(runtime_error)?;
                (ledger, 0, Vec::new())
            }
            (
                None,
                RuntimeBoot::Cutover {
                    legacy: None,
                    now_ms,
                },
            ) => {
                /* No store and no file is a first boot with a longer name —
                 * and it walks through the journal's own door rather than
                 * writing rows by hand, so a bare boot and an import leave
                 * the same shape behind: a revision-zero image whose first
                 * verb is one. */
                let ledger = Ledger::new();
                journal
                    .initialize_fresh(&lease, &ledger.export(), now_ms)
                    .map_err(runtime_error)?;
                (ledger, 0, Vec::new())
            }
        };
        drop(connection);
        let recovery_permits = journal.recoverable(&lease).map_err(runtime_error)?;
        if recovery_permits.len() > 1 {
            return Err(RuntimeError::MultipleInFlightEffects);
        }
        Ok(Self {
            journal,
            lease,
            store,
            ledger_id,
            ledger,
            panes,
            launcher,
            revision,
            stamp: disk_stamp,
            walking: None,
            poison: None,
            recovery_permits,
            repairs,
        })
    }

    /// The store's stamp for a mutation the caller clocked at `now_ms` —
    /// never behind the last one this actor wrote.
    fn stamped(&mut self, now_ms: i64) -> i64 {
        self.stamp = self.stamp.max(now_ms);
        self.stamp
    }
    /// The whole ledger, as a reader may see it.
    ///
    /// Built only when somebody asks for it. An image costs a walk of every
    /// row, so handing one back with every decision would make the price of a
    /// verb the size of the ledger.
    fn image(&self) -> RuntimeImage {
        RuntimeImage {
            revision: self.revision,
            projection: Arc::new(self.ledger.export()),
            recoveries: self
                .recovery_permits
                .iter()
                .map(RuntimeRecovery::from_permit)
                .collect(),
            repairs: self.repairs.clone(),
        }
    }

    fn verify_authority(&self) -> Result<(), RuntimeError> {
        /* The head row, not the bytes.
         *
         * What this asks is unchanged — has anything but this actor moved the
         * ledger — but it asks it of the revision the tables carry rather than
         * of a serialized image. Reading every row back to compare it would
         * make a question that runs before EVERY request cost the size of the
         * ledger.
         */
        let connection = self
            .store
            .connection()
            .map_err(|_| RuntimeError::StoreUnavailable)?;
        let durable = ledger_store::head_revision(&connection, &self.ledger_id)
            .map_err(runtime_error)?
            .ok_or(RuntimeError::AuthorityChanged)?;
        if durable != self.revision {
            return Err(RuntimeError::AuthorityChanged);
        }
        /* DESCRIBED, not granted. This runs before every request and only
         * ever compares fields; asking for permits here minted one per open
         * row per request and dropped them all — a capability spent on a
         * question. */
        let recoverable = self
            .journal
            .open_operations(&self.lease)
            .map_err(runtime_error)?;
        if recoverable.len() > 1 {
            return Err(RuntimeError::MultipleInFlightEffects);
        }
        /* Every open operation the journal shows must be one this actor
         * already knows — either the recovery it is fencing for, or the walk
         * it handed a permit out for. Anything else, and anything MISSING,
         * is the authority moving outside its owner. */
        let mut walked_seen = false;
        let mut expected = self.recovery_permits.iter();
        for durable in &recoverable {
            if !walked_seen
                && self
                    .walking
                    .as_ref()
                    .is_some_and(|walk| walk.matches(durable))
            {
                walked_seen = true;
                continue;
            }
            let Some(owned) = expected.next() else {
                return Err(RuntimeError::AuthorityChanged);
            };
            if durable.operation_id() != owned.operation_id()
                || durable.state() != owned.state()
                || durable.kind() != owned.kind()
                || durable.attempt() != owned.attempt()
                || durable.operation_authority_epoch() != owned.operation_authority_epoch()
                || durable.prepared_revision() != owned.prepared_revision()
            {
                return Err(RuntimeError::AuthorityChanged);
            }
        }
        if expected.next().is_some() || (self.walking.is_some() && !walked_seen) {
            return Err(RuntimeError::AuthorityChanged);
        }
        Ok(())
    }

    fn request(&mut self, request: RuntimeRequest) -> Result<RuntimeReply, RuntimeError> {
        /* A runtime that could not make its last decision durable answers
         * nothing — reads included.
         *
         * Its ledger is AHEAD of the disk by exactly the change it failed to
         * write, so answering a reader from memory would show them an effect
         * that never happened. The way back is to take the disk's word for it,
         * tried once here on the next request so a passing failure does not
         * cost a restart.
         */
        if let Some(why) = self.poison.clone() {
            /* An authority violation found during recovery keeps its own name:
             * it is not "the write did not land", it is "this is not the
             * ledger this actor owns", and the caller must be able to tell
             * those apart. */
            self.take_the_disks_word(why).map_err(|found| match found {
                RuntimeError::AuthorityChanged
                | RuntimeError::StoreCorrupt
                | RuntimeError::LedgerInvariant(_) => found,
                _ => RuntimeError::NotDurable,
            })?;
        }
        self.verify_authority()?;
        match request {
            RuntimeRequest::View => Ok(RuntimeReply::View(self.image())),
            RuntimeRequest::Plan(command) => self.carry_out(command),
            RuntimeRequest::TerminalGone {
                term,
                screen,
                now_ms,
            } => self.seat_gone(term, screen, now_ms),
            RuntimeRequest::PaneTurnEnded {
                term,
                turn_started_ms,
                interrupted,
                now_ms,
            } => self.turn_ended(term, turn_started_ms, interrupted, now_ms),
            RuntimeRequest::QuietSweep { stalled, now_ms } => self.quiet_swept(&stalled, now_ms),
            RuntimeRequest::QuotaWalls { walled, now_ms } => self.quota_walled(&walled, now_ms),
            RuntimeRequest::QuotaLifts { lifted, now_ms } => self.quota_lifted(&lifted, now_ms),
            RuntimeRequest::ClassifierDeclines { declined, now_ms } => {
                self.classifier_declined(&declined, now_ms)
            }
            RuntimeRequest::ModelDeviations { deviated, now_ms } => {
                self.model_deviated(&deviated, now_ms)
            }
            RuntimeRequest::StallCauses { judged, now_ms } => {
                self.stall_causes_judged(&judged, now_ms)
            }
            RuntimeRequest::HandoverBegin { plan, now_ms } => self.handover_begun(&plan, now_ms),
            RuntimeRequest::HandoverStep {
                run,
                handover,
                step,
                now_ms,
            } => self.handover_stepped(&run, &handover, step, now_ms),
            RuntimeRequest::HandoverSettle {
                run,
                handover,
                replacement,
                now_ms,
            } => self.handover_settled(&run, &handover, replacement, now_ms),
            RuntimeRequest::ResumeBegin { plan, now_ms } => self.resume_begun(&plan, now_ms),
            RuntimeRequest::ResumeSettle {
                run,
                resume,
                outcome,
                now_ms,
            } => self.resume_settled(&run, &resume, &outcome, now_ms),
            RuntimeRequest::ReadinessSweep {
                spoken,
                delivery_failures,
                now_ms,
            } => self.readiness_swept(&spoken, &delivery_failures, now_ms),
            RuntimeRequest::PanesMissing { missing, now_ms } => {
                self.panes_missing(&missing, now_ms)
            }
            RuntimeRequest::PanesSeen { workers, now_ms } => self.panes_seen(&workers, now_ms),
            RuntimeRequest::CheckoutExamined {
                worker,
                examined,
                now_ms,
            } => self.checkout_examined(&worker, &examined, now_ms),
            RuntimeRequest::Observation {
                to,
                body,
                receipt,
                now_ms,
            } => self.observed(&to, &body, receipt.as_deref(), now_ms),
            RuntimeRequest::RetentionSweep { now_ms } => self.retention_swept(now_ms),
            RuntimeRequest::PaneTakenOver { term, now_ms } => self.pane_taken(term, now_ms),
            RuntimeRequest::WindowExiting { now_ms } => self.window_exiting(now_ms),
            RuntimeRequest::WorkerPaneResumed {
                term,
                checkout,
                agent,
                session_id,
                now_ms,
            } => self.worker_pane_resumed(term, &checkout, &agent, &session_id, now_ms),
            RuntimeRequest::SleeperExpired { worker, now_ms } => {
                self.sleeper_expired(&worker, now_ms)
            }
            RuntimeRequest::WorkerSessionReported {
                term,
                session,
                now_ms,
            } => self.worker_session_reported(term, session, now_ms),
            RuntimeRequest::Federation(call) => self.federation_called(call),
            RuntimeRequest::WorkerSeated {
                team,
                pane,
                checkout,
                now_ms,
            } => self.worker_seat_reported(&team, &pane, &checkout, now_ms),
            RuntimeRequest::CoordinatorReturned {
                run,
                team,
                pane,
                actor,
                now_ms,
            } => self.coordinator_returned(&run, &team, &pane, actor.as_deref(), now_ms),
            RuntimeRequest::PrepareWorkerReseat {
                run,
                worker,
                team,
                coordinator_pane,
                leader_term,
                resume_nudge,
            } => self.prepare_worker_reseat(
                &run,
                &worker,
                &team,
                &coordinator_pane,
                leader_term,
                &resume_nudge,
            ),
            RuntimeRequest::WorkerReseated {
                worker,
                team,
                pane,
                now_ms,
            } => self.worker_reseated(&worker, &team, &pane, now_ms),
            RuntimeRequest::FinishSleepingReseat {
                worker,
                reason,
                now_ms,
            } => self.finish_sleeping_reseat(&worker, &reason, now_ms),
            RuntimeRequest::SeatNeverOpened { worker, now_ms } => {
                self.seat_never_opened(&worker, now_ms)
            }
            RuntimeRequest::PrepareEffect { request, now_ms } => {
                self.prepare_effect(&request, now_ms)
            }
            RuntimeRequest::SettleEffect { permit, settlement } => {
                self.settle_effect(permit, settlement).map_err(|refused| {
                    let SettleRefused { permit, why } = *refused;
                    if let Some(permit) = permit {
                        /* The caller across the channel cannot be handed a
                         * permit back — a reply carries facts, not
                         * capabilities — so the road for it is the next
                         * boot's, and this window SAYS so rather than letting
                         * it fall. */
                        permit.abandon(
                            "a settlement the runtime refused for a caller across the channel",
                        );
                    }
                    why
                })
            }
            RuntimeRequest::WindowRestarted { now_ms } => self.window_restarted(now_ms),
            RuntimeRequest::TeamDissolved { team, now_ms } => self.team_dissolved(&team, now_ms),
            RuntimeRequest::LookAgain {
                waiting,
                receipt,
                now_ms,
            } => self.look_again(&waiting, receipt.as_deref(), now_ms),
            RuntimeRequest::ServeReceipt { decided, now_ms } => {
                self.serve_receipt(&decided, now_ms)
            }
            RuntimeRequest::HandoverRevoked {
                run,
                handover,
                now_ms,
            } => {
                if now_ms < 0 || run.len() > MAX_NAME || handover.len() > MAX_NAME {
                    return Err(RuntimeError::InvalidInput);
                }
                if !self.recovery_permits.is_empty() {
                    return Err(RuntimeError::RecoveryRequired);
                }
                self.ledger
                    .handover_revoked(&run, &handover, now_ms)
                    .map_err(|_| RuntimeError::AuthorityRejected)?;
                let revision = self.write_through(now_ms)?;
                Ok(RuntimeReply::Settled {
                    moved: true,
                    revision,
                })
            }
            RuntimeRequest::WorkerTerminalSettled {
                seat,
                term,
                capability,
                stop,
                screen,
                handover,
                fence,
                now_ms,
            } => {
                if now_ms < 0 || screen.as_ref().is_some_and(|value| value.len() > MAX_PROSE) {
                    return Err(RuntimeError::InvalidInput);
                }
                if !self.recovery_permits.is_empty() {
                    return Err(RuntimeError::RecoveryRequired);
                }
                if handover.as_ref().is_some_and(|plan| {
                    !self.ledger.run(&plan.run).is_some_and(|run| {
                        zerocode_core::orchestration::handover_order_current(run, plan, false)
                    })
                }) {
                    return Err(RuntimeError::AuthorityRejected);
                }
                if fence.is_none() && handover.is_some() {
                    return Err(RuntimeError::AuthorityRejected);
                }
                let mut fence = fence;
                let mut settled_at_ms = now_ms;
                let mut settled = None;
                let ledger = &mut self.ledger;
                self.panes
                    .with_pane_authority(&seat.team, &seat.pane, &capability, &mut |team| {
                        if team.is_none_or(|team| team.term_of(&seat.pane) != Some(term))
                            || !ledger.worker_terminal_matches(&seat)
                        {
                            return;
                        }
                        if let Some(fence) = fence.take() {
                            let Ok(at_ms) = fence.enter() else {
                                return;
                            };
                            if at_ms < 0 {
                                return;
                            }
                            settled_at_ms = at_ms;
                        }
                        settled = Some(match stop.as_deref() {
                            Some(reason) => ledger.end_attempt(
                                &seat.worker,
                                zerocode_core::orchestration::Ending::Stopped,
                                reason,
                                settled_at_ms,
                            ),
                            None => Ok(match screen.as_ref() {
                                Some(screen) => {
                                    ledger.finish_release(&seat.worker, Some(screen.clone()))
                                }
                                None => ledger.release_unknown(&seat.worker),
                            }),
                        });
                    });
                let state = settled
                    .ok_or(RuntimeError::AuthorityRejected)?
                    .map_err(|_| RuntimeError::AuthorityRejected)?;
                let revision = self.write_through(settled_at_ms)?;
                Ok(RuntimeReply::Release { state, revision })
            }
            RuntimeRequest::HandoverSplit {
                prepared,
                operation,
                fence,
            } => {
                if !self.recovery_permits.is_empty() {
                    return Err(RuntimeError::RecoveryRequired);
                }
                if self
                    .walking
                    .as_ref()
                    .is_none_or(|walk| walk.operation_id != operation)
                    || self.ledger.run(&prepared.run).is_none_or(|run| {
                        !zerocode_core::orchestration::handover_start_current(run, &prepared)
                    })
                {
                    return Err(RuntimeError::AuthorityRejected);
                }
                fence.enter()?;
                Ok(RuntimeReply::Settled {
                    moved: false,
                    revision: self.revision,
                })
            }
            RuntimeRequest::ReleaseUnobserved {
                mut decided,
                now_ms,
            } => {
                if now_ms < 0 {
                    return Err(RuntimeError::InvalidInput);
                }
                if !self.recovery_permits.is_empty() {
                    return Err(RuntimeError::RecoveryRequired);
                }
                let Effect::WorkerTerminal {
                    seat, stop: None, ..
                } = &decided.effect
                else {
                    return Err(RuntimeError::InvalidInput);
                };
                // The host incarnation may be gone; only the original ledger
                // seat may lose its pending release. Never settle a reseat.
                let worker = self
                    .ledger
                    .runs()
                    .iter()
                    .find_map(|run| run.worker(&seat.worker))
                    .filter(|worker| seat.matches(worker))
                    .ok_or(RuntimeError::AuthorityRejected)?;
                let state = worker.state;
                let state = if state == WorkerState::ReleasePending {
                    self.ledger.release_unknown(&seat.worker)
                } else {
                    state
                };
                decided.reply = zerocode_core::agent_teams::Reply::ok(format!(
                    "{}\n",
                    serde_json::json!({"workerId": seat.worker, "state": state.as_str(), "archived": false})
                ));
                self.ledger.file_receipt(&decided, now_ms);
                let revision = self.write_through(now_ms)?;
                Ok(RuntimeReply::Planned { decided, revision })
            }
            RuntimeRequest::ReleaseSettled {
                worker,
                screen,
                now_ms,
            } => self.release_settled(&worker, screen, now_ms),
            RuntimeRequest::RememberServed {
                key,
                stdout,
                now_ms,
            } => self.remember_served(&key, &stdout, now_ms),
            RuntimeRequest::SettleRecovered { settlement } => self.settle_recovered(settlement),
        }
    }

    /// A terminal the host closed: settle whatever attempt sat in it.
    ///
    /// The seat is resolved and settled UNDER the table lock, exactly the
    /// fence the shell keeps today — a respawn re-points a pane on purpose,
    /// and in any gap between "which seat is term N" and "settle that seat"
    /// the answer can come to mean a different incarnation. The disk write
    /// happens after the lock is given back: the settlement is already
    /// decided, and a refused write poisons the LEDGER only.
    fn seat_gone(
        &mut self,
        term: u32,
        screen: Option<String>,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let ledger = &mut self.ledger;
        let mut moved = false;
        self.panes.with_seat_of_term(term, &mut |seat| {
            if let Some((team, pane)) = seat {
                moved = ledger
                    .terminal_gone_with_archive(team, pane, screen.clone(), now_ms)
                    .is_some();
            }
        });
        if !moved {
            /* Nothing to settle is an answer, not a failure — and not a
             * revision. Writing the same rows again to say so would wake
             * every reader for a ledger that did not move. */
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// A turn ended in a seat: write the worker's silence down, once.
    fn turn_ended(
        &mut self,
        term: u32,
        turn_started_ms: i64,
        interrupted: bool,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 || turn_started_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let ledger = &mut self.ledger;
        let mut moved = false;
        self.panes.with_seat_of_term(term, &mut |seat| {
            if let Some((team, pane)) = seat {
                // Any sound retires the readiness window — an interrupted
                // turn is still an agent that was THERE for it.
                let spoke = ledger.worker_spoke((team, pane));
                moved = ledger
                    .worker_fell_silent((team, pane), turn_started_ms, interrupted, now_ms)
                    .is_some()
                    || spoke;
            }
        });
        if !moved {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// A person took a pane: the seat table turns the terminal into a seat,
    /// and the ledger marks the worker sitting there as the person's.
    fn pane_taken(&mut self, term: u32, now_ms: i64) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let ledger = &mut self.ledger;
        let mut moved = false;
        self.panes.with_seat_of_term(term, &mut |seat| {
            if let Some((team, pane)) = seat {
                moved = ledger.worker_taken_over((team, pane));
            }
        });
        if !moved {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// The window says goodbye: the same sweep as `window_restarted` makes
    /// at the next boot, made now for the workers it can keep (t-3058).
    /// Nothing else moves — the boot sweep stays the authority on the rest.
    fn window_exiting(&mut self, now_ms: i64) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let swept = self.ledger.window_exiting(now_ms);
        if !swept.moved {
            return Ok(RuntimeReply::Swept {
                swept,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Swept { swept, revision })
    }

    /// The witness road (t-3058): the terminal resolves to its seat under
    /// the pane table, as every host fact about a pane does, and the ledger
    /// decides whether a sleeper is that conversation. `None` is the
    /// ordinary answer for a conversation a person reopened.
    fn worker_pane_resumed(
        &mut self,
        term: u32,
        checkout: &str,
        agent: &str,
        session_id: &str,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || checkout.is_empty()
            || checkout.len() > MAX_PROSE
            || agent.is_empty()
            || agent.len() > MAX_NAME
            || session_id.is_empty()
            || session_id.len() > MAX_NAME
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let ledger = &mut self.ledger;
        let mut seated = None;
        self.panes.with_seat_of_term(term, &mut |seat| {
            if let Some((team, pane)) = seat {
                seated =
                    ledger.worker_pane_resumed((team, pane), checkout, agent, session_id, now_ms);
            }
        });
        if seated.is_none() {
            return Ok(RuntimeReply::Witnessed {
                worker: None,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Witnessed {
            worker: seated,
            revision,
        })
    }

    /// A sleeper past its grace ends through the ledger's own road and is
    /// announced there (t-3058); the refusal for anything but a sleeper is
    /// the ledger's.
    fn sleeper_expired(&mut self, worker: &str, now_ms: i64) -> Result<RuntimeReply, RuntimeError> {
        if worker.is_empty() || worker.len() > MAX_NAME || now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        self.ledger
            .sleeper_expired(worker, now_ms)
            .map_err(|_| RuntimeError::AuthorityRejected)?;
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// A provider session observed in a terminal. Seat resolution and the
    /// ledger mutation share the pane-table borrow, closing the respawn gap;
    /// the disk write follows after the host lock is released.
    fn worker_session_reported(
        &mut self,
        term: u32,
        session: ProviderSession,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let ledger = &mut self.ledger;
        let mut moved = false;
        self.panes.with_seat_of_term(term, &mut |seat| {
            if let Some((team, pane)) = seat {
                moved = ledger.worker_session_reported((team, pane), session.clone());
            }
        });
        if !moved {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// The seat report: the checkout the split host resolved lands on the
    /// worker row it summoned. `moved: false` is the common case and not an
    /// error — every teammate split walks this lane, and most panes are
    /// nobody's worker.
    fn worker_seat_reported(
        &mut self,
        team: &str,
        pane: &str,
        checkout: &str,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 || team.is_empty() || pane.is_empty() || checkout.is_empty() {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        if !self.ledger.worker_seated((team, pane), checkout) {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// A restored coordinator sits again, if the chair is empty. The same
    /// shape as the seat report: `moved: false` is an ordinary answer — the
    /// chair was held, or this pane already holds it.
    fn coordinator_returned(
        &mut self,
        run: &str,
        team: &str,
        pane: &str,
        actor: Option<&str>,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || run.is_empty()
            || run.len() > MAX_NAME
            || team.is_empty()
            || team.len() > MAX_NAME
            || pane.is_empty()
            || pane.len() > MAX_NAME
            || actor.is_some_and(|actor| actor.is_empty() || actor.len() > MAX_NAME)
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let seat = format!("{team}/{pane}");
        if !self.ledger.coordinator_returned(run, &seat, actor, now_ms) {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_worker_reseat(
        &mut self,
        run: &str,
        worker: &str,
        team: &str,
        coordinator_pane: &str,
        leader_term: u32,
        resume_nudge: &str,
    ) -> Result<RuntimeReply, RuntimeError> {
        if run.is_empty()
            || worker.is_empty()
            || team.is_empty()
            || coordinator_pane.is_empty()
            || resume_nudge.len() > MAX_PROSE
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let ledger = &self.ledger;
        let launcher = self.launcher.as_ref();
        let mut planned = None;
        let mut coordinator_stands = false;
        self.panes.with_team(team, &mut |found| {
            let Some(found) = found else { return };
            if found.leader_term != leader_term || found.leader_pane != coordinator_pane {
                return;
            }
            coordinator_stands = true;
            planned = Some(ledger.prepare_worker_reseat(
                run,
                worker,
                found,
                coordinator_pane,
                launcher,
                resume_nudge,
            ));
        });
        if !coordinator_stands {
            return Err(RuntimeError::UnauthorizedSeat);
        }
        Ok(RuntimeReply::ReseatPlanned {
            decided: planned
                .expect("the verified coordinator planned")
                .map(Box::new),
            revision: self.revision,
        })
    }

    fn worker_reseated(
        &mut self,
        worker: &str,
        team: &str,
        pane: &str,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if worker.is_empty() || team.is_empty() || pane.is_empty() || now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let ledger = &mut self.ledger;
        let mut reseated = None;
        self.panes.with_team(team, &mut |found| {
            if found.is_some_and(|held| held.term_of(pane).is_some()) {
                reseated = Some(ledger.worker_reseated(worker, (team, pane), now_ms));
            }
        });
        reseated
            .ok_or(RuntimeError::UnauthorizedSeat)?
            .map_err(|_| RuntimeError::AuthorityRejected)?;
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    fn finish_sleeping_reseat(
        &mut self,
        worker: &str,
        reason: &str,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if worker.is_empty() || reason.is_empty() || now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        self.ledger
            .finish_sleeping_reseat(worker, reason, now_ms)
            .map_err(|_| RuntimeError::AuthorityRejected)?;
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Walk one federation call through the ledger. Refusals become
    /// `LedgerRefused` so the transport can hand the sentence to the far
    /// side verbatim; mutations write through before answering, and a
    /// mutation that moved nothing (a skipped repeat) still answers with
    /// the standing cursor.
    fn federation_called(&mut self, call: FederationCall) -> Result<RuntimeReply, RuntimeError> {
        use FederationAnswer as Answer;
        use FederationCall as Call;
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        macro_rules! law {
            ($fallible:expr) => {
                match $fallible {
                    Ok(value) => value,
                    Err(why) => {
                        return Ok(RuntimeReply::Federation {
                            answer: Answer::Refused(why),
                            revision: self.revision,
                        });
                    }
                }
            };
        }
        let (answer, wrote_ms) = match call {
            Call::EnsureRun { home, now_ms } => {
                let id = self.ledger.ensure_federation_run(&home, now_ms);
                (Answer::Run(id), Some(now_ms))
            }
            Call::Attach {
                run,
                dispatch,
                home,
                worker,
                now_ms,
            } => {
                law!(
                    self.ledger
                        .attach_federated(&run, &dispatch, &home, &worker, now_ms)
                );
                (Answer::Attached, Some(now_ms))
            }
            Call::Pull {
                dispatch,
                home,
                after_seq,
                limit,
            } => {
                let items = law!(
                    self.ledger
                        .federation_pull(&dispatch, &home, after_seq, limit)
                );
                (Answer::Items(items), None)
            }
            Call::Ack {
                dispatch,
                home,
                through_seq,
                settlements,
                now_ms,
            } => {
                let cursor =
                    law!(
                        self.ledger
                            .federation_ack(&dispatch, &home, through_seq, &settlements)
                    );
                (Answer::Cursor(cursor), Some(now_ms))
            }
            Call::Import {
                dispatch,
                home,
                items,
                now_ms,
            } => {
                let cursor = law!(
                    self.ledger
                        .federation_import(&dispatch, &home, &items, now_ms)
                );
                (Answer::Cursor(cursor), Some(now_ms))
            }
            Call::Stop {
                dispatch,
                home,
                now_ms,
            } => {
                let (state, worker) = law!(self.ledger.federation_stop(&dispatch, &home));
                (Answer::Stopped { state, worker }, Some(now_ms))
            }
            Call::StopUnknown {
                dispatch,
                home,
                now_ms,
            } => {
                law!(self.ledger.federation_stop_unknown(&dispatch, &home));
                (Answer::Done, Some(now_ms))
            }
            Call::Absorb {
                run,
                dispatch,
                items,
                now_ms,
            } => {
                let cursor = law!(
                    self.ledger
                        .federation_absorb(&run, &dispatch, &items, now_ms)
                );
                (Answer::Cursor(cursor), Some(now_ms))
            }
            Call::Outbox { run, dispatch } => (
                Answer::Items(self.ledger.federation_outbox(&run, &dispatch)),
                None,
            ),
            Call::AbortRemote {
                run,
                dispatch,
                task_preimage,
                now_ms,
            } => {
                law!(
                    self.ledger
                        .abort_remote_start(&run, &dispatch, &task_preimage)
                );
                (Answer::Done, Some(now_ms))
            }
            Call::HomeSeats => {
                let seats = self
                    .ledger
                    .runs()
                    .iter()
                    .flat_map(|run| {
                        run.dispatches
                            .iter()
                            .filter(|held| held.is_open())
                            .filter_map(|held| {
                                held.remote.as_ref().map(|seat| {
                                    (
                                        run.id.clone(),
                                        held.id.clone(),
                                        seat.server.clone(),
                                        seat.absorbed_seq,
                                    )
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                (Answer::Seats(seats), None)
            }
            Call::Exported {
                run,
                dispatch,
                through_seq,
                now_ms,
            } => {
                self.ledger
                    .federation_exported(&run, &dispatch, through_seq);
                (Answer::Done, Some(now_ms))
            }
        };
        let revision = match wrote_ms {
            Some(now_ms) => self.write_through(now_ms)?,
            None => self.revision,
        };
        Ok(RuntimeReply::Federation { answer, revision })
    }

    /// The readiness sweep: silent summonses past their window are reported
    /// to their coordinators, once each, and the windows retire with the
    /// report. `spoken` carries panes the window heard since the last beat.
    fn readiness_swept(
        &mut self,
        spoken: &[u32],
        delivery_failures: &[(u32, i64)],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 || delivery_failures.iter().any(|(_, since_ms)| *since_ms < 0) {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        // Delivery failures are observed first and sounds second. A marker can
        // disappear between the filesystem sweep and this request because the
        // successful hook that removed it is already queued; when both facts
        // share one beat, the real payload is the later and stronger word.
        let ledger = &mut self.ledger;
        for (term, since_ms) in delivery_failures {
            self.panes.with_seat_of_term(*term, &mut |seat| {
                if let Some((team, pane)) = seat {
                    ledger.worker_hook_delivery_failed((team, pane), *since_ms);
                }
            });
        }
        // The window speaks in terminals; the seat table turns each into a
        // `(team, pane)` the ledger knows — the same turning `turn_ended`
        // does, under the same lock.
        for term in spoken {
            self.panes.with_seat_of_term(*term, &mut |seat| {
                if let Some((team, pane)) = seat {
                    ledger.worker_spoke((team, pane));
                }
            });
        }
        let told = self.ledger.workers_overdue(now_ms);
        // `spoken` may have retired windows without telling anybody — that
        // is a write too, and one worth making durable so a restart does not
        // resurrect a window its worker already answered.
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: told > 0,
            revision,
        })
    }

    /// Write only notices that are due in the durable quiet episode.
    fn quiet_swept(
        &mut self,
        stalled: &[(String, i64)],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || stalled.len() > MAX_LIST
            || stalled.iter().any(|(worker, since_ms)| {
                worker.is_empty() || worker.len() > MAX_NAME || *since_ms < 0
            })
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let told = self.ledger.workers_stalled(stalled, now_ms);
        if told == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Write one `quota_walled` notice per walled attempt, and nothing for
    /// a wall already written down.
    fn quota_walled(
        &mut self,
        walled: &[zerocode_core::orchestration::QuotaWallWitness],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || walled.len() > MAX_LIST
            || walled
                .iter()
                .any(|one| one.worker.is_empty() || one.worker.len() > MAX_NAME)
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let told = self.ledger.workers_quota_walled(walled, now_ms);
        if told == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    fn quota_lifted(
        &mut self,
        lifted: &[zerocode_core::orchestration::QuotaLift],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || lifted.len() > MAX_LIST
            || lifted.iter().any(|one| {
                [&one.worker, &one.wall, &one.marker.source]
                    .iter()
                    .any(|name| name.is_empty() || name.len() > MAX_NAME)
                    || one.marker.line.as_str().len() > MAX_PROSE
            })
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let told = self.ledger.workers_quota_lifted(lifted, now_ms);
        if told == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    fn classifier_declined(
        &mut self,
        declined: &[zerocode_core::orchestration::ClassifierDeclineWitness],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || declined.len() > MAX_LIST
            || declined.iter().any(|one| {
                [&one.worker, &one.record.source, &one.record.key]
                    .iter()
                    .any(|name| name.is_empty() || name.len() > MAX_NAME)
                    || one
                        .record
                        .category
                        .as_ref()
                        .is_some_and(|word| word.len() > MAX_NAME)
                    || [&one.screen, &one.record.line]
                        .iter()
                        .any(|line| line.as_str().len() > MAX_PROSE)
            })
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let told = self.ledger.workers_classifier_declined(declined, now_ms);
        if told == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    fn model_deviated(
        &mut self,
        deviated: &[zerocode_core::orchestration::ModelDeviation],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 || deviated.len() > MAX_LIST || deviated.iter().any(|one| !one.fits()) {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let told = self.ledger.workers_model_deviated(deviated, now_ms);
        if told == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    fn stall_causes_judged(
        &mut self,
        judged: &[zerocode_core::orchestration::StallJudged],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || judged.len() > MAX_LIST
            || judged.iter().any(|one| {
                one.worker.is_empty()
                    || one.worker.len() > MAX_NAME
                    || one.cause.is_empty()
                    || one.cause.len() > MAX_NAME
                    || one.stalled_since_ms < 0
                    || !one.confidence.is_finite()
            })
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let told = self.ledger.stall_causes_judged(judged, now_ms);
        if told == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Reserve a handover's receipt row, durably, before the walk begins.
    fn handover_begun(
        &mut self,
        plan: &zerocode_core::orchestration::HandoverPlan,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || plan.run.is_empty()
            || plan.run.len() > MAX_NAME
            || plan.worker.is_empty()
            || plan.worker.len() > MAX_NAME
            || plan.dispatch.is_empty()
            || plan.dispatch.len() > MAX_NAME
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let Ok(id) = self.ledger.handover_begin(plan, now_ms) else {
            return Ok(RuntimeReply::Reserved {
                id: None,
                revision: self.revision,
            });
        };
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Reserved {
            id: Some(id),
            revision,
        })
    }

    /// One step onto a walking receipt, durably.
    fn handover_stepped(
        &mut self,
        run: &str,
        handover: &str,
        step: zerocode_core::orchestration::HandoverStep,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || run.is_empty()
            || run.len() > MAX_NAME
            || handover.is_empty()
            || handover.len() > MAX_NAME
            || step.name.is_empty()
            || step.name.len() > MAX_NAME
            || step.detail.as_str().len() > MAX_PROSE
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        if self.ledger.handover_step(run, handover, step).is_err() {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Settle and deliver a receipt, durably.
    fn handover_settled(
        &mut self,
        run: &str,
        handover: &str,
        replacement: Option<(String, String)>,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || run.is_empty()
            || run.len() > MAX_NAME
            || handover.is_empty()
            || handover.len() > MAX_NAME
            || replacement.as_ref().is_some_and(|(worker, dispatch)| {
                worker.is_empty()
                    || worker.len() > MAX_NAME
                    || dispatch.is_empty()
                    || dispatch.len() > MAX_NAME
            })
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        if self
            .ledger
            .handover_settled(run, handover, replacement, now_ms)
            .is_err()
        {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Reserve a continuation's receipt row, durably, before the words are
    /// typed.
    fn resume_begun(
        &mut self,
        plan: &zerocode_core::orchestration::ResumePlan,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || [
                &plan.run,
                &plan.worker,
                &plan.agent,
                &plan.dispatch,
                &plan.task,
                &plan.worker_team,
                &plan.worker_pane,
                &plan.marker.source,
                &plan.marker.key,
            ]
            .iter()
            .any(|name| name.is_empty() || name.len() > MAX_NAME)
            || plan.marker.line.as_str().len() > MAX_PROSE
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let Ok(id) = self.ledger.resume_begin(plan, now_ms) else {
            return Ok(RuntimeReply::Reserved {
                id: None,
                revision: self.revision,
            });
        };
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Reserved {
            id: Some(id),
            revision,
        })
    }

    /// Settle and deliver a continuation's receipt, durably.
    fn resume_settled(
        &mut self,
        run: &str,
        resume: &str,
        outcome: &zerocode_core::orchestration::ResumeOutcome,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || run.is_empty()
            || run.len() > MAX_NAME
            || resume.is_empty()
            || resume.len() > MAX_NAME
            || outcome.detail.as_str().len() > MAX_PROSE
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        if self
            .ledger
            .resume_settled(run, resume, outcome, now_ms)
            .is_err()
        {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Write only first transitions for confirmed missing-pane episodes.
    fn checkout_examined(
        &mut self,
        worker: &str,
        examined: &zerocode_core::orchestration::Examined,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 || worker.is_empty() || worker.len() > MAX_NAME {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let changed = self.ledger.checkout_examined(worker, examined, now_ms);
        if changed == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    fn observed(
        &mut self,
        to: &str,
        body: &str,
        receipt: Option<&str>,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || to.is_empty()
            || to.len() > MAX_PROSE
            || body.is_empty()
            || body.len() > MAX_PROSE
            || receipt.is_some_and(|value| value.is_empty() || value.len() > MAX_NAME)
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let filed = self.ledger.post_observation_once(to, body, receipt, now_ms);
        if filed == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    fn panes_missing(
        &mut self,
        missing: &[(String, i64)],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || missing.len() > MAX_LIST
            || missing.iter().any(|(worker, since_ms)| {
                worker.is_empty() || worker.len() > MAX_NAME || *since_ms < 0
            })
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let told = self.ledger.panes_missing(missing, now_ms);
        if told == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Clear episodes only when a positive host probe names their workers.
    fn panes_seen(
        &mut self,
        workers: &[String],
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0
            || workers.len() > MAX_LIST
            || workers
                .iter()
                .any(|worker| worker.is_empty() || worker.len() > MAX_NAME)
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let cleared = self.ledger.panes_seen(workers);
        if cleared == 0 {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// A pane the plan asked for never opened: the worker row goes back.
    ///
    /// The rollback half of the effect round trip, carried as a host FACT —
    /// the host is the only thing that knows the pane never appeared, and a
    /// worker row pointing at a pane that never opened is a roster entry
    /// nobody can click.
    fn seat_never_opened(
        &mut self,
        worker: &str,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if worker.is_empty() || worker.len() > MAX_NAME || now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let seat = self.ledger.runs().iter().find_map(|run| {
            run.workers
                .iter()
                .find(|one| one.id == worker)
                .map(|one| (one.team.clone(), one.pane.clone()))
        });
        let Some((team, pane)) = seat else {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        };
        /* The door checks its own name: the actor holds the table, so "the
         * pane never opened" is a question it can ask rather than a promise
         * the caller makes. A recorded seat means the split LANDED — erasing
         * that worker would orphan a live agent — so a lying caller is
         * refused here, not obeyed. The same shape as `recover`: make the
         * false sentence impossible to spell.
         *
         * Strictly, the question this asks is "the seat is not open NOW",
         * not "it never opened": a seat that opened and then lost its
         * terminal answers None here too. The caller calls this door only
         * for the split that just failed — read that sentence before
         * calling it from anywhere else. */
        let mut seat_open = false;
        self.panes.with_team(&team, &mut |held| {
            if let Some(held) = held {
                seat_open = held.term_of(&pane).is_some();
            }
        });
        if seat_open {
            return Err(RuntimeError::SeatStillOpen);
        }
        self.ledger.forget_worker(worker);
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// Reserve one host effect against the current rows, in one transaction.
    ///
    /// No recovery gate: reserving is exactly how an in-flight operation is
    /// offered BACK for reconciliation, so this door is the recovery road.
    /// On `Execute` the journal wrote the rows at the next revision, and the
    /// actor's mirror moves with it; every other answer wrote nothing.
    fn prepare_effect(
        &mut self,
        request: &EffectRequest,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        let at_ms = self.stamped(now_ms);
        let begun = self
            .journal
            .prepare(
                &self.lease,
                self.revision,
                &self.ledger.export(),
                request,
                at_ms,
            )
            .map_err(runtime_error)?;
        match &begun {
            BeginEffect::Execute(permit) => {
                self.revision = self
                    .revision
                    .checked_add(1)
                    .ok_or(RuntimeError::RevisionMismatch)?;
                /* The op this actor just handed out is a WALK, not a
                 * recovery: mutations keep flowing while the lane cuts the
                 * pane, and only a second effect is refused — by the
                 * journal's own single-flight door. */
                self.walking = Some(WalkMark::of(permit));
            }
            BeginEffect::Reconcile(permit) => {
                /* The journal handed a pending operation BACK: the caller is
                 * taking its reconciliation over, so it stops being a fence
                 * and becomes the walk. */
                self.walking = Some(WalkMark::of(permit));
                let taken = permit.operation_id().to_string();
                self.recovery_permits
                    .retain(|held| held.operation_id() != taken);
            }
            BeginEffect::Replay(_) | BeginEffect::Refused(_) => {}
        }
        Ok(RuntimeReply::Effect {
            begun,
            revision: self.revision,
        })
    }

    /// The lane walked (or failed to walk) the effect; write how it ended.
    ///
    /// On refusal the permit comes back beside the error, the way the journal
    /// hands it back one layer down. Only one caller can use it —
    /// [`Self::settle_recovered`], which owns the permit it brought — and
    /// that is the point: before this, that caller recovered by asking the
    /// journal to MINT the row a second time.
    fn settle_effect(
        &mut self,
        permit: EffectPermit,
        settlement: EffectSettlement,
    ) -> Result<RuntimeReply, Box<SettleRefused>> {
        /* The settlement carries its own clock, read on the caller's thread —
         * so it gets the same lift every other stamp gets, rebuilt rather
         * than trusted to be in order. */
        let settlement = match settlement {
            EffectSettlement::Applied {
                result_digest,
                settled_at_ms,
            } => EffectSettlement::Applied {
                result_digest,
                settled_at_ms: self.stamped(settled_at_ms),
            },
            EffectSettlement::NotStarted {
                failure,
                tombstone_digest,
                retryable,
                retry_not_before_ms,
                settled_at_ms,
            } => {
                let settled_at_ms = self.stamped(settled_at_ms);
                /* The retry floor rides the same lift: the journal requires
                 * it to sit strictly past the settlement, and a floor the
                 * caller read a beat before the clamp would otherwise turn a
                 * valid settlement into a refused one — leaving the very
                 * operation it closes standing open. The caller's INTENT —
                 * "due this soon after settling" — is what survives. */
                let retry_not_before_ms =
                    retry_not_before_ms.map(|at| at.max(settled_at_ms.saturating_add(1)));
                EffectSettlement::NotStarted {
                    failure,
                    tombstone_digest,
                    retryable,
                    retry_not_before_ms,
                    settled_at_ms,
                }
            }
        };
        if let Err(refused) = self.journal.settle(
            &self.lease,
            permit,
            self.revision,
            &self.ledger.export(),
            settlement,
        ) {
            let (permit, why) = *refused;
            return Err(Box::new(SettleRefused {
                permit: Some(permit),
                why: runtime_error(why),
            }));
        }
        /* Past this line the permit is SPENT — the row moved, durably — so
         * these refusals have nothing to hand back. They are the settlement
         * succeeding and the bookkeeping after it failing, which is a
         * different accident with a different answer. */
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SettleRefused::spent(RuntimeError::RevisionMismatch))?;
        self.walking = None;
        self.recovery_permits = self
            .journal
            .recoverable(&self.lease)
            .map_err(|why| SettleRefused::spent(runtime_error(why)))?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision: self.revision,
        })
    }

    /// Settle the one operation recovered at boot, with the permit that
    /// never left this thread.
    ///
    /// The same transition as [`settle_effect`], reached differently: the
    /// caller brings only the outcome, because a recovered permit is the
    /// journal's and stays here. Refused when there is nothing to settle —
    /// a recovery settlement with no recovery is a caller that has lost
    /// track of what it is reconciling.
    ///
    /// [`settle_effect`]: RuntimeState::settle_effect
    fn settle_recovered(
        &mut self,
        settlement: EffectSettlement,
    ) -> Result<RuntimeReply, RuntimeError> {
        let Some(permit) = self.recovery_permits.pop() else {
            return Err(RuntimeError::InvalidInput);
        };
        match self.settle_effect(permit, settlement) {
            Ok(reply) => Ok(reply),
            Err(refused) => {
                /* The permit goes back where it was — literally the same one.
                 * A settlement the journal refused has settled nothing, and
                 * dropping the permit here would leave a fence nothing can
                 * ever lift. This used to recover by asking the journal to
                 * MINT the row again, which worked and was also the clearest
                 * proof that a "one-use capability" could be issued twice for
                 * one row. */
                let SettleRefused { permit, why } = *refused;
                if let Some(permit) = permit {
                    self.recovery_permits.push(permit);
                }
                Err(why)
            }
        }
    }

    /// Every terminal the last window had is gone; settle all of them, once.
    ///
    /// The one door that does NOT resolve seats through the table: the table
    /// is empty at the moment this runs — that emptiness is the fact.
    fn window_restarted(&mut self, now_ms: i64) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let swept = self.ledger.window_restarted(now_ms);
        /* Asked of the sweep, not recomputed from its counts. A restart that
         * ended no attempt and put nothing to sleep still converges pending
         * releases and stands standing orders down, and skipping the write
         * for those left them in memory only — the change was real and the
         * next boot made it again, with no symptom either time. */
        if !swept.moved {
            return Ok(RuntimeReply::Swept {
                swept,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Swept { swept, revision })
    }

    /// Give up what a finished run no longer has to cost.
    ///
    /// The rate limit lives HERE rather than in the caller, so a second
    /// window's beat cannot talk this one into walking every run on every
    /// tick — the stamp it reads is the ledger's own, and the ledger is what
    /// both of them share.
    fn retention_swept(&mut self, now_ms: i64) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        if !self.ledger.sweep_due(now_ms) {
            return Ok(RuntimeReply::Retention {
                swept: Sweep::default(),
                revision: self.revision,
            });
        }
        let swept = self.ledger.sweep_retention(now_ms);
        /* A sweep that found nothing still moved the stamp, and the stamp is
         * a rate limit rather than a fact anybody reads — so it rides to disk
         * on the next real mutation instead of spending a write of its own.
         * Nothing else moved, so nothing else has to be told. */
        if !swept.moved() {
            return Ok(RuntimeReply::Retention {
                swept,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Retention { swept, revision })
    }

    /// A team ended with its leader: settle its workers, put its orders down.
    fn team_dissolved(&mut self, team: &str, now_ms: i64) -> Result<RuntimeReply, RuntimeError> {
        if team.is_empty() || team.len() > MAX_NAME || now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        /* Asked of the dissolution, not recomputed from its counts — the
         * same rule `window_restarted` keeps below: a leader whose team held
         * no worker still vacated the coordinator seat, and a vacated seat
         * left in memory only is a chair the next `run-use` finds held by a
         * pane that died. */
        if !self.ledger.team_dissolved(team, now_ms).moved {
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// A woken wait looks once more — and if something arrived, the delivery
    /// and its receipt are one transition, so a second verb cannot land
    /// between the answer and the record that it was given.
    fn look_again(
        &mut self,
        waiting: &Waiting,
        receipt: Option<&ReceiptKey>,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let Some(found) = zerocode_core::orchestration::look_again(&mut self.ledger, waiting)
        else {
            /* Nothing arrived. Nothing was taken, nothing is written, and the
             * revision this answer speaks for is the one the sleeper already
             * knew — still waiting is not news. */
            return Ok(RuntimeReply::Looked {
                found: None,
                revision: self.revision,
            });
        };
        if let Some(key) = receipt {
            self.ledger.remember_answer(key, &found, now_ms);
        }
        /* A delivery lease was taken whether or not anybody named a retry:
         * the rows moved, so the disk has to agree before the answer goes
         * out — the same rule as a verb. */
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Looked {
            found: Some(Box::new(found)),
            revision,
        })
    }

    /// File the receipt of a decision whose effect has now happened.
    fn serve_receipt(
        &mut self,
        decided: &Decided,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        if decided.receipt.is_none() {
            /* Nothing to file is an answer: `file_receipt` on a receiptless
             * decision writes nothing, and writing the rows again to say so
             * would wake every reader for a ledger that did not move. */
            return Ok(RuntimeReply::Settled {
                moved: false,
                revision: self.revision,
            });
        }
        self.ledger.file_receipt(decided, now_ms);
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// A release's read is over; write how the worker ended.
    fn release_settled(
        &mut self,
        worker: &str,
        screen: Option<String>,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if worker.is_empty()
            || worker.len() > MAX_NAME
            || screen.as_ref().is_some_and(|held| held.len() > MAX_PROSE)
            || now_ms < 0
        {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        let state = match screen {
            Some(screen) => self.ledger.finish_release(worker, Some(screen)),
            None => self.ledger.release_unknown(worker),
        };
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Release { state, revision })
    }

    /// Remember what a named request was told, for an answer built after
    /// the effect.
    fn remember_served(
        &mut self,
        key: &ReceiptKey,
        stdout: &str,
        now_ms: i64,
    ) -> Result<RuntimeReply, RuntimeError> {
        if stdout.is_empty() || stdout.len() > MAX_PROSE || now_ms < 0 {
            return Err(RuntimeError::InvalidInput);
        }
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        self.ledger.remember_served(key, stdout, now_ms);
        let revision = self.write_through(now_ms)?;
        Ok(RuntimeReply::Settled {
            moved: true,
            revision,
        })
    }

    /// One verb, and then the writing of it.
    fn carry_out(&mut self, command: PlanCommand) -> Result<RuntimeReply, RuntimeError> {
        if !self.recovery_permits.is_empty() {
            return Err(RuntimeError::RecoveryRequired);
        }
        /* Planned UNDER the table lock — and AUTHENTICATED under it, in the
         * same generation. The presented capability is verified by the table
         * implementation inside this very borrow, so a respawn cannot rotate
         * the token between the check and the plan: the check IS part of the
         * plan's lock hold. The DISK stays outside the lock below: the
         * ledger's atomicity comes from this thread being the only writer,
         * and a slow or refusing disk must not stand between every other
         * pane and its table. */
        if let Some(plan) = command.handover.as_ref() {
            let stopped = command
                .argv()
                .first()
                .is_some_and(|verb| verb == "worker-start");
            if command.team() != plan.team
                || command.pane() != plan.pane
                || !self.ledger.run(&plan.run).is_some_and(|run| {
                    zerocode_core::orchestration::handover_order_current(run, plan, stopped)
                })
            {
                return Err(RuntimeError::AuthorityRejected);
            }
            if !stopped
                && self.panes.incarnation(&plan.worker_team, &plan.worker_pane)
                    != command.worker_incarnation
            {
                return Err(RuntimeError::AuthorityRejected);
            }
        }
        let ledger = &mut self.ledger;
        let launcher = self.launcher.as_ref();
        let mut planned: Option<Decided> = None;
        self.panes.with_pane_authority(
            command.team(),
            command.pane(),
            command.presented(),
            &mut |team| {
                if let Some(team) = team {
                    planned = Some(zerocode_core::orchestration::plan(
                        ledger,
                        team,
                        launcher,
                        command.argv(),
                        command.pane(),
                        command.now_ms(),
                        command.actor(),
                    ));
                }
            },
        );
        let Some(mut decided) = planned else {
            return Err(RuntimeError::UnauthorizedSeat);
        };
        if let Effect::WorkerTerminal {
            seat,
            incarnation,
            handover,
            ..
        } = &mut decided.effect
        {
            *incarnation = command
                .worker_incarnation
                .clone()
                .or_else(|| self.panes.incarnation(&seat.team, &seat.pane));
            *handover = command.handover.clone();
        }
        if let Some(prepared) = decided.prepared_worker_start.as_mut() {
            prepared.handover = command.handover.clone();
        }
        /* A decision whose whole effect is nothing has already "happened", so
         * its receipt is filed HERE, in the same transaction as the plan —
         * the shell used to file it afterwards, and a crash between the two
         * writes left an answered caller with no receipt to replay. Effect
         * verbs cannot do this: their receipt becomes true only when the
         * effect does, and the shell brings the decision back through
         * [`RuntimeRequest::ServeReceipt`] then. */
        let receipt_filed = matches!(decided.effect, Effect::None)
            && decided.waiting.is_none()
            // A remote reservation is an effect the WIRE has not carried out
            // yet — its receipt becomes true the way a split's does, when
            // the shell brings it back through ServeReceipt.
            && decided.prepared_remote_start.is_none()
            && decided.receipt.is_some();
        if receipt_filed {
            ledger.file_receipt(&decided, command.now_ms());
        }
        /* A verb that wrote nothing has nothing to make durable, and bumping
         * the revision for it would tell every other reader the ledger moved
         * when it did not. `requires_durability` is the road's own answer to
         * "did this change anything", set where the change is made — and a
         * receipt just filed is a change wherever the verb itself stood. */
        if !decided.requires_durability && !receipt_filed {
            return Ok(RuntimeReply::Planned {
                decided: Box::new(decided),
                revision: self.revision,
            });
        }
        /* A refused write drops the decision with it — the caller is told
         * NotDurable and never sees a receipt for an answer it was not given;
         * the poisoned runtime rewinds its memory from the disk's word. */
        let revision = self.write_through(command.now_ms())?;
        Ok(RuntimeReply::Planned {
            decided: Box::new(decided),
            revision,
        })
    }

    /// Make what the ledger now says durable, or poison this runtime.
    ///
    /// One door for every mutation — a verb and a host fact are different
    /// callers, but "the disk agrees with the memory or nothing is answered"
    /// is one rule, and it lives in one place.
    fn write_through(&mut self, now_ms: i64) -> Result<u64, RuntimeError> {
        let next = self
            .revision
            .checked_add(1)
            .ok_or(RuntimeError::RevisionMismatch)?;
        let at_ms = self.stamped(now_ms);
        /* One transaction, or nothing. `ledger_store::write` moves the head,
         * clears every row and re-inserts the whole ledger, and it does so on
         * whatever connection it is handed — the promise that a ledger is
         * never half of two is the CALLER's transaction. Written without one,
         * each statement committed on its own, and a write that died in the
         * middle (measured 2026-08-27: the head at revision 132408 counting
         * 448,542 bytes over rows that stopped twelve workers in) left a store
         * every later read refused as corrupt — first as `NotDurable` until
         * the window was restarted, then as "the authority store is corrupt"
         * once it was. */
        let written = self
            .store
            .connection()
            .map_err(|_| RuntimeError::StoreUnavailable)
            .and_then(|mut connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|_| RuntimeError::StoreUnavailable)?;
                ledger_store::write(
                    &transaction,
                    &self.ledger_id,
                    self.revision,
                    next,
                    &self.ledger.export(),
                    at_ms,
                )
                .map_err(runtime_error)?;
                transaction
                    .commit()
                    .map_err(|_| RuntimeError::StoreUnavailable)
            });
        match written {
            Ok(()) => {
                self.revision = next;
                Ok(next)
            }
            Err(why) => {
                /* The decision stays here. A write the disk refused is not a
                 * promise the window may keep, and the effect this decision
                 * describes would be exactly that promise. The caller is told
                 * that it did not land — not which part of the store said so,
                 * which is this thread's problem to solve on the next request. */
                self.poison = Some(why);
                Err(RuntimeError::NotDurable)
            }
        }
    }

    /// Become answerable again by reading what the store actually holds.
    ///
    /// The memory that could not be written is dropped. That loses the verb
    /// that failed, which is the point: it never happened as far as anything
    /// outside this thread was told.
    fn take_the_disks_word(&mut self, why: RuntimeError) -> Result<(), RuntimeError> {
        let connection = self.store.connection().map_err(|_| why.clone())?;
        let held = ledger_store::read(&connection, &self.ledger_id, PROJECTION_SCHEMA)
            .map_err(|_| why.clone())?
            .ok_or(why)?;
        self.stamp = self.stamp.max(held.updated_at_ms);
        /* Confirm, do not adopt.
         *
         * This reads the disk to find out whether the change that failed is
         * really absent — not to accept whatever is there now. A head that has
         * moved PAST the last thing this actor made durable was written by
         * something else, and a poisoned runtime quietly taking that over
         * would be a fail-open where a healthy one fails closed. Raised by a
         * mutation that removed the line this replaces: nothing failed,
         * because nothing was asking the question.
         *
         * Removing THIS check cannot be caught either, and that is honest
         * rather than accidental: with the revision no longer copied from the
         * disk, a head that moved leaves the two numbers apart and
         * `verify_authority` refuses a moment later. The check stays because
         * it names the rule where the rule applies — the next reader should
         * not have to derive "recovery does not adopt" from the absence of an
         * assignment two functions away.
         */
        if held.revision != self.revision {
            return Err(RuntimeError::AuthorityChanged);
        }
        let ledger = Ledger::rebuild(held.projection).map_err(RuntimeError::LedgerInvariant)?;
        self.ledger = ledger;
        /* Clearing this is a cost, not a correctness: a runtime that forgot to
         * would simply read the store again on every request and answer the
         * same way. Left as one line rather than defended by a test, because a
         * test for it would be measuring how often a thing is read rather than
         * what anybody is told. */
        self.poison = None;
        Ok(())
    }
}

fn actor_main(
    store: WorkflowStore,
    ledger_id: String,
    boot: RuntimeBoot,
    panes: Box<dyn PaneTable>,
    launcher: Box<dyn Launcher + Send>,
    receiver: Receiver<ActorCommand>,
    boot_reply: SyncSender<Result<RuntimeImage, RuntimeError>>,
) -> Result<RuntimeImage, RuntimeError> {
    let mut state = match RuntimeState::boot(store, ledger_id, boot, panes, launcher) {
        Ok(state) => state,
        Err(error) => {
            let _ = boot_reply.send(Err(error.clone()));
            return Err(error);
        }
    };
    if let Err(error) = state.verify_authority() {
        let _ = boot_reply.send(Err(error.clone()));
        return Err(error);
    }
    let image = state.image();
    if boot_reply.send(Ok(image)).is_err() {
        return Err(RuntimeError::Closed);
    }
    loop {
        let command = match receiver.recv() {
            Ok(command) => command,
            Err(_) => return Ok(state.image()),
        };
        match command {
            ActorCommand::Request { request, reply } => match state.request(*request) {
                Ok(answer) => {
                    let _ = reply.send(Ok(answer));
                }
                Err(error) if error.fatal() => {
                    let _ = reply.send(Err(error.clone()));
                    return Err(error);
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            },
            ActorCommand::Shutdown { reply } => {
                if let Err(error) = state.verify_authority() {
                    let _ = reply.send(Err(error.clone()));
                    return Err(error);
                }
                let image = state.image();
                if let Err(error) = state
                    .journal
                    .relinquish_authority(&mut state.lease, wall_clock_ms())
                    .map_err(runtime_error)
                    .map_err(|error| {
                        /* The release refuses over ANY open operation. Which
                         * story is true depends on whose operation it is: a
                         * walk this actor authorized, or a dead window's
                         * recovery. */
                        if matches!(error, RuntimeError::EffectInFlight) && state.walking.is_none()
                        {
                            RuntimeError::RecoveryRequired
                        } else {
                            error
                        }
                    })
                {
                    let _ = reply.send(Err(error.clone()));
                    return Err(error);
                }
                let _ = reply.send(Ok(image.clone()));
                return Ok(image);
            }
            #[cfg(test)]
            ActorCommand::Hold {
                entered,
                release,
                finished,
            } => {
                let _ = entered.send(());
                let _ = release.recv();
                let _ = finished.send(());
            }
            #[cfg(test)]
            ActorCommand::Panic => panic!("injected runtime actor panic"),
            #[cfg(test)]
            ActorCommand::Close => return Ok(state.image()),
        }
    }
}

fn runtime_error(error: EffectJournalError) -> RuntimeError {
    match error {
        EffectJournalError::Database => RuntimeError::StoreUnavailable,
        EffectJournalError::Corrupt => RuntimeError::StoreCorrupt,
        EffectJournalError::InvalidInput { .. } => RuntimeError::InvalidInput,
        EffectJournalError::RevisionMismatch => RuntimeError::RevisionMismatch,
        EffectJournalError::ImportConflict => RuntimeError::ImportConflict,
        EffectJournalError::InitializationConflict => RuntimeError::InitializationConflict,
        EffectJournalError::EffectInFlight => RuntimeError::EffectInFlight,
        EffectJournalError::TimestampRegression => RuntimeError::TimestampRegression,
        EffectJournalError::RequestConflict
        | EffectJournalError::InvalidTransition
        | EffectJournalError::ConcurrentTransition => RuntimeError::AuthorityRejected,
        EffectJournalError::OwnerBusy => RuntimeError::AuthorityChanged,
        EffectJournalError::OwnerLockUnsupported => RuntimeError::StoreUnavailable,
        EffectJournalError::StaleAuthority => RuntimeError::AuthorityChanged,
        EffectJournalError::ReleaseInFlight => RuntimeError::EffectInFlight,
    }
}

fn boot_now_ms(boot: &RuntimeBoot) -> i64 {
    match boot {
        #[cfg(test)]
        RuntimeBoot::Fresh { now_ms } => *now_ms,
        #[cfg(test)]
        RuntimeBoot::Reopen => wall_clock_ms(),
        RuntimeBoot::Cutover { now_ms, .. } => *now_ms,
    }
}

fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn random_instance_digest() -> String {
    let mut random = [0_u8; 32];
    rand::fill(&mut random);
    let mut digest = Sha256::new();
    digest.update(b"zerocode.runtime-owner-instance.v1");
    digest.update(random);
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::orchestration::{Auto, NoLauncher};

    /// A store that could not be opened is answered, not the end of the actor.
    ///
    /// The write road already says how a passing failure should be read: a
    /// refused write poisons the runtime and the next request takes the
    /// disk's word for it, "so a passing failure does not cost a restart". A
    /// connection that never opened deserves the same and needs less — it
    /// read nothing and wrote nothing, so this actor's state is untouched.
    ///
    /// It was fatal, and that promoted a passing condition into a permanent
    /// one: on 2026-09-18 a full disk refused one sqlite open, the actor
    /// returned, and the ledger stayed shut long after the disk was freed —
    /// window still running, database intact, and nothing able to reopen it
    /// but a restart only a person can order.
    #[test]
    fn a_store_that_could_not_be_opened_is_answered_and_the_actor_lives() {
        assert!(
            !RuntimeError::StoreUnavailable.fatal(),
            "a store this actor could not open right now is not a store it has lost"
        );

        /* The ones that stay fatal are the ones where carrying on would be a
         * lie about what this actor is holding. */
        for ending in [
            RuntimeError::StoreCorrupt,
            RuntimeError::AuthorityChanged,
            RuntimeError::MultipleInFlightEffects,
            RuntimeError::Panicked,
        ] {
            assert!(ending.fatal(), "{ending} must still end the actor");
        }

        /* And the ordinary refusals were never fatal; naming them here keeps
         * the set from growing by accident. */
        for answered in [
            RuntimeError::NotDurable,
            RuntimeError::InvalidInput,
            RuntimeError::UnknownTeam,
            RuntimeError::RevisionMismatch,
        ] {
            assert!(!answered.fatal(), "{answered} must be answered, not fatal");
        }
    }

    /// The door takes a command at its bound and refuses the one past it.
    ///
    /// Each of these is a separate reason, and the door has to have all of
    /// them: the arithmetic that says a full mailbox costs about a megabyte
    /// and a third is only true while every one of them holds.
    #[test]
    fn the_door_takes_a_command_at_its_bound_and_refuses_the_one_past_it() {
        let a_word = || "w".repeat(MAX_NAME);
        let at_the_bound = vec!["send".to_string(); MAX_COMMAND_WORDS];
        assert!(PlanCommand::checked(at_the_bound.clone(), "team-1", "%1", "c", None, 1).is_ok());

        let mut one_too_many = at_the_bound;
        one_too_many.push("send".to_string());
        assert_eq!(
            PlanCommand::checked(one_too_many, "team-1", "%1", "c", None, 1).unwrap_err(),
            RuntimeError::InvalidInput,
            "a command past the word count was taken"
        );

        let too_much = vec!["b".repeat(MAX_COMMAND_BYTES + 1)];
        assert_eq!(
            PlanCommand::checked(too_much, "team-1", "%1", "c", None, 1).unwrap_err(),
            RuntimeError::InvalidInput,
            "a command past the byte count was taken"
        );

        assert_eq!(
            PlanCommand::checked(Vec::new(), "team-1", "%1", "c", None, 1).unwrap_err(),
            RuntimeError::InvalidInput,
            "a command with no verb at all was taken"
        );
        assert_eq!(
            PlanCommand::checked(
                vec!["send".to_string()],
                "team-1",
                a_word() + "x",
                "c",
                None,
                1
            )
            .unwrap_err(),
            RuntimeError::InvalidInput,
            "a pane longer than a name was taken"
        );
        assert_eq!(
            PlanCommand::checked(vec!["send".to_string()], "team-1", "%1", "", None, 1)
                .unwrap_err(),
            RuntimeError::InvalidInput,
            "a command with no capability at all was taken"
        );
        assert_eq!(
            PlanCommand::checked(
                vec!["send".to_string()],
                "team-1",
                "%1",
                "c".repeat(MAX_TOKEN_BYTES + 1),
                None,
                1
            )
            .unwrap_err(),
            RuntimeError::InvalidInput,
            "a capability past the token bound was taken"
        );
        assert_eq!(
            PlanCommand::checked(
                vec!["send".to_string()],
                "team-1",
                "%1",
                "c",
                Some(a_word() + "x"),
                1
            )
            .unwrap_err(),
            RuntimeError::InvalidInput,
            "a caller longer than a name was taken"
        );
        assert_eq!(
            PlanCommand::checked(vec!["send".to_string()], "team-1", "%1", "c", None, -1)
                .unwrap_err(),
            RuntimeError::InvalidInput,
            "a command from before the epoch was taken"
        );
    }

    /// What a full mailbox costs is a number, and the number is small.
    ///
    /// This is the whole point of taking verbs instead of ledger images: four
    /// waiting commands used to be four whole ledgers.
    #[test]
    fn a_full_mailbox_of_commands_is_bounded_by_a_number_a_person_can_hold() {
        let waiting = MAX_RUNTIME_MAILBOX * MAX_COMMAND_BYTES;
        assert!(
            waiting < 2 * 1024 * 1024,
            "a full mailbox is {waiting} bytes, which is not a bound anybody \
             can reason about"
        );
        /* And a command really cannot exceed its share of that. The pane and
         * the presented capability are counted too — they are bytes the
         * mailbox holds like any other. */
        let biggest = PlanCommand::checked(
            vec!["send".to_string(), "b".repeat(MAX_COMMAND_BYTES - 13)],
            "team-1",
            "%1",
            "c",
            None,
            1,
        )
        .expect("a command at the bound");
        assert_eq!(biggest.bytes(), MAX_COMMAND_BYTES);
        assert_eq!(
            PlanCommand::checked(
                vec!["send".to_string(), "b".repeat(MAX_COMMAND_BYTES - 12)],
                "team-1",
                "%1",
                "c",
                None,
                1,
            )
            .unwrap_err(),
            RuntimeError::InvalidInput,
            "one byte past the bound was taken"
        );
    }

    /// What somebody typed does not appear in a log — and neither does the
    /// capability the pane proved itself with.
    #[test]
    fn a_command_printed_says_its_shape_and_not_its_words() {
        let held = PlanCommand::checked(
            vec![
                "send".to_string(),
                "--body".to_string(),
                "the-prose-nobody-should-see".to_string(),
            ],
            "team-1",
            "%1",
            "token-nobody-should-see",
            Some("caller-nobody-should-see".to_string()),
            1,
        )
        .expect("a command");
        let printed = format!("{held:?}");
        assert!(!printed.contains("nobody-should-see"), "{printed}");
        assert!(printed.contains("send"), "the verb is the shape: {printed}");
    }
    use std::sync::mpsc;

    use tempfile::TempDir;

    use crate::effect_journal::{
        BeginEffect, EffectRequest, HostEffectFailure, HostEffectKind, ImportResult,
    };

    struct Fixture {
        _root: TempDir,
        store: WorkflowStore,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("private runtime root");
            let private = root.path().join("authority");
            std::fs::create_dir(&private).expect("private authority directory");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
                    .expect("private authority permissions");
            }
            let store =
                WorkflowStore::open(private.join("authority.sqlite")).expect("workflow authority");
            Self { _root: root, store }
        }
    }

    fn digest(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    /// A distinct, valid projection per (revision, word) — the rows the
    /// journal writes now that the serialized image is gone. For the tests
    /// that are about the JOURNAL rather than about the actor.
    fn snapshot(revision: u64, word: &str) -> LedgerProjectionV1 {
        let mut held = Ledger::new().export();
        held.next_id =
            revision * 1009 + word.bytes().fold(0_u64, |sum, byte| sum + u64::from(byte));
        held
    }

    /// The pane table an actor is handed at the door.
    fn a_team() -> Team {
        Team::new("team-1", "leader-capability", 1)
    }

    /// The fixture's deterministic capability for a pane, registered by
    /// [`TableFixture::holding`] for every pane the team already has.
    fn capability_of(pane: &str) -> String {
        format!("cap-{pane}")
    }

    /// One verb, as a caller would send it.
    ///
    /// It carries a caller name because several verbs refuse without one — a
    /// pane with no session identity cannot be given a retryable answer, which
    /// is the ledger's rule and not this actor's. And it presents the leader
    /// pane's capability, because the door authenticates before it plans.
    fn a_command(argv: &[&str], now_ms: i64) -> PlanCommand {
        PlanCommand::checked(
            argv.iter().map(|word| (*word).to_string()).collect(),
            "team-1",
            "%1",
            capability_of("%1"),
            Some(format!("actor-v1:{}", "a".repeat(64))),
            now_ms,
        )
        .expect("a command inside the door's bounds")
    }

    /// The one table, as a test stands it up: the same shape the shell
    /// implements over its process-global mutex — teams beside their pane
    /// capabilities, taken in that order.
    #[derive(Clone)]
    struct TableFixture {
        teams: Arc<Mutex<std::collections::HashMap<String, Team>>>,
        tokens: Arc<Mutex<std::collections::HashMap<(String, String), String>>>,
    }

    impl TableFixture {
        fn holding(team: Team) -> Box<Self> {
            let mut tokens = std::collections::HashMap::new();
            for pane in team.panes() {
                tokens.insert((team.id.clone(), pane.id.clone()), capability_of(&pane.id));
            }
            let mut held = std::collections::HashMap::new();
            held.insert(team.id.clone(), team);
            Box::new(Self {
                teams: Arc::new(Mutex::new(held)),
                tokens: Arc::new(Mutex::new(tokens)),
            })
        }
    }

    impl PaneTable for TableFixture {
        fn incarnation(
            &self,
            team: &str,
            pane: &str,
        ) -> Option<zerocode_core::agent_teams::PaneIncarnation> {
            let teams = self.teams.lock().unwrap_or_else(|held| held.into_inner());
            let term = teams.get(team)?.term_of(pane)?;
            let tokens = self.tokens.lock().unwrap_or_else(|held| held.into_inner());
            Some(zerocode_core::agent_teams::PaneIncarnation {
                term,
                capability: tokens.get(&(team.to_string(), pane.to_string()))?.clone(),
            })
        }

        fn with_team(&self, team: &str, action: &mut dyn FnMut(Option<&mut Team>)) {
            let mut held = self
                .teams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            action(held.get_mut(team));
        }

        fn with_seat_of_term(&self, term: u32, action: &mut dyn FnMut(Option<(&str, &str)>)) {
            let held = self
                .teams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let seat = held.iter().find_map(|(id, team)| {
                team.panes()
                    .find(|pane| pane.term == term)
                    .map(|pane| (id.clone(), pane.id.clone()))
            });
            match &seat {
                Some((team, pane)) => action(Some((team, pane))),
                None => action(None),
            }
        }

        fn with_pane_authority(
            &self,
            team: &str,
            pane: &str,
            presented: &str,
            action: &mut dyn FnMut(Option<&mut Team>),
        ) {
            let mut held = self
                .teams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            /* A test double compares plainly; the constant-time requirement
             * binds the shell's implementation, which is the one an
             * adversary can time. Tokens under teams — the shell's order. */
            let authorized = held.get(team).is_some_and(|held| held.pane(pane).is_some())
                && self
                    .tokens
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&(team.to_string(), pane.to_string()))
                    .is_some_and(|expected| expected == presented);
            match authorized {
                true => action(held.get_mut(team)),
                false => action(None),
            }
        }
    }

    fn start(fixture: &Fixture, boot: RuntimeBoot, capacity: usize) -> RuntimeActor {
        RuntimeActor::start(
            &fixture.store,
            "main-ledger",
            boot,
            capacity,
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        )
        .expect("runtime actor")
    }

    /// A ledger's revision moves with the verbs that changed it, and with
    /// nothing else.
    ///
    /// The caller used to hand in the next revision itself, so this test used
    /// to check three refusals — a skipped number, a repeated one, a stamp
    /// from the past. None of them can be asked for any more: the actor is the
    /// only thing that numbers a ledger. What is left to prove is that it
    /// numbers it once per durable verb, not at all for a read, and that the
    /// number survives a shutdown.
    #[test]
    fn a_verb_that_wrote_moves_the_revision_and_a_read_does_not() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 4);
        assert_eq!(actor.view().expect("initial image").revision(), 1);

        let (decided, revision) = actor
            .plan(a_command(
                &["run-create", "--name", "nightly", "--retry-request", "r-1"],
                20,
            ))
            .expect("a run is created");
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        assert_eq!(revision, 2, "a verb that wrote did not move the revision");

        let (read, unchanged) = actor
            .plan(a_command(&["run-list"], 21))
            .expect("a read of the runs");
        assert_eq!(read.reply.exit_code, 0, "{}", read.reply.stderr);
        assert_eq!(unchanged, 2, "a read moved the revision");

        assert_eq!(actor.shutdown().expect("joined actor").revision(), 2);

        let reopened = start(&fixture, RuntimeBoot::Reopen, 4);
        let image = reopened.view().expect("reopened image");
        assert_eq!(image.revision(), 2);
        assert_eq!(image.projection().runs.len(), 1, "the run did not survive");
        reopened.shutdown().expect("join reopened actor");
    }

    /// A fresh boot will not adopt a ledger that is already there, and a
    /// reopen will not invent one that is not.
    ///
    /// Both halves are the same rule from opposite sides: a runtime says which
    /// of the two it means, and the store is what decides whether it can have
    /// it. A fresh boot that quietly took over an existing ledger is how a
    /// person's work would disappear with nobody told.
    #[test]
    fn a_fresh_boot_will_not_adopt_a_ledger_and_a_reopen_will_not_invent_one() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);
        assert_eq!(actor.view().expect("fresh image").revision(), 1);
        actor.shutdown().expect("fresh shutdown");

        assert_eq!(
            RuntimeActor::start(
                &fixture.store,
                "main-ledger",
                RuntimeBoot::Fresh { now_ms: 11 },
                2,
                TableFixture::holding(a_team()),
                Box::new(NoLauncher),
            )
            .expect_err("a second fresh boot"),
            RuntimeError::InitializationConflict
        );

        assert_eq!(
            RuntimeActor::start(
                &fixture.store,
                "a-ledger-nobody-made",
                RuntimeBoot::Reopen,
                2,
                TableFixture::holding(a_team()),
                Box::new(NoLauncher),
            )
            .expect_err("reopening what is not there"),
            RuntimeError::MissingLedger
        );

        let reopened = start(&fixture, RuntimeBoot::Reopen, 2);
        assert_eq!(reopened.view().expect("reopened image").revision(), 1);
        reopened.shutdown().expect("reopened shutdown");
    }

    /// A command too big for the door never reaches the mailbox, and the
    /// ledger does not move for it.
    ///
    /// This used to be about IMAGES: a caller handed in a whole ledger and the
    /// journal refused one above sixteen megabytes. Commands are verbs now, so
    /// the question moved with them — what must not get in is a command past
    /// the bound the mailbox's arithmetic depends on.
    #[test]
    fn an_oversized_command_never_enters_the_mailbox_or_moves_the_ledger() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);
        let before = actor.view().expect("image before").revision();

        let refused = PlanCommand::checked(
            vec!["send".to_string(), "b".repeat(MAX_COMMAND_BYTES)],
            "team-1",
            "%1",
            capability_of("%1"),
            None,
            20,
        )
        .expect_err("a command past the door");
        assert_eq!(refused, RuntimeError::InvalidInput);

        // The door is not the only net: the actor answers the same way.
        let (decided, revision) = actor
            .plan(a_command(
                &["run-create", "--name", "nightly", "--retry-request", "r-1"],
                20,
            ))
            .expect("a command inside the bound still works");
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        assert_eq!(revision, before + 1);
        actor.shutdown().expect("shutdown");
    }

    #[test]
    fn reopen_reports_prepared_and_unknown_recovery_and_blocks_mutation() {
        for unknown in [false, true] {
            let fixture = Fixture::new();
            let journal = EffectJournal::new(&fixture.store);
            let lease = journal
                .claim_authority("main-ledger", &digest(0xf5), 1)
                .expect("setup authority");
            assert_eq!(
                journal
                    .import_legacy(&lease, &snapshot(0, "legacy"), &digest(0xaa), 10)
                    .expect("legacy import"),
                ImportResult::Imported
            );
            let request = EffectRequest::new(
                "main-ledger",
                digest(0x11),
                digest(0x21),
                digest(0xb1),
                digest(0xe1),
                HostEffectKind::Split,
            )
            .expect("effect request");
            let BeginEffect::Execute(prepared) = journal
                .prepare(&lease, 0, &snapshot(1, "prepared"), &request, 20)
                .expect("prepare effect")
            else {
                panic!("expected prepared effect");
            };
            if unknown {
                journal
                    .mark_unknown(
                        &lease,
                        prepared,
                        1,
                        &snapshot(2, "unknown"),
                        HostEffectFailure::Timeout,
                        30,
                    )
                    .expect("unknown effect")
                    .abandon("fixture: this arm only needed the row to read `unknown`");
            } else {
                prepared
                    .abandon("fixture: this arm leaves the row prepared for the reopen to find");
            }
            drop(lease);
            drop(journal);

            let actor = start(&fixture, RuntimeBoot::Reopen, 2);
            let image = actor.view().expect("recovery image");
            assert_eq!(image.recoveries().len(), 1);
            assert_eq!(
                image.recoveries()[0].state(),
                if unknown {
                    HostEffectState::Unknown
                } else {
                    HostEffectState::Prepared
                }
            );
            assert_eq!(
                actor
                    .plan(a_command(
                        &[
                            "run-create",
                            "--name",
                            "blocked",
                            "--retry-request",
                            "r-blocked"
                        ],
                        40
                    ))
                    .expect_err("a mutation while a host effect is unfinished"),
                RuntimeError::RecoveryRequired
            );
            assert_eq!(actor.shutdown(), Err(RuntimeError::RecoveryRequired));
        }
    }

    #[test]
    fn database_fences_cross_slot_effects_before_actor_reopen() {
        let fixture = Fixture::new();
        let journal = EffectJournal::new(&fixture.store);
        let lease = journal
            .claim_authority("main-ledger", &digest(0xf6), 1)
            .expect("setup authority");
        journal
            .import_legacy(&lease, &snapshot(0, "legacy"), &digest(0xaa), 10)
            .expect("legacy import");
        let first = EffectRequest::new(
            "main-ledger",
            digest(0x31),
            digest(0x41),
            digest(0xb1),
            digest(0xe1),
            HostEffectKind::Split,
        )
        .expect("first request");
        let begun = journal
            .prepare(&lease, 0, &snapshot(1, "prepared"), &first, 20)
            .expect("first prepare");
        assert!(matches!(begun, BeginEffect::Execute(_)));
        if let BeginEffect::Execute(permit) = begun {
            permit.abandon("fixture: this test wanted the row read, not the row moved");
        }
        let second = EffectRequest::new(
            "main-ledger",
            digest(0x32),
            digest(0x42),
            digest(0xb1),
            digest(0xe1),
            HostEffectKind::Split,
        )
        .expect("second request");
        assert!(matches!(
            journal.prepare(&lease, 1, &snapshot(2, "second"), &second, 21),
            Err(EffectJournalError::EffectInFlight)
        ));
        assert_eq!(journal.open_operations(&lease).expect("open rows").len(), 1);
        drop(lease);
        drop(journal);
        let actor = start(&fixture, RuntimeBoot::Reopen, 2);
        assert_eq!(actor.view().expect("recovery image").recoveries().len(), 1);
        assert_eq!(actor.shutdown(), Err(RuntimeError::RecoveryRequired));
    }

    /// A window reopening after the wall clock stepped BACK still writes.
    ///
    /// The store refuses a head that moves backwards — the right rule. The
    /// clamp that satisfies it used to be seeded from zero, so a reopened
    /// window whose wall clock sat behind the head (an NTP correction, or a
    /// head written by a machine ahead of this one) had every mutation
    /// refused until real time caught up with the old head. The clamp is
    /// seeded from the head itself now.
    #[test]
    fn a_reopened_window_accepts_a_clock_behind_the_heads() {
        let fixture = Fixture::new();
        /* The AUTHORITY is claimed and released on an honest clock; only the
         * HEAD is written by a fast caller — which is the shape the defect
         * needs, since the head's own clock is what the reopened clamp must
         * inherit. */
        let ahead = wall_clock_ms().saturating_add(1_000_000);
        {
            let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 4);
            actor
                .plan(a_command(
                    &[
                        "run-create",
                        "--name",
                        "ahead",
                        "--retry-request",
                        "r-ahead",
                    ],
                    ahead,
                ))
                .expect("a run written at the fast clock");
            actor.shutdown().expect("a clean close");
        }
        let actor = start(&fixture, RuntimeBoot::Reopen, 4);
        let planned = actor.plan(a_command(
            &[
                "run-create",
                "--name",
                "behind",
                "--retry-request",
                "r-behind",
            ],
            50,
        ));
        assert!(
            planned.is_ok(),
            "a wall clock behind the head refused a mutation: {planned:?}"
        );
    }

    /// An operation the journal hands BACK (`Reconcile`) can be settled by
    /// the arm that receives it — and once settled, the same request name is
    /// offered a fresh attempt instead of the old refusal.
    ///
    /// Two halves. The VALIDATOR half: a retryable settlement with no retry
    /// floor is refused before the database is touched — the exact shape the
    /// shell's reconcile arm used to send and discard, leaving standing the
    /// very fence it was written to lift. The ROAD half: the valid shape
    /// settles, lowers the walk, and the next ask of the SAME name begins
    /// attempt two.
    #[test]
    fn a_reconciled_operation_settles_and_frees_its_name() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 4);
        let ask = || {
            EffectRequest::new(
                "main-ledger",
                digest(0x51),
                digest(0x61),
                digest(0xb5),
                digest(0xe5),
                HostEffectKind::Split,
            )
            .expect("a request")
        };
        let (begun, _) = actor.prepare_effect(ask(), 20).expect("the reservation");
        assert!(
            matches!(begun, BeginEffect::Execute(_)),
            "a fresh slot must Execute"
        );
        /* The crash shape: a window that took a permit and died without
         * settling it. It is ABANDONED rather than dropped now that the
         * difference can be said — this is the one fixture where letting a
         * permit go IS the thing under test. The next ask of the same slot
         * must get the operation handed back. */
        if let BeginEffect::Execute(permit) = begun {
            permit.abandon("fixture: the crash shape — this window never settles this one");
        }
        let (begun, _) = actor.prepare_effect(ask(), 21).expect("the second ask");
        let BeginEffect::Reconcile(permit) = begun else {
            panic!("an open operation must be handed back, got {begun:?}");
        };
        assert_eq!(
            actor.settle_effect(
                permit,
                EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Disconnected,
                    tombstone_digest: digest(0x71),
                    retryable: true,
                    retry_not_before_ms: None,
                    settled_at_ms: 22,
                },
            ),
            Err(RuntimeError::InvalidInput),
            "a retryable settlement with no floor must be refused whole"
        );
        /* The refused settlement moved nothing durable: the operation is
         * still open and the journal hands it back once more. */
        let (begun, _) = actor.prepare_effect(ask(), 23).expect("the third ask");
        let BeginEffect::Reconcile(permit) = begun else {
            panic!("the operation must still be open after the refused shape");
        };
        actor
            .settle_effect(
                permit,
                EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Disconnected,
                    tombstone_digest: digest(0x71),
                    retryable: true,
                    retry_not_before_ms: Some(25),
                    settled_at_ms: 24,
                },
            )
            .expect("the valid shape settles");
        let (begun, _) = actor
            .prepare_effect(ask(), 26)
            .expect("the name asked again");
        let BeginEffect::Execute(permit) = begun else {
            panic!("a settled retryable name must begin again, got {begun:?}");
        };
        assert_eq!(permit.attempt(), 2, "the fresh walk is attempt two");
        /* Close what this test opened, so nothing stays walking. */
        actor
            .settle_effect(
                permit,
                EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Refused,
                    tombstone_digest: digest(0x72),
                    retryable: false,
                    retry_not_before_ms: None,
                    settled_at_ms: 27,
                },
            )
            .expect("the cleanup settles");
    }

    #[test]
    fn bounded_mailbox_holds_the_line_and_shutdown_joins() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RuntimeActor>();
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 1);
        let sender = actor.sender.as_ref().expect("actor sender");
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::sync_channel(1);
        let (finished, finished_rx) = mpsc::sync_channel(1);
        sender
            .try_send(ActorCommand::Hold {
                entered,
                release: release_rx,
                finished,
            })
            .expect("hold command");
        entered_rx.recv().expect("actor entered hold");
        let (queued_reply, queued_answer) = mpsc::sync_channel(0);
        sender
            .try_send(ActorCommand::Request {
                request: Box::new(RuntimeRequest::View),
                reply: queued_reply,
            })
            .expect("fill one mailbox slot");
        /* The mailbox is full and the actor is held: the next caller WAITS
         * its turn rather than losing it. A refusal here was the first
         * design, and under a burst of verbs it scattered `MailboxFull`
         * across whichever callers lost the race — every one of which would
         * simply have asked again. */
        std::thread::scope(|turns| {
            let parked = turns.spawn(|| actor.view());
            release.send(()).expect("release actor");
            finished_rx.recv().expect("hold finished");
            assert!(matches!(
                queued_answer.recv().expect("queued reply"),
                Ok(RuntimeReply::View(_))
            ));
            assert!(
                parked.join().expect("the waiter").is_ok(),
                "the caller that stood in line was never answered"
            );
        });
        actor.shutdown().expect("shutdown through bounded mailbox");

        assert!(matches!(
            RuntimeActor::start(
                &fixture.store,
                "other-ledger",
                RuntimeBoot::Reopen,
                0,
                TableFixture::holding(a_team()),
                Box::new(NoLauncher)
            ),
            Err(RuntimeError::InvalidMailboxCapacity)
        ));
        assert!(matches!(
            RuntimeActor::start(
                &fixture.store,
                "other-ledger",
                RuntimeBoot::Reopen,
                MAX_RUNTIME_MAILBOX + 1,
                TableFixture::holding(a_team()),
                Box::new(NoLauncher),
            ),
            Err(RuntimeError::InvalidMailboxCapacity)
        ));
    }

    /// The coordinator made its review command while the actor was waiting;
    /// the worker's hand-in entered the mailbox first. The actor must compare
    /// the review with the source it has WHEN it commits the command (t-6815).
    #[test]
    fn a_queued_review_cannot_approve_the_source_that_preceded_a_hand_in() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let image = actor.view().expect("the seated run");
        let run = image.projection().runs[0].id.clone();
        let task = image.projection().tasks[0].id.clone();
        let attempt = image.projection().dispatches[0].id.clone();
        let (seated, _) = actor
            .plan(a_command(
                &["run-use", &run, "--retry-request", "seat-for-review"],
                11,
            ))
            .expect("the leader seats the legacy run");
        assert_eq!(seated.reply.exit_code, 0, "{}", seated.reply.stderr);
        let stale_review = a_command(
            &[
                "task-update",
                "--run",
                &run,
                "--task",
                &task,
                "--result",
                r#"{"verified":true,"testedHead":"abc1234"}"#,
                "--attempt",
                &attempt,
                "--source",
                "abc1234",
                "--retry-request",
                "queued-review",
            ],
            31,
        );
        let hand_in = PlanCommand::checked(
            vec![
                "send".to_string(),
                "--run".to_string(),
                run.clone(),
                "--type".to_string(),
                "worker_done".to_string(),
                "--body".to_string(),
                r#"{"ok":true,"head":"def5678"}"#.to_string(),
                "--retry-request".to_string(),
                "queued-hand-in".to_string(),
            ],
            "team-1",
            "%2",
            capability_of("%2"),
            Some(format!("actor-v1:{}", "b".repeat(64))),
            30,
        )
        .expect("the worker's command");

        let sender = actor.sender.as_ref().expect("actor sender");
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::sync_channel(1);
        let (finished, finished_rx) = mpsc::sync_channel(1);
        sender
            .try_send(ActorCommand::Hold {
                entered,
                release: release_rx,
                finished,
            })
            .expect("hold the actor");
        entered_rx.recv().expect("actor is held");
        let (report_reply, reported) = mpsc::sync_channel(1);
        sender
            .try_send(ActorCommand::Request {
                request: Box::new(RuntimeRequest::Plan(hand_in)),
                reply: report_reply,
            })
            .expect("the hand-in stands first");
        let (review_reply, reviewed) = mpsc::sync_channel(1);
        sender
            .try_send(ActorCommand::Request {
                request: Box::new(RuntimeRequest::Plan(stale_review)),
                reply: review_reply,
            })
            .expect("the earlier observation stands second");
        release.send(()).expect("release the actor");
        finished_rx.recv().expect("actor has resumed");
        let RuntimeReply::Planned {
            decided: report,
            revision: report_revision,
        } = reported
            .recv()
            .expect("hand-in reply")
            .expect("hand-in decision")
        else {
            panic!("the hand-in was not planned");
        };
        assert_eq!(report.reply.exit_code, 0, "{}", report.reply.stderr);
        let RuntimeReply::Planned {
            decided: review,
            revision: review_revision,
        } = reviewed
            .recv()
            .expect("review reply")
            .expect("review decision")
        else {
            panic!("the review was not planned");
        };
        assert_ne!(review.reply.exit_code, 0, "the old source was accepted");
        assert!(
            review.reply.stderr.contains("source"),
            "{}",
            review.reply.stderr
        );
        assert_eq!(
            review_revision, report_revision,
            "a refusal wrote a success receipt"
        );
        let image = actor.view().expect("the ledger after both requests");
        let task_row = image
            .projection()
            .tasks
            .iter()
            .find(|one| one.id == task)
            .expect("the task");
        assert!(task_row.result.as_str().contains("def5678"));
        actor.shutdown().expect("join the actor");
    }

    #[test]
    fn a_live_actor_fences_external_authority_claims() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);
        let journal = EffectJournal::new(&fixture.store);
        assert!(matches!(
            journal.claim_authority("main-ledger", &digest(0xf7), 20),
            Err(EffectJournalError::OwnerBusy)
        ));
        assert_eq!(actor.view().expect("owner remains live").revision(), 1);
        actor.shutdown().expect("release owner");
    }

    #[test]
    fn a_duplicate_actor_loses_the_durable_cas_and_closes() {
        let fixture = Fixture::new();
        let owner = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);
        assert!(matches!(
            RuntimeActor::start(
                &fixture.store,
                "main-ledger",
                RuntimeBoot::Reopen,
                2,
                TableFixture::holding(a_team()),
                Box::new(NoLauncher)
            ),
            Err(RuntimeError::AuthorityChanged)
        ));
        assert_eq!(owner.shutdown().expect("join owner").revision(), 1);
    }

    /// A runtime that could not write does not adopt somebody else's ledger
    /// while it is trying to recover.
    ///
    /// The healthy path refuses a ledger that moved underneath it. The
    /// recovering path reads the disk on purpose, so without a rule of its own
    /// it would take whatever it found — a fail-open reachable only after a
    /// fault, which is the worst place to have one.
    #[test]
    fn a_recovering_runtime_will_not_take_over_a_ledger_that_moved() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);

        let connection = fixture.store.connection().expect("store connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_runtime_advance
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected runtime failure'); END;",
            )
            .expect("runtime fault");
        assert_eq!(
            actor
                .plan(a_command(
                    &["run-create", "--name", "lost", "--retry-request", "r-lost"],
                    20,
                ))
                .expect_err("a write the disk refuses"),
            RuntimeError::NotDurable
        );
        connection
            .execute_batch("DROP TRIGGER fail_runtime_advance;")
            .expect("remove runtime fault");

        // Somebody else moves the ledger while this one is poisoned.
        ledger_store::write(
            &connection,
            "main-ledger",
            1,
            2,
            &Ledger::new().export(),
            21,
        )
        .expect("an outside writer");
        drop(connection);

        assert_eq!(
            actor
                .view()
                .expect_err("a recovering runtime reading somebody else's ledger"),
            RuntimeError::AuthorityChanged
        );
    }

    /// A write that dies half-way leaves the ledger that was there, whole.
    ///
    /// `ledger_store::write` moves the head, clears every row and re-inserts
    /// them, on the caller's connection. Without a transaction around it each
    /// statement commits alone, so a fault after the head and the clearing
    /// leaves a head counting prose whose rows are gone — a store every read
    /// from then on refuses as corrupt, across restarts. The fault here lands
    /// on the first row insert, where the real one did; the ledger the disk
    /// holds afterwards must be the one from before, and the runtime must be
    /// able to take the disk's word and answer again once the fault is gone.
    #[test]
    fn a_write_that_dies_half_way_leaves_the_previous_ledger_whole() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);

        let connection = fixture.store.connection().expect("store connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_row_insert
                    AFTER INSERT ON ledger_runs
                    BEGIN SELECT RAISE(ABORT, 'injected row failure'); END;",
            )
            .expect("row fault");
        assert_eq!(
            actor
                .plan(a_command(
                    &["run-create", "--name", "lost", "--retry-request", "r-lost"],
                    20,
                ))
                .expect_err("a write the rows refused"),
            RuntimeError::NotDurable
        );
        connection
            .execute_batch("DROP TRIGGER fail_row_insert;")
            .expect("remove row fault");

        let held = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
            .expect("the store still reads as one ledger")
            .expect("and the ledger is there");
        assert_eq!(held.revision, 1, "the head moved without its rows");
        assert!(held.projection.runs.is_empty(), "the lost run half-landed");
        drop(connection);

        assert_eq!(
            actor
                .view()
                .expect("a recovering runtime takes the disk's word")
                .revision(),
            1
        );
    }

    /// A ledger that moved under the actor is noticed before anything else
    /// happens, and the actor closes rather than writing over it.
    ///
    /// The lease says who OWNS the ledger; this says whether the ledger is
    /// still where the owner left it. They answer different questions: a
    /// second writer that somehow held no lease would leave the lease intact
    /// and the rows changed, and the next verb from this actor would be built
    /// on a ledger that no longer exists.
    #[test]
    fn a_ledger_moved_by_something_else_is_noticed_and_the_actor_closes() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);
        assert_eq!(
            actor.view().expect("the image it booted with").revision(),
            1
        );

        {
            let connection = fixture.store.connection().expect("store connection");
            ledger_store::write(
                &connection,
                "main-ledger",
                1,
                2,
                &Ledger::new().export(),
                20,
            )
            .expect("somebody else writes the same ledger");
        }

        assert_eq!(
            actor.view().expect_err("a ledger that moved underneath"),
            RuntimeError::AuthorityChanged
        );
    }

    /// A store that refuses the write keeps its refusal: nothing lands, the
    /// decision never leaves, and the runtime answers nothing until the disk
    /// speaks again — then it carries on.
    ///
    /// Four promises in one fault, because they are one promise seen from four
    /// sides. The verb happened in memory and nowhere else; a decision handed
    /// back would have told the window to carry out an effect for a write the
    /// disk refused; a READ answered from that memory would show somebody the
    /// same phantom; and a passing fault must not cost the pane table, the
    /// launcher, and the thread that owns them.
    #[test]
    fn a_write_the_store_refuses_lands_nothing_answers_nothing_and_then_recovers() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);
        let before = actor.view().expect("image before the fault").revision();

        let connection = fixture.store.connection().expect("fault connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_runtime_advance
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected runtime failure'); END;",
            )
            .expect("runtime fault");

        // ① The verb does not land, and what comes back is a refusal — never
        //    a decision the window could act on.
        let refused = actor
            .plan(a_command(
                &[
                    "run-create",
                    "--name",
                    "must-not-land",
                    "--retry-request",
                    "r-lost",
                ],
                20,
            ))
            .expect_err("a write the disk refuses");
        assert_eq!(refused, RuntimeError::NotDurable);

        /* ② The next request is answered from the DISK, not from the memory
         *    that ran ahead of it. The store can still be read here, so the
         *    runtime takes its word at once and the phantom run is simply not
         *    there — which is the whole point of refusing to answer from a
         *    ledger it could not write. */
        let image = actor.view().expect("a read after taking the disk's word");
        assert_eq!(image.revision(), before, "recovery invented a revision");
        assert!(
            image.projection().runs.is_empty(),
            "the run that never landed came back anyway"
        );

        /* ③ Take the fault away and it carries on — same thread, same pane
         *    table, same launcher. A passing fault costing a restart is the
         *    thing this refusal exists to avoid. */
        connection
            .execute_batch("DROP TRIGGER fail_runtime_advance;")
            .expect("remove runtime fault");
        let (decided, revision) = actor
            .plan(a_command(
                &[
                    "run-create",
                    "--name",
                    "after-the-fault",
                    "--retry-request",
                    "r-after",
                ],
                30,
            ))
            .expect("a verb after recovery");
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        assert_eq!(revision, before + 1);

        let connection = fixture.store.connection().expect("store connection");
        assert_eq!(
            ledger_store::head_revision(&connection, "main-ledger").expect("head"),
            Some(before + 1)
        );
        drop(connection);
        actor
            .shutdown()
            .expect("the actor is still alive to be shut down");
    }

    #[test]
    fn actor_panic_and_closed_channel_are_terminal_fail_closed_states() {
        let fixture = Fixture::new();
        let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 2);
        actor
            .sender
            .as_ref()
            .expect("panic sender")
            .try_send(ActorCommand::Panic)
            .expect("panic command");
        assert_eq!(actor.view(), Err(RuntimeError::Panicked));
        assert_eq!(actor.shutdown(), Err(RuntimeError::Panicked));

        let closed = start(&fixture, RuntimeBoot::Reopen, 2);
        closed
            .sender
            .as_ref()
            .expect("close sender")
            .try_send(ActorCommand::Close)
            .expect("close command");
        assert_eq!(closed.view(), Err(RuntimeError::Closed));
        assert_eq!(closed.shutdown(), Err(RuntimeError::Closed));
    }

    #[test]
    fn runtime_debug_never_discloses_snapshot_bytes_digests_or_store_paths() {
        let fixture = Fixture::new();
        let secret = "/private/project raw-session --token";
        let legacy = digest(0xdd);
        let initial = snapshot(0, secret);
        let boot = RuntimeBoot::Fresh { now_ms: 10 };
        let boot_rendered = format!("{boot:?}");
        let request = RuntimeRequest::Plan(
            PlanCommand::checked(
                vec!["send".to_string(), "--body".to_string(), secret.to_string()],
                secret.to_string(),
                "%1",
                secret.to_string(),
                Some(secret.to_string()),
                20,
            )
            .expect("a command carrying what nobody should see"),
        );
        let actor = start(&fixture, boot, 2);
        /* The secret has to actually LAND in rows before anything renders,
         * or every absence below is an empty container counted redacted. A
         * run NAMED the secret puts it in the projection, and the planned
         * reply carries the decision that named it. */
        let (planned, _) = actor
            .plan(
                PlanCommand::checked(
                    vec![
                        "run-create".to_string(),
                        "--name".to_string(),
                        secret.to_string(),
                        "--retry-request".to_string(),
                        "r-privacy".to_string(),
                    ],
                    "team-1",
                    "%1",
                    capability_of("%1"),
                    Some(secret.to_string()),
                    20,
                )
                .expect("a verb carrying what nobody should see"),
            )
            .expect("the secret lands in rows");
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        /* And in MORE than one table: a redaction that hand-picks counts is
         * broken per field, so every field this fixture can reach carries
         * the secret — a run named it, a message says it, a task specs it. */
        for argv in [
            vec![
                "send".to_string(),
                "--type".to_string(),
                "status".to_string(),
                "--body".to_string(),
                secret.to_string(),
                "--retry-request".to_string(),
                "r-privacy-2".to_string(),
            ],
            vec![
                "task-create".to_string(),
                "--spec".to_string(),
                secret.to_string(),
                "--retry-request".to_string(),
                "r-privacy-3".to_string(),
            ],
        ] {
            let (spoke, _) = actor
                .plan(
                    PlanCommand::checked(
                        argv,
                        "team-1",
                        "%1",
                        capability_of("%1"),
                        Some(secret.to_string()),
                        21,
                    )
                    .expect("a verb carrying what nobody should see"),
                )
                .expect("the secret lands in another table");
            assert_eq!(spoke.reply.exit_code, 0, "{}", spoke.reply.stderr);
        }
        let image = actor.view().expect("runtime image");
        assert!(
            image
                .projection()
                .runs
                .iter()
                .any(|run| run.name.as_str() == secret),
            "the fixture has to actually carry the secret in a row"
        );
        assert!(
            !image.projection().messages.is_empty() && !image.projection().tasks.is_empty(),
            "the fixture has to carry the secret in messages and tasks too"
        );
        /* And the ROWS' own Debug — the string a `dbg!(projection)` prints.
         *
         * Without this the leaf redaction has no witness at all: every
         * surface above renders counts and shapes, so a `Text` that stopped
         * hiding would never be reached by anything this test looks at, and
         * a row field that was never a `Text` would not be reached either.
         * Both were true at once here — `BoundRow::caller` was a bare
         * `String` holding the caller's own name, in the same projection
         * where `ServedRow::caller` hid it. Two guards covering for each
         * other look exactly like one guard working. */
        let rows = format!("{:?}", image.projection());
        assert!(
            !rows.contains(secret),
            "a row's own Debug disclosed the caller"
        );
        let reply = RuntimeReply::View(image.clone());
        let cutover_boot = RuntimeBoot::Cutover {
            legacy: Some(Box::new((snapshot(0, secret), legacy.clone()))),
            now_ms: 10,
        };
        /* The new doors' own Debug, each fed the secret everywhere a caller
         * could put one. The receipt key is a REAL one — its fields cannot
         * be built by hand, so it is taken from a decision whose caller and
         * retry name carry the secret. */
        let (waiting_decided, _) = actor
            .plan(
                PlanCommand::checked(
                    vec![
                        "check".to_string(),
                        "--wait".to_string(),
                        "--retry-request".to_string(),
                        secret.to_string(),
                    ],
                    "team-1",
                    "%1",
                    capability_of("%1"),
                    Some(secret.to_string()),
                    22,
                )
                .expect("a wait whose key carries the secret"),
            )
            .expect("the wait decision");
        let secret_key = waiting_decided
            .receipt
            .clone()
            .expect("a named retry has a key");
        let look_again_request = RuntimeRequest::LookAgain {
            waiting: Box::new(Waiting {
                run: secret.to_string(),
                address: secret.to_string(),
                kinds: Vec::new(),
                peek: false,
                deadline_ms: None,
                acked: false,
                format: false,
                thread: Some(secret.to_string()),
                seat: "team-secret/%1".to_string(),
            }),
            receipt: Some(Box::new(secret_key.clone())),
            now_ms: 22,
        };
        let serve_request = RuntimeRequest::ServeReceipt {
            decided: Box::new(*waiting_decided),
            now_ms: 22,
        };
        let release_request = RuntimeRequest::ReleaseSettled {
            worker: secret.to_string(),
            screen: Some(secret.to_string()),
            now_ms: 22,
        };
        let remember_request = RuntimeRequest::RememberServed {
            key: Box::new(secret_key),
            stdout: secret.to_string(),
            now_ms: 22,
        };
        let dissolved_request = RuntimeRequest::TeamDissolved {
            team: secret.to_string(),
            now_ms: 22,
        };
        let missing_request = RuntimeRequest::PanesMissing {
            missing: vec![(secret.to_string(), 20)],
            now_ms: 22,
        };
        let seen_request = RuntimeRequest::PanesSeen {
            workers: vec![secret.to_string()],
            now_ms: 22,
        };
        let never_opened = RuntimeRequest::SeatNeverOpened {
            worker: secret.to_string(),
            now_ms: 20,
        };
        let gone = RuntimeRequest::TerminalGone {
            term: 7,
            screen: Some(secret.to_string()),
            now_ms: 20,
        };
        let settled = RuntimeReply::Settled {
            moved: true,
            revision: 1,
        };
        let rendered = format!(
            "{boot_rendered} {cutover_boot:?} {actor:?} {image:?} {reply:?} {request:?} \
             {planned:?} {never_opened:?} {gone:?} {settled:?} {look_again_request:?} \
             {serve_request:?} {release_request:?} {remember_request:?} \
             {dissolved_request:?} {missing_request:?} {seen_request:?} {:?}",
            initial.next_id
        );
        assert!(!rendered.contains(secret));
        assert!(!rendered.contains(&legacy));
        assert!(!rendered.contains(&fixture._root.path().to_string_lossy().into_owned()));
        actor.shutdown().expect("join redaction actor");
    }

    /* ---- cutover v2: the one table, the host's facts, and the effect
     * round trip ------------------------------------------------------- */

    /// A launcher that answers, so a `worker-start` can actually decide.
    struct AnsweringLauncher;

    impl Launcher for AnsweringLauncher {
        fn command_for(
            &self,
            agent: &str,
            prompt: &str,
            _tuning: &[String],
        ) -> Result<String, String> {
            Ok(format!("{agent} --prompt {prompt}"))
        }
    }

    /// The table with a second seat: pane `%2` at term 7, split from the
    /// leader — through the same door the shell records a real split.
    fn a_seated_table() -> Box<TableFixture> {
        let mut team = a_team();
        team.record_split(
            "%2",
            7,
            "%1",
            zerocode_core::agent_teams::Direction::Horizontal,
        );
        TableFixture::holding(team)
    }

    /// A legacy ledger with a worker seated at ("team-1", "%2"), on a task,
    /// exactly as a person's file would hold one.
    fn a_seated_legacy() -> LedgerProjectionV1 {
        let mut legacy = Ledger::new();
        let run = legacy.create_run("nightly", 5);
        let task = legacy
            .create_task(
                &run,
                "carry the wall".to_string(),
                "wall".to_string(),
                Vec::new(),
                None,
                5,
            )
            .expect("a task for the seat");
        legacy
            .start_worker(&run, "codex", ("team-1", "%2"), Some(&task), 6)
            .expect("a seated worker");
        legacy.export()
    }

    fn cutover(legacy: Option<LedgerProjectionV1>, now_ms: i64) -> RuntimeBoot {
        RuntimeBoot::Cutover {
            legacy: legacy.map(|initial| Box::new((initial, digest(0xaa)))),
            now_ms,
        }
    }

    fn start_with(
        fixture: &Fixture,
        boot: RuntimeBoot,
        panes: Box<TableFixture>,
        launcher: Box<dyn Launcher + Send>,
    ) -> RuntimeActor {
        RuntimeActor::start(&fixture.store, "main-ledger", boot, 4, panes, launcher)
            .expect("runtime actor")
    }

    /// The three ways a cutover boot comes up: import the file once, reopen
    /// from the store thereafter — the file no longer argued with — and
    /// fresh when there is neither. Import and bare fresh leave the SAME
    /// shape: a revision-zero image whose first verb is one.
    #[test]
    fn a_cutover_boot_imports_once_then_the_store_wins() {
        let fixture = Fixture::new();
        let legacy = a_seated_legacy();
        let actor = start_with(
            &fixture,
            cutover(Some(legacy.clone()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let imported = actor.view().expect("the imported image");
        assert_eq!(imported.revision(), 0, "an import is revision zero");
        assert!(
            *imported.projection() == legacy,
            "the imported rows are the file's rows"
        );
        let (decided, revision) = actor
            .plan(a_command(
                &["run-create", "--name", "second", "--retry-request", "r-2"],
                20,
            ))
            .expect("a verb on the imported ledger");
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        assert_eq!(revision, 1);
        actor.shutdown().expect("join cutover actor");

        /* The same boot again, carrying a STALE copy of the file. The store
         * holds revision one now; a boot that read the file again would
         * hand the person a ledger with their second run missing. */
        let again = start_with(
            &fixture,
            cutover(Some(legacy), 30),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let reopened = again.view().expect("the reopened image");
        assert_eq!(reopened.revision(), 1, "the store's revision won");
        assert_eq!(
            reopened.projection().runs.len(),
            2,
            "the run planned after the import survived the stale file"
        );
        again.shutdown().expect("join reopened actor");

        // And with neither store nor file: the journal's own fresh door,
        // leaving the same revision-zero shape an import leaves.
        let bare = Fixture::new();
        let fresh = start_with(
            &bare,
            cutover(None, 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let image = fresh.view().expect("the fresh image");
        assert_eq!(image.revision(), 0, "a bare boot image is revision zero");
        assert!(image.projection().runs.is_empty());
        let (_, revision) = fresh
            .plan(a_command(
                &["run-create", "--name", "first", "--retry-request", "r-f"],
                20,
            ))
            .expect("the first verb");
        assert_eq!(revision, 1, "the first verb is one");
        fresh.shutdown().expect("join fresh actor");
    }

    /// A terminal the host closed settles its seat durably, exactly once —
    /// addressed by TERM, resolved under the table's own lock.
    #[test]
    fn a_terminal_gone_settles_the_seat_durably_and_only_once() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor.terminal_gone(7, 20).expect("the settlement"),
            (true, 1),
            "the seat had an attempt to settle"
        );
        assert_eq!(
            actor.terminal_gone(7, 21).expect("the second settlement"),
            (false, 1),
            "a seat settles once, and a no-op does not move the revision"
        );
        assert_eq!(
            actor.terminal_gone(99, 22).expect("an unknown terminal"),
            (false, 1),
            "a terminal no table knows settles nothing"
        );
        actor.shutdown().expect("join settling actor");

        // The settlement survives the window: a reopen still has nothing to
        // settle, at the same revision.
        let again = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            again
                .terminal_gone(7, 30)
                .expect("a settlement after reopen"),
            (false, 1),
            "the settlement did not survive the window"
        );
        again.shutdown().expect("join reopened actor");
    }

    /// A turn's silence is written once; the same turn again and an
    /// interrupted one write nothing.
    #[test]
    fn a_turn_that_ended_is_written_once_and_an_interrupted_one_is_not() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor
                .pane_turn_ended(7, 15, false, 20)
                .expect("the silence"),
            (true, 1),
            "a finished turn writes the worker's silence"
        );
        assert_eq!(
            actor
                .pane_turn_ended(7, 15, false, 21)
                .expect("the silence again"),
            (false, 1),
            "the same turn does not write twice"
        );
        assert_eq!(
            actor
                .pane_turn_ended(7, 16, true, 22)
                .expect("an interrupted turn"),
            (false, 1),
            "an interrupted turn is the person typing, not the worker quiet"
        );
        actor.shutdown().expect("join quiet actor");
    }

    #[test]
    fn readiness_records_a_failed_delivery_and_a_sound_on_the_same_beat_wins() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );

        assert_eq!(
            actor
                .readiness_sweep(Vec::new(), vec![(7, 15)], 20)
                .expect("failed delivery"),
            (false, 1),
            "channel evidence was mistaken for coordinator mail"
        );
        assert_eq!(
            actor.view().expect("marked image").projection().workers[0].hook_unreachable_since_ms,
            Some(15)
        );
        actor
            .readiness_sweep(vec![7], vec![(7, 15)], 21)
            .expect("sound after the stale marker");
        assert_eq!(
            actor.view().expect("recovered image").projection().workers[0]
                .hook_unreachable_since_ms,
            None,
            "the marker beat a payload the window actually received"
        );
        actor.shutdown().expect("join hearing actor");
    }

    #[test]
    fn missing_and_seen_pane_facts_are_level_triggered_and_durable() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let worker = actor.view().expect("the worker image").projection().workers[0]
            .id
            .clone();

        assert_eq!(
            actor
                .panes_missing(vec![(worker.clone(), 15)], 20)
                .expect("the missing pane"),
            (true, 1)
        );
        assert_eq!(
            actor
                .panes_missing(vec![(worker.clone(), 16)], 21)
                .expect("the same missing pane"),
            (false, 1),
            "a level-triggered miss wrote twice"
        );
        let marked = actor.view().expect("the marked image");
        assert_eq!(
            marked.projection().workers[0].pane_missing_since_ms,
            Some(15)
        );
        assert_eq!(
            marked
                .projection()
                .messages
                .iter()
                .filter(|message| message.body.as_str().contains("pane_missing"))
                .count(),
            1
        );
        actor.shutdown().expect("join missing-pane actor");

        let actor = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor
                .view()
                .expect("the reopened missing image")
                .projection()
                .workers[0]
                .pane_missing_since_ms,
            Some(15),
            "the missing episode did not survive the window"
        );

        assert_eq!(
            actor
                .panes_seen(vec![worker.clone()], 22)
                .expect("the returned pane"),
            (true, 2)
        );
        assert_eq!(
            actor
                .panes_seen(vec![worker], 23)
                .expect("the same returned pane"),
            (false, 2)
        );
        actor.shutdown().expect("join pane actor");

        let again = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            again
                .view()
                .expect("the reopened image")
                .projection()
                .workers[0]
                .pane_missing_since_ms,
            None,
            "the positive probe was not durable"
        );
        again.shutdown().expect("join reopened pane actor");
    }

    /// The rollback fact: a worker whose pane never opened goes back,
    /// durably — a worker nobody seated is a no-op answer — and a worker
    /// whose seat IS open is refused, because erasing it would orphan a
    /// live agent.
    #[test]
    fn a_seat_that_never_opened_takes_its_worker_row_back() {
        let fixture = Fixture::new();
        /* The table holds only the leader pane: the worker's `%2` never
         * opened, which is the exact world this door exists for. */
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        let seated = actor.view().expect("before the rollback");
        let worker = seated.projection().workers[0].id.clone();
        assert_eq!(
            actor
                .seat_never_opened(worker.clone(), 20)
                .expect("the rollback"),
            (true, 1),
        );
        assert!(
            actor
                .view()
                .expect("after the rollback")
                .projection()
                .workers
                .is_empty(),
            "the worker row survived a pane that never opened"
        );
        assert_eq!(
            actor
                .seat_never_opened(worker, 21)
                .expect("the second rollback"),
            (false, 1),
            "a worker already gone is an answer, not a revision"
        );
        actor.shutdown().expect("join rollback actor");

        /* And the refusal arm, on a table where the seat DID open: the door
         * checks its own name, so a caller lying about the host cannot
         * erase a live agent's worker row. */
        let open_fixture = Fixture::new();
        let open_actor = start_with(
            &open_fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let live = open_actor.view().expect("the live seat");
        let live_worker = live.projection().workers[0].id.clone();
        assert_eq!(
            open_actor
                .seat_never_opened(live_worker.clone(), 20)
                .expect_err("a seat that opened must refuse its own erasure"),
            RuntimeError::SeatStillOpen,
        );
        assert_eq!(
            open_actor
                .view()
                .expect("the ledger afterwards")
                .projection()
                .workers[0]
                .id,
            live_worker,
            "the refusal erased the worker anyway"
        );
        open_actor.shutdown().expect("join refusing actor");
    }

    /// The witness the review asked for by name: the actor can see the
    /// worker it just made. The whole round trip in one test — plan mints
    /// the worker and the effect, the caller executes and records the seat
    /// under the ONE table, the journal brackets it, and the next verb
    /// counts the worker as live.
    #[test]
    fn the_actor_can_see_the_worker_it_just_made() {
        let fixture = Fixture::new();
        let shared = TableFixture::holding(a_team());
        let actor = start_with(
            &fixture,
            cutover(None, 10),
            Box::new((*shared).clone()),
            Box::new(AnsweringLauncher),
        );
        actor
            .plan(a_command(
                &["run-create", "--name", "nightly", "--retry-request", "r-1"],
                20,
            ))
            .expect("a run");
        let task = actor
            .plan(a_command(
                &["task-create", "--spec", "carry", "--retry-request", "r-2"],
                21,
            ))
            .expect("a task");
        let task_id = task.0.reply.stdout;
        let task_id = task_id
            .split("\"taskId\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("a task id in the reply")
            .to_string();
        let (started, _) = actor
            .plan(a_command(
                &[
                    "worker-start",
                    "--agent",
                    "claude",
                    "--task",
                    &task_id,
                    "--retry-request",
                    "r-3",
                ],
                22,
            ))
            .expect("the worker starts in the ledger");
        assert_eq!(started.reply.exit_code, 0, "{}", started.reply.stderr);
        let zerocode_core::agent_teams::Effect::Split {
            pane,
            from,
            direction,
            ..
        } = started.effect
        else {
            panic!("a worker start carries a split effect");
        };

        /* Before the pane opens, the roster is honestly empty — the exact
         * line the review's demo printed as a lie is the truth here,
         * because nothing has recorded the seat yet. */
        let (roster, _) = actor
            .plan(a_command(&["worker-list"], 23))
            .expect("a roster before the seat");
        assert!(
            roster.reply.stdout.contains("\"workers\":[]"),
            "before the seat lands the roster is honestly empty: out={} err={} exit={}",
            roster.reply.stdout,
            roster.reply.stderr,
            roster.reply.exit_code
        );

        /* The caller's half, exactly as CUT-2's run() will do it: the
         * journal brackets the effect, the host opens the pane, the seat is
         * recorded into the ONE table — the same object the actor borrows —
         * and the outcome is settled. */
        let request = EffectRequest::new(
            "main-ledger",
            digest(0x31),
            digest(0x41),
            digest(0xb1),
            digest(0xe1),
            HostEffectKind::Split,
        )
        .expect("an effect request");
        let (begun, _) = actor.prepare_effect(request, 24).expect("the reservation");
        let BeginEffect::Execute(permit) = begun else {
            panic!("a fresh effect executes, got {begun:?}");
        };
        shared.with_team("team-1", &mut |team| {
            team.expect("the one team")
                .record_split(&pane, 7, &from, direction);
        });
        actor
            .settle_effect(
                permit,
                EffectSettlement::Applied {
                    result_digest: digest(0x91),
                    settled_at_ms: 25,
                },
            )
            .expect("the settlement");

        let worker_id = started
            .reply
            .stdout
            .split("\"workerId\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("a worker id in the start reply")
            .to_string();
        let (after, _) = actor
            .plan(a_command(&["worker-list"], 26))
            .expect("a roster after the seat");
        /* Asserted in the POSITIVE: the roster answers, and it contains the
         * worker by name. The negative form (`!contains("workers\":[]")`)
         * was green for a roster that REFUSED outright — an empty stdout
         * contains nothing, including the emptiness marker. */
        assert_eq!(after.reply.exit_code, 0, "{}", after.reply.stderr);
        assert!(
            after.reply.stdout.contains(&worker_id),
            "the actor cannot see the worker it just made: {}",
            after.reply.stdout
        );
        actor.shutdown().expect("join witness actor");
    }

    /// A host fact the disk refuses poisons the runtime exactly like a verb:
    /// nothing lands, nothing answers, and the disk's word recovers it.
    #[test]
    fn a_host_fact_the_disk_refuses_lands_nothing_and_recovers() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let connection = fixture.store.connection().expect("fault connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_seat_settlement
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected settlement failure'); END;",
            )
            .expect("settlement fault");
        assert_eq!(
            actor
                .terminal_gone(7, 20)
                .expect_err("a settlement the disk refuses"),
            RuntimeError::NotDurable,
        );
        /* The next read answers from the DISK, where the worker is still
         * seated — the settlement that could not be written is not shown. */
        let image = actor.view().expect("a read after the disk's word");
        assert_eq!(image.revision(), 0);
        connection
            .execute_batch("DROP TRIGGER fail_seat_settlement;")
            .expect("remove settlement fault");
        assert_eq!(
            actor
                .terminal_gone(7, 30)
                .expect("the settlement after the fault"),
            (true, 1),
            "the seat was still settleable because the refused write never half-landed"
        );
        actor.shutdown().expect("join recovered actor");
    }

    /// While a host effect is half-finished, a host fact is refused like a
    /// verb is — the reconciliation about to run must not have the ledger
    /// move underneath it. The effect doors themselves stay open: they ARE
    /// the recovery road.
    #[test]
    fn a_host_fact_meets_the_same_recovery_fence_a_verb_does() {
        let fixture = Fixture::new();
        let journal = EffectJournal::new(&fixture.store);
        let lease = journal
            .claim_authority("main-ledger", &digest(0xf7), 1)
            .expect("setup authority");
        journal
            .import_legacy(&lease, &a_seated_legacy(), &digest(0xaa), 10)
            .expect("legacy import");
        let request = EffectRequest::new(
            "main-ledger",
            digest(0x33),
            digest(0x43),
            digest(0xb3),
            digest(0xe3),
            HostEffectKind::Split,
        )
        .expect("an effect request");
        let begun = journal
            .prepare(&lease, 0, &a_seated_legacy(), &request, 20)
            .expect("prepare");
        assert!(matches!(begun, BeginEffect::Execute(_)));
        if let BeginEffect::Execute(permit) = begun {
            permit.abandon("fixture: this test wanted the row read, not the row moved");
        }
        drop(lease);
        drop(journal);

        let actor = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor
                .terminal_gone(7, 30)
                .expect_err("a fact while a host effect is unfinished"),
            RuntimeError::RecoveryRequired,
        );
        /* The boot-time sweep and the dissolution meet the SAME fence: the
         * boot drives recovery first, and a door that slipped past it would
         * have the ledger move under the reconciliation. */
        assert_eq!(
            actor
                .window_restarted(30)
                .expect_err("a sweep while a host effect is unfinished"),
            RuntimeError::RecoveryRequired,
        );
        assert_eq!(
            actor
                .team_dissolved("team-1", 30)
                .expect_err("a dissolution while a host effect is unfinished"),
            RuntimeError::RecoveryRequired,
        );
        assert_eq!(
            actor
                .panes_missing(vec![("w-1".to_string(), 20)], 30)
                .expect_err("a pane reconciliation while a host effect is unfinished"),
            RuntimeError::RecoveryRequired,
        );
        assert_eq!(
            actor
                .panes_seen(vec!["w-1".to_string()], 30)
                .expect_err("a pane recovery while a host effect is unfinished"),
            RuntimeError::RecoveryRequired,
        );
        /* And the door that LIFTS the fence: the recovered permit never left
         * the actor, so the caller brings only the outcome. A window that
         * died mid-effect can prove exactly one thing about the old backend
         * — it is gone — which is a fence, not an application. */
        /* First a settlement the runtime REFUSES. It has settled nothing, so
         * the recovery must still be there to settle — with the very permit
         * the refusal handed back. Before the doors returned their permits,
         * this was recovered by asking the journal to MINT the row a second
         * time, which worked and was also the clearest proof that a one-use
         * capability could be issued twice for one row. */
        assert_eq!(
            actor
                .settle_recovered(EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Disconnected,
                    tombstone_digest: digest(0xdc),
                    retryable: true,
                    retry_not_before_ms: None,
                    settled_at_ms: 31,
                })
                .expect_err("a retryable settlement with no floor is refused whole"),
            RuntimeError::InvalidInput,
        );
        assert_eq!(
            actor
                .view()
                .expect("the image after a refused recovery settlement")
                .recoveries()
                .len(),
            1,
            "a refused settlement carried the recovery away with it"
        );
        let settled = actor
            .settle_recovered(EffectSettlement::NotStarted {
                failure: HostEffectFailure::Disconnected,
                tombstone_digest: digest(0xdd),
                retryable: false,
                retry_not_before_ms: None,
                settled_at_ms: 31,
            })
            .expect("the recovered operation settles through its own door");
        assert_eq!(settled, 2, "the settlement is one durable revision");
        assert!(
            actor
                .view()
                .expect("the image after recovery")
                .recoveries()
                .is_empty(),
            "the fence did not lift"
        );
        assert_eq!(
            actor.terminal_gone(7, 32).expect("a fact after recovery"),
            (true, 3),
            "the seat settles once the fence is down"
        );
        /* Settling again with nothing recovered is a caller that lost track. */
        assert_eq!(
            actor
                .settle_recovered(EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Disconnected,
                    tombstone_digest: digest(0xde),
                    retryable: false,
                    retry_not_before_ms: None,
                    settled_at_ms: 33,
                })
                .expect_err("a second recovery settlement"),
            RuntimeError::InvalidInput,
        );
        actor.shutdown().expect("join recovered-settlement actor");
    }

    /// Unknown team, unknown pane and a wrong capability are ONE refusal —
    /// three different answers would tell a caller which of the three it
    /// guessed — and none of them moves the ledger. The check happens under
    /// the same table borrow the plan runs in, so there is no generation in
    /// which a rotated token still plans.
    #[test]
    fn a_verb_without_authority_gets_one_answer_and_plans_nothing() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(None, 10),
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        let argv = || {
            vec![
                "run-create".to_string(),
                "--name".to_string(),
                "nightly".to_string(),
                "--retry-request".to_string(),
                "r-1".to_string(),
            ]
        };
        let caller = Some(format!("actor-v1:{}", "a".repeat(64)));
        for (team, pane, presented, wrong) in [
            ("team-9", "%1", capability_of("%1"), "a team nobody made"),
            ("team-1", "%9", capability_of("%9"), "a pane nobody cut"),
            (
                "team-1",
                "%1",
                "rotated-away".to_string(),
                "a dead capability",
            ),
        ] {
            let command = PlanCommand::checked(argv(), team, pane, presented, caller.clone(), 20)
                .expect("a command inside the door's bounds");
            assert_eq!(
                actor.plan(command).expect_err(wrong),
                RuntimeError::UnauthorizedSeat,
                "{wrong} got a different refusal"
            );
        }
        assert_eq!(
            actor.view().expect("the ledger afterwards").revision(),
            0,
            "a refused seat moved the ledger"
        );
        actor.shutdown().expect("join authority actor");
    }

    fn handover_at_stop(
        fixture: &Fixture,
    ) -> (
        RuntimeActor,
        Box<TableFixture>,
        zerocode_core::orchestration::HandoverPlan,
        Effect,
    ) {
        let legacy = a_seated_legacy();
        let run_id = legacy.runs[0].id.clone();
        let shared = a_seated_table();
        let actor = start_with(
            fixture,
            cutover(Some(legacy), 10),
            Box::new((*shared).clone()),
            Box::new(AnsweringLauncher),
        );
        for argv in [
            vec!["run-use", &run_id, "--retry-request", "use"],
            vec![
                "handover-policy",
                "--on-quota-wall",
                "claude",
                "--retry-request",
                "policy",
            ],
        ] {
            let (decided, _) = actor.plan(a_command(&argv, 20)).unwrap();
            assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        }
        actor.worker_seated("team-1", "%2", "/tmp", 21).unwrap();
        let image = actor.view().unwrap();
        let run = &image.projection().runs[0];
        let worker = &image.projection().workers[0];
        let task = &image.projection().tasks[0];
        let policy = run.handover.clone().unwrap();
        let plan = zerocode_core::orchestration::HandoverPlan {
            run: run_id,
            worker: worker.id.clone(),
            agent: worker.agent.clone(),
            dispatch: worker.dispatch.clone().unwrap(),
            task: task.id.clone(),
            spec: task.spec.clone(),
            checkout: "/tmp".to_string(),
            cause: zerocode_core::orchestration::HandoverCause::QuotaWall {
                provider: "codex".to_string(),
                used_percent: 98,
                resets_at_ms: Some(60),
            },
            to: policy.on_quota_wall.clone().unwrap(),
            wip_commit: policy.wip_commit,
            team: "team-1".to_string(),
            pane: "%1".to_string(),
            worker_team: worker.team.clone(),
            worker_pane: worker.pane.clone(),
            worker_started_ms: worker.started_ms,
            policy: Some(policy),
            coordinator_generation: run.coordinator.as_ref().unwrap().generation,
        };
        let command = a_command(
            &[
                "worker-stop",
                "--worker",
                &plan.worker,
                "--retry-request",
                "stop",
            ],
            22,
        )
        .for_handover(plan.clone(), shared.incarnation("team-1", "%2").unwrap())
        .unwrap();
        let (stopping, _) = actor.plan(command).unwrap();
        assert_eq!(stopping.reply.exit_code, 0, "{}", stopping.reply.stderr);
        (actor, shared, plan, stopping.effect)
    }

    /// Park a real mailbox request ahead of the effect. The send returns only
    /// once the actor is parked, so subsequent queued requests cannot run early.
    fn hold_effect_mailbox(actor: &RuntimeActor) -> (SyncSender<()>, Receiver<()>) {
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::sync_channel(1);
        let (finished, finished_rx) = mpsc::sync_channel(1);
        actor
            .sender
            .as_ref()
            .unwrap()
            .send(ActorCommand::Hold {
                entered,
                release: release_rx,
                finished,
            })
            .unwrap();
        entered_rx.recv().unwrap();
        (release, finished_rx)
    }

    #[test]
    fn queued_terminal_settlement_observes_recovery_and_time_only_after_the_mailbox() {
        use zerocode_core::orchestration::{
            Headroom, QuotaWallMarker, QuotaWindow, quota_wall_witness,
        };
        for recovery in ["activity", "headroom", "reset", "stale", "current"] {
            let fixture = Fixture::new();
            let (actor, _shared, plan, effect) = handover_at_stop(&fixture);
            let marker = QuotaWallMarker {
                source: "screen".into(),
                line: "usage_limit_exceeded".into(),
            };
            let evidence = Mutex::new((
                false,
                Headroom {
                    provider: "codex".into(),
                    used_percent: 98,
                    window: QuotaWindow::Session,
                    resets_at_ms: Some(60),
                    updated_at_ms: 20,
                    status: "ok".into(),
                    failure_kind: None,
                },
                30,
            ));
            assert!(
                quota_wall_witness(
                    &plan.worker,
                    Some(marker.clone()),
                    Some(&evidence.lock().unwrap().1),
                    30
                )
                .is_some()
            );
            let (release, finished) = hold_effect_mailbox(&actor);
            let Effect::WorkerTerminal {
                seat,
                incarnation: Some(incarnation),
                stop,
                handover,
                ..
            } = effect
            else {
                panic!("a stop carries its terminal");
            };
            let pending = actor
                .pending_effect(|fence| RuntimeRequest::WorkerTerminalSettled {
                    seat,
                    term: incarnation.term,
                    capability: incarnation.capability,
                    stop,
                    screen: None,
                    handover,
                    fence: Some(fence),
                    now_ms: 30,
                })
                .unwrap();
            // The final pre-mailbox probe saw a wall. The SAME terminal now
            // recovers, or its numeric witness expires, before dequeue.
            {
                let mut evidence = evidence.lock().unwrap();
                match recovery {
                    "activity" => evidence.0 = true,
                    "headroom" => evidence.1.used_percent = 20,
                    "reset" => evidence.2 = 61,
                    "stale" => {
                        evidence.1.resets_at_ms = None;
                        evidence.2 =
                            21 + zerocode_core::orchestration::QUOTA_POLICY.snapshot_max_age_ms;
                    }
                    "current" => {}
                    _ => unreachable!(),
                }
            }
            release.send(()).unwrap();
            finished.recv().unwrap();
            let mut committed = false;
            let settled = actor.observe_terminal(pending, |commit| {
                let evidence = evidence.lock().unwrap();
                if !evidence.0
                    && quota_wall_witness(&plan.worker, Some(marker), Some(&evidence.1), evidence.2)
                        .is_some()
                {
                    commit(evidence.2);
                    committed = true;
                }
            });
            let image = actor.view().unwrap();
            let dispatch = image
                .projection()
                .dispatches
                .iter()
                .find(|row| row.id == plan.dispatch)
                .unwrap();
            if recovery == "current" {
                assert_eq!(settled.unwrap().0, WorkerState::Released);
                assert!(committed);
                assert_eq!(dispatch.ended_ms, Some(30));
            } else {
                assert_eq!(settled, Err(RuntimeError::AuthorityRejected), "{recovery}");
                assert!(!committed, "{recovery}");
                assert_eq!(dispatch.ended_ms, None, "{recovery}");
                assert_eq!(
                    image.projection().workers[0].state,
                    WorkerState::Active,
                    "{recovery}"
                );
            }
            actor.shutdown().unwrap();
        }
    }

    #[test]
    fn a_policy_change_queued_ahead_of_prepare_effect_cannot_authorize_the_old_split() {
        let fixture = Fixture::new();
        let (actor, shared, plan, effect) = handover_at_stop(&fixture);
        actor
            .worker_terminal_settled_fenced(effect, None, 30, |commit| commit(30))
            .unwrap();
        shared.with_team("team-1", &mut |team| {
            team.unwrap().remove_pane("%2");
        });
        let command = a_command(
            &[
                "worker-start",
                "--agent",
                "claude",
                "--task",
                &plan.task,
                "--retry-of",
                &plan.dispatch,
                "--inherit-checkout",
                "--retry-request",
                "start",
            ],
            31,
        )
        .for_handover(
            plan,
            zerocode_core::agent_teams::PaneIncarnation {
                term: 7,
                capability: capability_of("%2"),
            },
        )
        .unwrap();
        let (decided, _) = actor.plan(command).unwrap();
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        let prepared = decided.prepared_worker_start.unwrap();
        let (release, finished) = hold_effect_mailbox(&actor);
        let (policy_reply, policy_answer) = mpsc::sync_channel(1);
        actor
            .sender
            .as_ref()
            .unwrap()
            .send(ActorCommand::Request {
                request: Box::new(RuntimeRequest::Plan(a_command(
                    &["handover-policy", "--off", "--retry-request", "off"],
                    32,
                ))),
                reply: policy_reply,
            })
            .unwrap();
        let (effect_reply, effect_answer) = mpsc::sync_channel(1);
        actor
            .sender
            .as_ref()
            .unwrap()
            .send(ActorCommand::Request {
                request: Box::new(RuntimeRequest::PrepareEffect {
                    request: EffectRequest::new(
                        "main-ledger",
                        digest(0x31),
                        digest(0x41),
                        digest(0x51),
                        digest(0x61),
                        HostEffectKind::Split,
                    )
                    .unwrap(),
                    now_ms: 31,
                }),
                reply: effect_reply,
            })
            .unwrap();
        release.send(()).unwrap();
        finished.recv().unwrap();
        let RuntimeReply::Planned { decided, .. } = policy_answer.recv().unwrap().unwrap() else {
            panic!("policy reply");
        };
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        let RuntimeReply::Effect {
            begun: BeginEffect::Execute(permit),
            ..
        } = effect_answer.recv().unwrap().unwrap()
        else {
            panic!("the queued effect reserves after the policy changed");
        };
        let mut spawned = false;
        assert_eq!(
            actor.handover_split(
                prepared.clone(),
                permit.operation_id().to_string(),
                33,
                || {
                    spawned = true;
                }
            ),
            Err(RuntimeError::AuthorityRejected)
        );
        assert!(!spawned);
        actor
            .settle_effect(
                permit,
                EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Refused,
                    tombstone_digest: digest(0x71),
                    retryable: false,
                    retry_not_before_ms: None,
                    settled_at_ms: 34,
                },
            )
            .unwrap();
        actor
            .seat_never_opened(prepared.worker.clone(), 35)
            .unwrap();
        let image = actor.view().unwrap();
        assert!(
            !image
                .projection()
                .workers
                .iter()
                .any(|worker| worker.id == prepared.worker)
        );
        assert_eq!(
            image.projection().tasks[0].status,
            zerocode_core::orchestration::TaskStatus::Ready
        );
        actor.shutdown().unwrap();
    }

    /// A legacy ledger whose one run is over: task completed, attempt
    /// closed, terminal let go.
    ///
    /// Built through the ledger's own doors and then aged by editing the
    /// PROJECTION, whose rows are the file's rows: reaching this shape by
    /// replaying verbs would need a pane table, a launcher and a worker that
    /// reports, none of which is what these tests are about.
    fn a_finished_legacy() -> LedgerProjectionV1 {
        use zerocode_core::orchestration::TaskStatus;

        let mut legacy = Ledger::new();
        let run = legacy.create_run("nightly", 5);
        let task = legacy
            .create_task(
                &run,
                "carry the wall".to_string(),
                "carry the wall".to_string(),
                Vec::new(),
                None,
                5,
            )
            .expect("a task");
        legacy
            .start_worker(&run, "codex", ("team-1", "%2"), Some(&task), 6)
            .expect("a seated worker");
        let mut held = legacy.export();
        for row in &mut held.tasks {
            row.status = TaskStatus::Completed;
        }
        for row in &mut held.dispatches {
            row.ended_ms = Some(7);
            row.succeeded = Some(true);
        }
        for row in &mut held.workers {
            row.state = WorkerState::Released;
            /* A worker whose attempt has ended does not still name it —
             * `validate_loaded` refuses that shape, because every road that
             * asks whether a seat is free reads a named dispatch as "no". */
            row.dispatch = None;
        }
        held
    }

    fn an_unattempted_dispatched_legacy() -> LedgerProjectionV1 {
        use zerocode_core::orchestration::TaskStatus;

        let mut ledger = Ledger::new();
        let run = ledger.create_run("repair", 1);
        let task = ledger
            .create_task(&run, "work".into(), "work".into(), Vec::new(), None, 2)
            .expect("task");
        ledger
            .update_task(
                &run,
                &task,
                None,
                Some(r#"{"note":"original","ok":false}"#.into()),
                zerocode_core::orchestration::ResultAuthor::Ledger,
            )
            .expect("prior result");
        let other = ledger
            .create_task(&run, "other".into(), "other".into(), Vec::new(), None, 3)
            .expect("unrelated task");
        ledger
            .start_worker(&run, "codex", ("team-1", "%2"), Some(&other), 4)
            .expect("an unrelated live attempt");
        let mut projection = ledger.export();
        // Reproduce a historical verb's output, without requiring the fixed
        // live verb to accept it or touching any real authority store.
        projection.tasks[0].status = TaskStatus::Dispatched;
        projection
    }

    #[test]
    fn boot_repairs_only_a_dispatched_task_without_an_attempt_once() {
        use zerocode_core::orchestration::TaskStatus;

        for dependent in [false, true] {
            let fixture = Fixture::new();
            let mut projection = an_unattempted_dispatched_legacy();
            let expected_status = if dependent {
                let dependency = projection.tasks[1].id.clone();
                projection.tasks[0].deps.push(dependency);
                TaskStatus::Pending
            } else {
                TaskStatus::Ready
            };
            let connection = fixture.store.connection().expect("private store");
            ledger_store::write(&connection, "main-ledger", 0, 7, &projection, 4)
                .expect("historical generation");
            let held = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
                .expect("SQLite and digests are valid")
                .expect("stored ledger");
            assert!(held.bytes_match);
            assert!(Ledger::rebuild(held.projection).is_err());
            drop(connection);

            let actor = start_with(
                &fixture,
                RuntimeBoot::Reopen,
                a_seated_table(),
                Box::new(NoLauncher),
            );
            let repaired = actor.view().expect("repaired boot");
            assert_eq!(repaired.revision(), 8);
            assert_eq!(repaired.repairs().len(), 1);
            assert!(repaired.repairs()[0].contains(&projection.tasks[0].id));
            assert!(!repaired.repairs()[0].contains("original"));
            let row = &repaired.projection().tasks[0];
            assert_eq!(row.status, expected_status);
            let result: serde_json::Value =
                serde_json::from_str(row.result.as_str()).expect("JSON result");
            assert_eq!(result["ok"], false);
            assert!(result["note"].as_str().expect("note").contains("original"));
            assert!(row.result.contains("dispatched"));
            assert!(row.result.contains("attempt"));
            projection.tasks[0].status = expected_status;
            projection.tasks[0].result = row.result.clone();
            assert_eq!(*repaired.projection(), projection, "other rows changed");
            actor.shutdown().expect("join repaired actor");

            let connection = fixture.store.connection().expect("private store");
            let held = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
                .expect("strict read after repair")
                .expect("stored ledger");
            Ledger::rebuild(held.projection).expect("repaired generation rebuilds");
            drop(connection);
            let actor = start_with(
                &fixture,
                RuntimeBoot::Reopen,
                a_seated_table(),
                Box::new(NoLauncher),
            );
            let reopened = actor.view().expect("second boot");
            assert_eq!(reopened.revision(), repaired.revision());
            assert_eq!(reopened.projection(), repaired.projection());
            assert!(reopened.repairs().is_empty());
            actor.shutdown().expect("join reopened actor");
        }
    }

    #[test]
    fn boot_does_not_publish_a_repair_when_another_content_invariant_fails() {
        use zerocode_core::orchestration::TaskStatus;

        let fixture = Fixture::new();
        let mut projection = an_unattempted_dispatched_legacy();
        projection.tasks[1].status = TaskStatus::Ready;
        let connection = fixture.store.connection().unwrap();
        ledger_store::write(&connection, "main-ledger", 0, 7, &projection, 7).unwrap();
        let error = RuntimeActor::start(
            &fixture.store,
            "main-ledger",
            RuntimeBoot::Reopen,
            4,
            a_seated_table(),
            Box::new(NoLauncher),
        )
        .expect_err("a second violation still prevents boot");
        assert!(matches!(error, RuntimeError::LedgerInvariant(_)));
        assert!(error.to_string().contains(&projection.tasks[1].id));
        assert!(error.fatal());
        let after = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
            .unwrap()
            .unwrap();
        assert_eq!(after.revision, 7);
        assert_eq!(after.projection, projection);
    }

    #[test]
    fn boot_repair_does_not_waive_byte_counts_or_table_digests() {
        for damage in [
            "UPDATE orchestration_ledger_heads SET bytes_held = bytes_held + 1",
            "UPDATE ledger_tasks SET result = 'tampered' WHERE ordinal = 0",
        ] {
            let fixture = Fixture::new();
            let projection = an_unattempted_dispatched_legacy();
            let connection = fixture.store.connection().expect("private store");
            ledger_store::write(&connection, "main-ledger", 0, 7, &projection, 7)
                .expect("historical generation");
            connection
                .execute(damage, [])
                .expect("private corruption fixture");
            let error = RuntimeActor::start(
                &fixture.store,
                "main-ledger",
                RuntimeBoot::Reopen,
                4,
                a_seated_table(),
                Box::new(NoLauncher),
            )
            .expect_err("byte corruption is not a task repair");
            assert_eq!(error, RuntimeError::StoreCorrupt, "{damage}");
            assert_eq!(
                ledger_store::head_revision(&connection, "main-ledger").unwrap(),
                Some(7)
            );
        }
    }

    #[test]
    fn boot_repair_preserves_pending_gates_and_closed_attempt_history() {
        use zerocode_core::orchestration::TaskStatus;

        for gated in [false, true] {
            let fixture = Fixture::new();
            let mut projection = if gated {
                let mut held = an_unattempted_dispatched_legacy();
                held.tasks[0].status = TaskStatus::Ready;
                let run = held.tasks[0].run.clone();
                let task = held.tasks[0].id.clone();
                let mut ledger = Ledger::rebuild(held).expect("valid fixture before gate");
                ledger
                    .create_gate(&run, &task, "decision".into(), Vec::new(), 6)
                    .expect("pending gate");
                ledger.export()
            } else {
                a_finished_legacy()
            };
            projection.tasks[0].status = TaskStatus::Dispatched;
            let connection = fixture.store.connection().expect("private store");
            ledger_store::write(&connection, "main-ledger", 0, 7, &projection, 7)
                .expect("historical generation");
            RuntimeActor::start(
                &fixture.store,
                "main-ledger",
                RuntimeBoot::Reopen,
                4,
                a_seated_table(),
                Box::new(NoLauncher),
            )
            .expect_err("a different contradiction needs its own decision");
            let after = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
                .expect("valid bytes remain")
                .expect("ledger");
            assert_eq!(after.revision, 7);
            assert_eq!(after.projection, projection);
        }
    }

    #[test]
    fn boot_repair_cannot_advance_past_an_unsettled_host_effect() {
        let fixture = Fixture::new();
        let projection = an_unattempted_dispatched_legacy();
        let journal = EffectJournal::new(&fixture.store);
        let lease = journal
            .claim_authority("main-ledger", &digest(0xf5), 1)
            .unwrap();
        journal
            .import_legacy(&lease, &Ledger::new().export(), &digest(0xaa), 2)
            .unwrap();
        let request = EffectRequest::new(
            "main-ledger",
            digest(0x11),
            digest(0x21),
            digest(0xb1),
            digest(0xe1),
            HostEffectKind::Split,
        )
        .unwrap();
        let BeginEffect::Execute(permit) = journal
            .prepare(&lease, 0, &projection, &request, 7)
            .unwrap()
        else {
            panic!("fixture effect was not prepared");
        };
        permit.abandon("leave an unfinished fixture effect for boot");
        drop(lease);
        let connection = fixture.store.connection().unwrap();
        let before: String = connection
            .query_row(
                "SELECT effect_state FROM host_effect_operations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let error = RuntimeActor::start(
            &fixture.store,
            "main-ledger",
            RuntimeBoot::Reopen,
            4,
            a_seated_table(),
            Box::new(NoLauncher),
        )
        .expect_err("repair must respect the effect fence");
        assert_eq!(error, RuntimeError::EffectInFlight);
        let after = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
            .unwrap()
            .unwrap();
        assert_eq!(after.revision, 1);
        assert_eq!(after.projection, projection);
        let state: String = connection
            .query_row(
                "SELECT effect_state FROM host_effect_operations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, before);
    }

    #[test]
    fn boot_names_an_unrepairable_content_invariant_apart_from_store_corruption() {
        use zerocode_core::orchestration::TaskStatus;

        let fixture = Fixture::new();
        let mut projection = a_seated_legacy();
        projection.tasks[0].status = TaskStatus::Ready;
        let connection = fixture.store.connection().expect("private store");
        ledger_store::write(&connection, "main-ledger", 0, 7, &projection, 7)
            .expect("valid bytes, invalid content");
        let before = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
            .expect("valid digests")
            .expect("ledger");
        let expected = Ledger::rebuild(before.projection.clone()).expect_err("invalid content");
        drop(connection);
        let error = match RuntimeActor::start(
            &fixture.store,
            "main-ledger",
            RuntimeBoot::Reopen,
            4,
            a_seated_table(),
            Box::new(NoLauncher),
        ) {
            Ok(actor) => {
                let _ = actor.shutdown();
                panic!("unrelated invariant was repaired");
            }
            Err(error) => error,
        };
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("invariant"), "{diagnostic}");
        assert!(diagnostic.contains(&expected.to_string()), "{diagnostic}");
        assert_ne!(diagnostic, RuntimeError::StoreCorrupt.to_string());
        let connection = fixture.store.connection().expect("private store");
        let after = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
            .expect("still readable")
            .expect("still present");
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.projection, before.projection);
    }

    /// An unstamped check receipt whose delivery rows disappeared becomes a
    /// tombstone, while its retry key remains spent. The stale derived byte
    /// count is rewritten in the same canonical generation before the actor
    /// answers anything.
    #[test]
    fn boot_tombstones_an_unverifiable_legacy_receipt_and_repairs_its_derived_count() {
        use zerocode_core::orchestration::{
            CheckV1, InboxRow, MessageKind, MessageRow, Priority, RunRow, ServedAnswer, ServedRow,
            Text,
        };

        let fixture = Fixture::new();
        let mut projection = Ledger::new().export();
        projection.next_id = 4;
        projection.runs.push(RunRow {
            id: "run-1".to_string(),
            name: Text::from("legacy".to_string()),
            created_ms: 1,
            auto: None,
            summary: None,
            coordinator: None,
            handover: None,
        });
        projection.messages.push(MessageRow {
            run: "run-1".to_string(),
            id: "m-2".to_string(),
            from: "worker".to_string(),
            to: "run:run-1".to_string(),
            kind: MessageKind::Status,
            body: Text::from("done".to_string()),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: Text::default(),
            thread: None,
            task: None,
            dispatch: None,
            author_seat: None,
            created_ms: 2,
        });
        projection.inboxes.push(InboxRow {
            run: "run-1".to_string(),
            address: "run:run-1".to_string(),
            pending: Vec::new(),
            open: None,
        });
        projection.served.push(ServedRow {
            caller: Some(Text::from(format!("actor-v1:{}", "a".repeat(64)))),
            request: Text::from("legacy-look".to_string()),
            answer: ServedAnswer::Check(CheckV1 {
                run: "run-1".to_string(),
                address: "run:run-1".to_string(),
                delivery: Some("d-3".to_string()),
                messages: vec!["m-2".to_string()],
            }),
            fingerprint: Some(Text::from("f".repeat(64))),
            filed_ms: None,
            expired: false,
        });

        let connection = fixture.store.connection().expect("the authority store");
        ledger_store::write(&connection, "main-ledger", 0, 7, &projection, 3)
            .expect("the historical generation");
        connection
            .execute(
                "UPDATE orchestration_ledger_heads
                    SET bytes_held = bytes_held + 1
                  WHERE ledger_id = 'main-ledger'",
                [],
            )
            .expect("the stale derived count");
        assert!(
            matches!(
                ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA),
                Err(EffectJournalError::Corrupt)
            ),
            "the ordinary reader accepted the wound"
        );
        drop(connection);

        let actor = start_with(
            &fixture,
            cutover(None, 4),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let image = actor.view().expect("the repaired ledger");
        assert_eq!(
            image.revision(),
            8,
            "the repair was not one durable generation"
        );
        let receipt = &image.projection().served[0];
        assert!(receipt.expired, "the unverifiable answer is still live");
        assert!(matches!(&receipt.answer, ServedAnswer::Inline(held) if held.is_empty()));
        actor.shutdown().expect("join the repaired actor");

        let connection = fixture.store.connection().expect("the repaired store");
        let held = ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
            .expect("strict read after repair")
            .expect("the ledger remains");
        assert!(held.bytes_match);
        Ledger::rebuild(held.projection).expect("the canonical generation rebuilds");
    }

    /// The beat's retention door: it asks at most once an hour, it spends a
    /// write only when something actually moved, and what it gave up is on
    /// the disk before the answer goes out.
    #[test]
    fn a_retention_sweep_is_rate_limited_durable_and_silent_when_nothing_moved() {
        let fixture = Fixture::new();
        let legacy = a_finished_legacy();
        let counter = legacy.next_id;
        Ledger::rebuild(legacy.clone()).expect("the fixture has to be a legal ledger");
        let actor = start_with(
            &fixture,
            cutover(Some(legacy), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(actor.view().expect("the imported image").revision(), 0);

        // Too soon after the last sweep, and nothing is even looked at.
        let (swept, revision) = actor.retention_sweep(1_000).expect("an early sweep");
        assert_eq!(swept, Sweep::default(), "a sweep ran before it was due");
        assert_eq!(revision, 0, "an early sweep spent a write");

        // Due, but the run is younger than the policy: nothing moves, and the
        // revision does not move for it either — a beat must not wake every
        // reader once an hour to say it found nothing.
        let (swept, revision) = actor
            .retention_sweep(zerocode_core::orchestration::SWEEP_INTERVAL_MS)
            .expect("a due sweep");
        assert_eq!(swept.runs, 0);
        assert_eq!(revision, 0, "a sweep that gave up nothing spent a write");

        // Past the policy, and the run's rows go — durably.
        let aged = 40 * 86_400_000_i64;
        let (swept, revision) = actor.retention_sweep(aged).expect("the sweep that bites");
        assert_eq!(swept.runs, 1, "the finished run was spared: {swept:?}");
        assert_eq!(revision, 1, "the sweep did not reach the disk");

        actor.shutdown().expect("join the sweeping actor");
        let reopened = start_with(
            &fixture,
            cutover(None, aged + 1),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let image = reopened.view().expect("the image after a restart");
        let held = image.projection();
        assert_eq!(held.runs.len(), 1, "the restart lost the run row itself");
        assert!(
            held.runs[0].summary.is_some(),
            "the summary did not survive the restart"
        );
        assert!(held.tasks.is_empty(), "a detail row survived the restart");
        assert_eq!(
            held.next_id, counter,
            "the counter moved across a sweep and a restart"
        );
        assert_eq!(
            held.retention_days,
            zerocode_core::orchestration::RETENTION_DEFAULT_DAYS,
            "a ledger with no policy came back with something other than the default"
        );
        assert!(
            held.swept_at_ms >= aged,
            "the sweep's stamp did not persist"
        );
        reopened.shutdown().expect("join the reopened actor");
    }

    /// The fourth host fact: the window itself restarted, so every worker
    /// the last one had settles at once — durably, and exactly once.
    ///
    /// Without this door the actor's cutover boot rebuilt the rows and left
    /// every old dispatch open, counting against a standing order's ceiling
    /// forever — the exact bug `window_restarted` was built for, reintroduced
    /// by the cutover that forgot to give it a door.
    #[test]
    fn a_restarted_window_sweeps_every_seat_at_once() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        let (swept, revision) = actor.window_restarted(20).expect("the sweep");
        assert_eq!(
            (swept.ended, swept.sleeping, swept.moved, revision),
            (1, 0, true, 1),
            "the seated worker settles, durably"
        );
        let image = actor.view().expect("the swept image");
        assert!(
            image
                .projection()
                .workers
                .iter()
                .all(|worker| !worker.state.is_live()),
            "a worker survived the restart with its pane gone"
        );
        assert!(
            image
                .projection()
                .dispatches
                .iter()
                .all(|dispatch| dispatch.ended_ms.is_some()),
            "a dispatch stayed open across the restart"
        );
        // Sweeping the already-swept says so without moving anything.
        assert_eq!(
            actor.window_restarted(30).expect("the second sweep"),
            (zerocode_core::orchestration::Restarted::default(), 1),
            "an empty sweep moved the ledger"
        );
        actor.shutdown().expect("join swept actor");
    }

    /// The seatable half of the restart fork must survive the real SQLite
    /// store, not merely the actor's in-memory image. The next process rebuilds
    /// the projection before it can answer any command.
    #[test]
    fn a_sleeping_attempt_reopens_from_the_authority_store() {
        let fixture = Fixture::new();
        let mut legacy = a_seated_legacy();
        legacy.workers[0].checkout = Some("/wt/nightly".to_string());
        let worker = legacy.workers[0].id.clone();
        let dispatch = legacy.workers[0]
            .dispatch
            .clone()
            .expect("the worker carries an attempt");
        let actor = start_with(
            &fixture,
            cutover(Some(legacy), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let (swept, revision) = actor.window_restarted(20).expect("the sweep");
        assert_eq!((swept.ended, swept.sleeping, revision), (0, 1, 1));
        actor.shutdown().expect("join sleeping actor");

        let reopened = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        let image = reopened.view().expect("the reopened image");
        let projection = image.projection();
        let kept = projection
            .workers
            .iter()
            .find(|held| held.id == worker)
            .expect("the sleeping worker was stored");
        assert_eq!(kept.state, WorkerState::Sleeping);
        let open = projection
            .dispatches
            .iter()
            .find(|held| held.id == dispatch)
            .expect("the dispatch was stored");
        assert!(open.ended_ms.is_none());
        reopened.shutdown().expect("join reopened actor");
    }

    /// A provider session is a restart fact, so the actor test crosses the
    /// actual SQLite authority store and starts a second actor. An in-memory
    /// image alone would not catch a projection that strict rebuild rejects.
    #[test]
    fn a_worker_session_reopens_from_the_authority_store() {
        let fixture = Fixture::new();
        let legacy = a_seated_legacy();
        let worker = legacy.workers[0].id.clone();
        let session = ProviderSession {
            key: zerocode_core::provider_session::SessionKey::SessionId,
            id: "0199a-session".to_string(),
            transcript_path: Some("/transcripts/0199a.jsonl".to_string()),
        };
        let actor = start_with(
            &fixture,
            cutover(Some(legacy), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor
                .worker_session_reported(7, session.clone(), 20)
                .expect("the pane reports its session"),
            (true, 1)
        );
        actor.shutdown().expect("join session actor");

        let reopened = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            a_seated_table(),
            Box::new(NoLauncher),
        );
        let image = reopened.view().expect("the reopened image");
        let carried = image
            .projection()
            .workers
            .iter()
            .find(|held| held.id == worker)
            .and_then(|held| held.session.as_ref())
            .expect("the stored worker has its conversation");
        assert_eq!(carried, &session);
        reopened.shutdown().expect("join reopened actor");
    }

    /// Stage one and stage two meet in this exact durable shape: an open
    /// dispatch held by a sleeping worker that also carries the provider
    /// session needed for reseating. Crossing the real SQLite store in one
    /// test prevents either isolated round-trip from hiding a combined
    /// `validate_loaded` refusal on the next window.
    #[test]
    fn a_sleeping_open_attempt_reopens_with_its_provider_session() {
        let fixture = Fixture::new();
        let mut legacy = a_seated_legacy();
        legacy.workers[0].checkout = Some("/wt/session-survival".to_string());
        let worker = legacy.workers[0].id.clone();
        let dispatch = legacy.workers[0]
            .dispatch
            .clone()
            .expect("the worker carries an attempt");
        let task = legacy.dispatches[0].task.clone();
        let session = ProviderSession {
            key: zerocode_core::provider_session::SessionKey::SessionId,
            id: "0199a-sleeping-session".to_string(),
            transcript_path: Some("/transcripts/sleeping.jsonl".to_string()),
        };

        let actor = start_with(
            &fixture,
            cutover(Some(legacy), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor
                .worker_session_reported(7, session.clone(), 20)
                .expect("write the provider session"),
            (true, 1)
        );
        let (swept, revision) = actor.window_restarted(30).expect("sleep the attempt");
        assert_eq!((swept.ended, swept.sleeping, revision), (0, 1, 2));
        actor.shutdown().expect("join sleeping actor");

        let reopened = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        let image = reopened.view().expect("the combined shape reopens");
        let projection = image.projection();
        let kept = projection
            .workers
            .iter()
            .find(|held| held.id == worker)
            .expect("the sleeping worker stands");
        assert_eq!(kept.state, WorkerState::Sleeping);
        assert_eq!(kept.session.as_ref(), Some(&session));
        assert!(
            projection
                .dispatches
                .iter()
                .find(|held| held.id == dispatch)
                .is_some_and(|held| held.ended_ms.is_none())
        );
        let kept_task = projection
            .tasks
            .iter()
            .find(|held| held.id == task)
            .expect("the dispatched task stands");
        assert_eq!(
            kept_task.status,
            zerocode_core::orchestration::TaskStatus::Dispatched
        );
        assert_eq!(kept_task.failures, 0);
        reopened.shutdown().expect("join reopened combined actor");
    }

    fn assert_reseat_refused_for_native_leader(worker_history: bool) {
        let fixture = Fixture::new();
        let mut ledger = Ledger::rebuild(a_seated_legacy()).expect("the fixture ledger");
        let run = ledger.runs()[0].id.clone();
        let worker = ledger.runs()[0].workers[0].id.clone();
        assert!(ledger.worker_seated(("team-1", "%2"), "/wt/reseat-authority"));
        if worker_history {
            // A historical worker may now look like a native leader, but
            // its row still prevents it from becoming a coordinator.
            ledger
                .start_worker(&run, "codex", ("team-other", "%1"), None, 7)
                .expect("the leader's worker history");
        }
        assert_eq!(ledger.window_restarted(8).sleeping, 1);
        if !worker_history {
            ledger
                .seat_coordinator(&run, "team-1/%1", None, 9)
                .expect("the legitimate coordinator");
        }
        let table = TableFixture::holding(Team::new("team-other", "token", 70));
        let native = table.clone();
        let actor = start_with(
            &fixture,
            cutover(Some(ledger.export()), 10),
            table,
            Box::new(AnsweringLauncher),
        );
        let (bound, _) = actor
            .plan(
                PlanCommand::checked(
                    vec![
                        "run-use".into(),
                        run.clone(),
                        "--retry-request".into(),
                        "bind-reseat".into(),
                    ],
                    "team-other",
                    "%1",
                    capability_of("%1"),
                    Some(format!("actor-v1:{}", "b".repeat(64))),
                    11,
                )
                .expect("the other leader's command"),
            )
            .expect("a binding is allowed");
        assert_eq!(bound.reply.exit_code, 0, "{}", bound.reply.stderr);
        let bound: serde_json::Value = serde_json::from_str(&bound.reply.stdout).unwrap();
        assert_eq!(bound["seated"], false, "{bound}");
        assert!(
            !actor
                .coordinator_returned(&run, "team-other", "%1", None, 12)
                .expect("the native return witness")
                .0
        );
        let before = actor.view().expect("before refusal");
        let next_pane = native.teams.lock().unwrap()["team-other"].clone();

        let planned = actor
            .prepare_worker_reseat(&run, &worker, "team-other", "%1", 70, "continue")
            .expect("a valid native leader request");
        assert!(
            planned.is_err(),
            "a refused coordinator planned a split (worker history: {worker_history})"
        );
        let after = actor.view().expect("after refusal");
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.projection(), before.projection());
        assert_eq!(
            native.teams.lock().unwrap()["team-other"],
            next_pane,
            "refusal allocated a pane"
        );
        actor.shutdown().expect("join private actor");
    }

    #[test]
    fn a_native_leader_cannot_plan_a_reseat_after_the_coordinator_refuses_it() {
        assert_reseat_refused_for_native_leader(false);
    }

    #[test]
    fn a_former_worker_cannot_plan_a_reseat_after_the_coordinator_refuses_it() {
        assert_reseat_refused_for_native_leader(true);
    }

    /// The durable reseat is not merely an in-memory routing update. It crosses
    /// the real authority SQLite store and reopens with the same attempt under
    /// the replacement team's pane.
    #[test]
    fn restored_workers_rejoin_the_coordinators_new_team() {
        let fixture = Fixture::new();
        let mut legacy = Ledger::new();
        let run = legacy.create_run("durable reseat", 5);
        let task = legacy
            .create_task(
                &run,
                "carry on".to_string(),
                String::new(),
                Vec::new(),
                None,
                6,
            )
            .expect("a task");
        let worker = legacy
            .start_worker(&run, "codex", ("team-old", "%2"), Some(&task), 7)
            .expect("a worker")
            .worker;
        assert!(legacy.worker_seated(("team-old", "%2"), "/wt/durable-reseat"));
        assert_eq!(legacy.window_restarted(8).sleeping, 1);
        let dispatch = legacy
            .run(&run)
            .and_then(|held| held.worker(&worker))
            .and_then(|held| held.dispatch.clone())
            .expect("the open dispatch");

        let new_table = || {
            let mut team = Team::new("team-new", "new-capability", 70);
            team.record_split(
                "%2",
                71,
                "%1",
                zerocode_core::agent_teams::Direction::Vertical,
            );
            TableFixture::holding(team)
        };
        let actor = start_with(
            &fixture,
            cutover(Some(legacy.export()), 10),
            new_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor
                .worker_reseated(&worker, "team-new", "%2", 20)
                .expect("the durable reseat"),
            1
        );
        actor.shutdown().expect("join reseated actor");

        let reopened = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            new_table(),
            Box::new(NoLauncher),
        );
        let image = reopened.view().expect("the reopened reseat");
        let projection = image.projection();
        let kept = projection
            .workers
            .iter()
            .find(|held| held.id == worker)
            .expect("the worker remains");
        assert_eq!(kept.state, WorkerState::Active);
        assert_eq!((kept.team.as_str(), kept.pane.as_str()), ("team-new", "%2"));
        assert_eq!(kept.dispatch.as_deref(), Some(dispatch.as_str()));
        let kept_task = projection
            .tasks
            .iter()
            .find(|held| held.id == task)
            .expect("the task remains");
        assert_eq!(
            kept_task.status,
            zerocode_core::orchestration::TaskStatus::Dispatched
        );
        assert_eq!(kept_task.failures, 0);
        assert!(
            projection
                .bound
                .iter()
                .any(|held| { held.caller.as_str() == "team-new/%2" && held.run == run })
        );
        assert!(
            !projection
                .bound
                .iter()
                .any(|held| held.caller.as_str() == "team-old/%2")
        );
        reopened.shutdown().expect("join reopened reseat actor");
    }

    /// `moved` is not derivable from the worker counts. With no live worker,
    /// release convergence and auto stand-down must still spend one durable
    /// revision, including the status mail generated by the stand-down.
    #[test]
    fn restart_convergence_without_live_workers_is_durable() {
        let fixture = Fixture::new();
        let mut ledger = Ledger::new();
        let run = ledger.create_run("converge", 5);
        ledger
            .start_worker(&run, "codex", ("team-1", "%2"), None, 6)
            .expect("a worker");
        let mut legacy = ledger.export();
        legacy.workers[0].state = WorkerState::ReleasePending;
        legacy.runs[0].auto = Some(Auto {
            max: 1,
            agent: "codex".to_string(),
            team: "team-1".to_string(),
            pane: "%1".to_string(),
            armed_ms: 7,
        });
        let actor = start_with(
            &fixture,
            cutover(Some(legacy), 10),
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );

        let (swept, revision) = actor.window_restarted(20).expect("the sweep");
        assert_eq!((swept.ended, swept.sleeping, swept.moved), (0, 0, true));
        assert_eq!(revision, 1);
        actor.shutdown().expect("join converged actor");

        let reopened = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        let projection = reopened.view().expect("the reopened image");
        assert_eq!(projection.revision(), 1);
        assert_eq!(
            projection.projection().workers[0].state,
            WorkerState::Released
        );
        assert!(projection.projection().runs[0].auto.is_none());
        assert_eq!(projection.projection().messages.len(), 1);
        reopened.shutdown().expect("join reopened actor");
    }

    /// A leader's death ends its whole team: the workers settle and the
    /// standing orders go down, through a door — and a team the ledger never
    /// heard of settles nothing.
    #[test]
    fn a_dissolved_team_puts_down_what_its_leader_watched() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert_eq!(
            actor.team_dissolved("team-9", 20).expect("a stranger team"),
            (false, 0),
            "a team with nothing here moved the ledger"
        );
        assert_eq!(
            actor.team_dissolved("team-1", 21).expect("the dissolution"),
            (true, 1),
            "the team's worker settles, durably"
        );
        /* Settled as KEPT (t-2512): the worker was carrying a task, so the
         * leader's exit orphans it rather than ending it — the attempt stays
         * open for the child's own report or the next coordinator's adoption,
         * and that is what reached the disk. */
        let image = actor.view().expect("the dissolved image");
        let worker = &image.projection().workers[0];
        assert_eq!(
            worker.state,
            WorkerState::Orphaned,
            "the dissolved team's carrying worker lost its attempt to its leader's exit"
        );
        assert!(
            worker.dispatch.is_some(),
            "the orphan's dispatch closed under it"
        );
        assert!(
            image.projection().runs[0].auto.is_none(),
            "the standing order kept standing on a seat that is gone"
        );
        actor.shutdown().expect("join dissolved actor");
    }

    /// The wait road, in the actor's grammar: the first look leaves a
    /// `waiting`, mail arrives, and the woken look takes the delivery AND
    /// remembers the answer in one transition — so the same retry name
    /// replays that answer instead of running again.
    #[test]
    fn a_wait_that_wakes_takes_the_answer_and_its_receipt_in_one_transition() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(None, 10),
            a_seated_table(),
            Box::new(AnsweringLauncher),
        );
        actor
            .plan(a_command(
                &["run-create", "--name", "nightly", "--retry-request", "r-1"],
                20,
            ))
            .expect("a run");
        let (asked, after_ask) = actor
            .plan(a_command(
                &["check", "--wait", "--retry-request", "r-w"],
                21,
            ))
            .expect("the first look");
        let waiting = asked.waiting.clone().expect("an empty inbox leaves a wait");
        let receipt = asked.receipt.clone().expect("a named retry has a key");
        /* Nothing has arrived: the woken look says so and writes nothing. */
        let (found, unchanged) = actor
            .look_again(waiting.clone(), Some(receipt.clone()), 22)
            .expect("a look at a still-empty inbox");
        assert!(found.is_none(), "an empty inbox answered");
        assert_eq!(unchanged, after_ask, "an empty look moved the ledger");
        actor
            .plan(
                PlanCommand::checked(
                    [
                        "send",
                        "--run",
                        "run-1",
                        "--to",
                        "run:run-1",
                        "--type",
                        "status",
                        "--body",
                        "the-wall-stands",
                        "--retry-request",
                        "r-s",
                    ]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                    "team-1",
                    "%2",
                    capability_of("%2"),
                    Some(format!("actor-v1:{}", "b".repeat(64))),
                    23,
                )
                .expect("peer mail inside the door's bounds"),
            )
            .expect("mail into the waited-on inbox");
        let (found, moved) = actor
            .look_again(waiting, Some(receipt), 24)
            .expect("the woken look");
        let found = found.expect("the delivery this wait was sleeping for");
        assert_eq!(found.reply.exit_code, 0, "{}", found.reply.stderr);
        assert!(
            found.reply.stdout.contains("the-wall-stands"),
            "the woken look answered something else: {}",
            found.reply.stdout
        );
        assert!(
            moved > unchanged,
            "a taken delivery did not move the ledger"
        );
        /* The same retry name now REPLAYS — the receipt was remembered in
         * the same transition as the delivery, so there is no window in
         * which the answer exists and the record of it does not. */
        let (replayed, after_replay) = actor
            .plan(a_command(
                &["check", "--wait", "--retry-request", "r-w"],
                25,
            ))
            .expect("the replay");
        assert_eq!(replayed.reply.exit_code, 0, "{}", replayed.reply.stderr);
        assert!(
            replayed.reply.stdout.contains("the-wall-stands"),
            "the replay answered something else: {}",
            replayed.reply.stdout
        );
        assert!(
            replayed.waiting.is_none(),
            "a replayed answer put the caller back to sleep"
        );
        assert_eq!(
            after_replay,
            moved + 1,
            "a replay re-proves the disk still agrees — exactly one revision"
        );
        actor.shutdown().expect("join waiting actor");
    }

    /// The timeout half of the wait road: the empty answer the first look
    /// already wrote is served as the receipt once the wait gives up, and
    /// the same retry name replays it.
    #[test]
    fn a_wait_that_times_out_serves_its_own_empty_answer() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(None, 10),
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        actor
            .plan(a_command(
                &["run-create", "--name", "nightly", "--retry-request", "r-1"],
                20,
            ))
            .expect("a run");
        let (asked, after_ask) = actor
            .plan(a_command(
                &["check", "--wait", "--retry-request", "r-t"],
                21,
            ))
            .expect("the first look");
        assert!(asked.waiting.is_some(), "an empty inbox leaves a wait");
        let empty_answer = asked.reply.stdout.clone();
        let (moved, revision) = actor
            .serve_receipt(Box::new(*asked.clone()), 22)
            .expect("the timeout files what the first look wrote");
        assert!(moved, "a named retry filed nothing");
        assert_eq!(revision, after_ask + 1);
        let (replayed, _) = actor
            .plan(a_command(
                &["check", "--wait", "--retry-request", "r-t"],
                23,
            ))
            .expect("the replay");
        assert_eq!(replayed.reply.exit_code, 0, "{}", replayed.reply.stderr);
        assert_eq!(
            replayed.reply.stdout, empty_answer,
            "the replay is the answer the wait actually gave"
        );
        /* And a decision with no receipt to file says so without writing. */
        let (bare, _) = actor
            .plan(a_command(&["run-list"], 24))
            .expect("a read with no retry name");
        assert_eq!(
            actor
                .serve_receipt(Box::new(*bare), 25)
                .expect("nothing to file"),
            (false, actor.view().expect("image").revision()),
        );
        actor.shutdown().expect("join timeout actor");
    }

    /// A release's end walks through the door with the screen the shell
    /// read — or without one, which is `release_unknown`: asked and not
    /// answered, the terminal left alone.
    #[test]
    fn a_release_settles_with_the_screen_it_read_or_says_unknown() {
        let fixture = Fixture::new();
        let shared = TableFixture::holding(a_team());
        let actor = start_with(
            &fixture,
            cutover(None, 10),
            Box::new((*shared).clone()),
            Box::new(AnsweringLauncher),
        );
        actor
            .plan(a_command(
                &["run-create", "--name", "nightly", "--retry-request", "r-1"],
                20,
            ))
            .expect("a run");
        let (started, _) = actor
            .plan(a_command(
                &[
                    "worker-start",
                    "--agent",
                    "claude",
                    "--retry-request",
                    "r-2",
                ],
                21,
            ))
            .expect("a worker in the ledger");
        assert_eq!(started.reply.exit_code, 0, "{}", started.reply.stderr);
        let worker_id = started
            .reply
            .stdout
            .split("\"workerId\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("a worker id in the start reply")
            .to_string();
        let request = EffectRequest::new(
            "main-ledger",
            digest(0x35),
            digest(0x45),
            digest(0xb5),
            digest(0xe5),
            HostEffectKind::Split,
        )
        .expect("an effect request");
        let (begun, _) = actor.prepare_effect(request, 22).expect("the reservation");
        let BeginEffect::Execute(permit) = begun else {
            panic!("a fresh effect executes, got {begun:?}");
        };
        let zerocode_core::agent_teams::Effect::Split {
            pane,
            from,
            direction,
            ..
        } = started.effect
        else {
            panic!("a worker start carries a split effect");
        };
        shared.with_team("team-1", &mut |team| {
            team.expect("the one team")
                .record_split(&pane, 7, &from, direction);
        });
        shared
            .tokens
            .lock()
            .unwrap()
            .insert(("team-1".to_string(), pane.clone()), capability_of(&pane));
        actor
            .settle_effect(
                permit,
                EffectSettlement::Applied {
                    result_digest: digest(0x95),
                    settled_at_ms: 23,
                },
            )
            .expect("the settlement");
        /* The release is PLANNED first — the verb is what moves the worker
         * into its pending state and asks for the capture; the door below is
         * only the settlement of what the shell then read. */
        let (releasing, before) = actor
            .plan(a_command(
                &[
                    "worker-release",
                    "--worker",
                    &worker_id,
                    "--retry-request",
                    "r-3",
                ],
                24,
            ))
            .expect("the release plan");
        assert_eq!(releasing.reply.exit_code, 0, "{}", releasing.reply.stderr);
        assert!(matches!(&releasing.effect,
            Effect::WorkerTerminal { seat, stop: None, incarnation: Some(_), .. }
                if seat.worker == worker_id));
        let (state, revision) = actor
            .worker_terminal_settled(releasing.effect, Some("the last screen".to_string()), 25)
            .expect("the release settles");
        assert_eq!(state, WorkerState::Released);
        assert_eq!(revision, before + 1);
        let image = actor.view().expect("the released image");
        let released = image
            .projection()
            .workers
            .iter()
            .find(|worker| worker.id == worker_id)
            .expect("the released worker's row");
        assert_eq!(released.state, WorkerState::Released);
        /* Asked again without a screen: `release_unknown` answers the state
         * it already reached rather than inventing a second retirement. */
        let (again, _) = actor
            .release_settled(worker_id, None, 25)
            .expect("the unknown arm");
        assert_eq!(again, WorkerState::Released);
        actor.shutdown().expect("join release actor");
    }

    /// An answer built after the effect — the release's own JSON — is
    /// remembered under the caller's retry name through its own door, and
    /// replays from there.
    #[test]
    fn an_answer_built_after_the_effect_is_remembered_and_replays() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(None, 10),
            TableFixture::holding(a_team()),
            Box::new(NoLauncher),
        );
        actor
            .plan(a_command(
                &["run-create", "--name", "nightly", "--retry-request", "r-1"],
                20,
            ))
            .expect("a run");
        /* A key only ever comes out of a decision this actor handed back —
         * the wait shape leaves one unfiled, which is exactly the state an
         * after-the-effect answer is in. */
        let (asked, after_ask) = actor
            .plan(a_command(
                &["check", "--wait", "--retry-request", "r-r"],
                21,
            ))
            .expect("a decision carrying an unfiled key");
        let key = asked.receipt.clone().expect("a named retry has a key");
        let said = "{\"workerId\":\"w-1\",\"state\":\"released\",\"archived\":true}\n";
        let revision = actor
            .remember_served(key, said, 22)
            .expect("the answer is remembered");
        assert_eq!(revision, after_ask + 1);
        let (replayed, _) = actor
            .plan(a_command(
                &["check", "--wait", "--retry-request", "r-r"],
                23,
            ))
            .expect("the replay");
        assert_eq!(replayed.reply.exit_code, 0, "{}", replayed.reply.stderr);
        assert_eq!(
            replayed.reply.stdout, said,
            "the replay is the answer that was remembered"
        );
        actor.shutdown().expect("join remembered actor");
    }
    #[test]
    fn scm_observation_retries_a_refused_sql_write_and_replays_after_restart() {
        let fixture = Fixture::new();
        let actor = start_with(
            &fixture,
            cutover(Some(a_seated_legacy()), 10),
            a_seated_table(),
            Box::new(NoLauncher),
        );
        actor.worker_seated("team-1", "%2", "/wt/scm", 11).unwrap();
        let connection = fixture.store.connection().unwrap();
        connection.execute_batch("CREATE TRIGGER fail_scm AFTER UPDATE OF revision ON orchestration_ledger_heads BEGIN SELECT RAISE(ABORT, 'scm write failure'); END;").unwrap();
        let send = |actor: &RuntimeActor, now| {
            actor.observation_once(
                "@worktree:/wt/scm".into(),
                "ci failed".into(),
                Some("scm:receipt".into()),
                now,
            )
        };
        assert_eq!(send(&actor, 12).unwrap_err(), RuntimeError::NotDurable);
        assert!(
            actor
                .view()
                .unwrap()
                .projection()
                .messages
                .iter()
                .all(|row| row.subject.as_str() != "scm:receipt")
        );
        connection.execute_batch("DROP TRIGGER fail_scm;").unwrap();
        assert!(send(&actor, 13).unwrap().0);
        actor.shutdown().unwrap();
        let again = start_with(
            &fixture,
            RuntimeBoot::Reopen,
            a_seated_table(),
            Box::new(NoLauncher),
        );
        assert!(send(&again, 14).unwrap().0);
        assert_eq!(
            again
                .view()
                .unwrap()
                .projection()
                .messages
                .iter()
                .filter(|row| row.subject.as_str() == "scm:receipt")
                .count(),
            1
        );
        again.shutdown().unwrap();
    }
}
