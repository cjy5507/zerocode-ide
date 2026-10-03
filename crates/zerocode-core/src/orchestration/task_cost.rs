//! What one finished task cost (t-9470, the Jev harness design's §H4): the
//! generation model's tokens and their API-equivalent price, the Jev requests
//! stamped with it, how many attempts it took and how long it stood open —
//! read off facts the window already holds, and never estimated.
//!
//! ## Where each number comes from
//!
//! A task is its attempts: every [`Dispatch`] of it, the rows
//! [`Run::newest_attempt`] picks the newest of and [`Run::review_of`] binds a
//! review to. More than one is rework, and nothing here counts it a second
//! way. The wall clock runs from the first attempt's start to the last one's
//! end, and the waits between them are in it.
//!
//! An attempt's tokens are its recorded conversations', joined by the
//! provider's own id to the vendor ledger the
//! usage scan already holds — Claude's transcripts ([`crate::usage_stats`]),
//! Codex's rollouts and OpenCode's database ([`crate::usage_ledger`]). The
//! worker retains its latest conversation for resume while each dispatch
//! keeps the session history its reports established. Old dispatches without
//! a complete history expose the known usage without calling it a complete
//! bill. [`GenerationCost::sessions_known`] counts distinct provider sessions,
//! never attempts, and a conversation shared outside the requested scope is
//! not apportioned by guessing.
//!
//! ## What is never a number
//!
//! A figure nobody can check is not written as one. The dollars stay `None`,
//! with the reason ([`UsdReason`]), wherever one attempt's price cannot be
//! read whole — an agent with no usage ledger (zo), a scan read before the
//! attempt ended or not at all, a conversation the scan does not hold or that
//! another task shares, one that switched models mid-way (Claude's ledger
//! keeps one model per conversation, the last, and pricing all of it at that
//! rate is wrong), a model with no price. The tokens beside it are the ones
//! that were linked.
//!
//! A handover is not a switch: the replacement is a new worker in a new
//! conversation (`worker-start --retry-of … --prompt`), priced at its own
//! model, and the attempt it replaced keeps its own.
//!
//! ## Jev
//!
//! A Jev request counts toward a task only where its row carries the task's
//! id — the seats the window stamps ([`TASK_STAMPED`]). Every other seat, and
//! zo's own project ledgers, write no task and are not counted;
//! [`JevCost::unstamped_seats`] says how many seats that leaves out. The
//! stamping seats carry the common wire's input-token accounting when known.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::Serialize;
use serde_json::Value;

use super::{Dispatch, MessageKind, Run};
use crate::agent::AgentKind;
use crate::jev::{self, JevUse, summary};
use crate::{usage_ledger, usage_stats, usage_stats_codex};

mod session_attribution;
pub use session_attribution::SessionAttribution;

/// The key a stamping seat writes the ledger's task id under.
pub const TASK_STAMP: &str = "task";

/// The seats whose request rows carry the task they were asked for
/// ([`TASK_STAMP`]): a silence judged, a worker placed, a summons chosen, a
/// step's effort moved, a coordinator's letter triaged (t-9471). Each writes
/// the run, worker, attempt and task beside its answer; a seat that does not
/// is not counted toward any task.
pub const TASK_STAMPED: [&JevUse; 7] = [
    &jev::STALL,
    &jev::PLACEMENT,
    &jev::SUMMON,
    &jev::SUMMON_DIFFICULTY,
    &jev::SUMMON_MODEL,
    &jev::STEP_EFFORT,
    &jev::MAIL_TRIAGE,
];

/// A vendor ledger the usage scan reads — where a conversation's tokens can be
/// found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UsageSource {
    Claude,
    Codex,
    OpenCode,
}

impl UsageSource {
    /// The ledger an agent's conversations are written to, or `None` for an
    /// agent with none on this machine — zo, and every CLI no scan reads.
    #[must_use]
    pub fn of_agent(agent: &str) -> Option<Self> {
        match AgentKind::from_slug(agent)? {
            AgentKind::Claude => Some(Self::Claude),
            AgentKind::Codex => Some(Self::Codex),
            AgentKind::Opencode => Some(Self::OpenCode),
            _ => None,
        }
    }
}

/// One conversation's spend, as its vendor's ledger totalled it, in the four
/// counters every vendor can be read into: fresh input, output, and the
/// cache's two sides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSpend {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Priced the vendor's way; `None` where its model has no price.
    pub usd: Option<f64>,
    /// Whether the vendor's own ledger saw more than one model in it.
    pub mixed_models: bool,
    invalid_usage: bool,
}

impl SessionSpend {
    fn counters(&self) -> Option<[i64; 4]> {
        let counters = [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ];
        if self.invalid_usage || self.usd.is_some_and(|usd| !usd.is_finite() || usd < 0.0) {
            return None;
        }
        token_sum(counters).map(|_| counters)
    }
}

fn token_sum(counters: [i64; 4]) -> Option<i64> {
    counters.into_iter().try_fold(0_i64, |total, tokens| {
        (tokens >= 0).then(|| total.checked_add(tokens)).flatten()
    })
}

/// Every conversation the held usage scans know, by vendor and the provider's
/// own id — read once per scan, then only looked up.
#[derive(Clone, Debug, Default)]
pub struct SessionBook {
    sessions: HashMap<(UsageSource, String), SessionSpend>,
    scanned_at: HashMap<UsageSource, i64>,
}

impl SessionBook {
    /// Claude's transcripts, read at `scanned_at`: four independent
    /// counters, priced at the conversation's model — the last one it named
    /// ([`usage_stats::Session::model`]), which is why a switch mid-way is
    /// the ledger's to say ([`MessageKind::ModelDeviated`]).
    pub fn read_claude(&mut self, ledger: &usage_stats::Ledger, scanned_at: i64) {
        self.scanned_at.insert(UsageSource::Claude, scanned_at);
        for session in &ledger.sessions {
            self.sessions.insert(
                (UsageSource::Claude, session.session_id.clone()),
                SessionSpend {
                    input_tokens: session.total_input_tokens,
                    output_tokens: session.total_output_tokens,
                    cache_read_tokens: session.total_cache_read_tokens,
                    cache_write_tokens: session.total_cache_write_tokens,
                    usd: usage_stats::estimate_cost_usd(
                        session.model.as_deref(),
                        session.total_input_tokens,
                        session.total_output_tokens,
                        session.total_cache_read_tokens,
                        session.total_cache_write_tokens,
                    ),
                    mixed_models: false,
                    invalid_usage: false,
                },
            );
        }
    }

    /// Codex's rollouts, read at `scanned_at`: the cached input is a SHARE of
    /// the input and the reasoning is inside the output
    /// ([`usage_stats_codex::estimate_cost_usd`]), so the fresh input is the
    /// difference; priced from Codex's own table at the conversation's
    /// model, and mixed where its ledger saw more than one — a turn it could
    /// not name counts as one more, since pricing that turn at the
    /// conversation's model would be a guess.
    pub fn read_codex(&mut self, ledger: &usage_ledger::Ledger, scanned_at: i64) {
        self.scanned_at.insert(UsageSource::Codex, scanned_at);
        for session in &ledger.sessions {
            let cached = session.cached_input_tokens.min(session.input_tokens);
            let fresh = session.input_tokens.checked_sub(cached);
            self.sessions.insert(
                (UsageSource::Codex, session.session_id.clone()),
                SessionSpend {
                    input_tokens: fresh.unwrap_or(0),
                    output_tokens: session.output_tokens,
                    cache_read_tokens: cached,
                    cache_write_tokens: 0,
                    usd: usage_stats_codex::estimate_cost_usd(
                        session.model.as_deref(),
                        session.input_tokens,
                        session.cached_input_tokens,
                        session.output_tokens,
                    ),
                    mixed_models: session.model_breakdown.len() > 1,
                    invalid_usage: fresh.is_none()
                        || session.input_tokens < 0
                        || session.cached_input_tokens < 0
                        || session.cached_input_tokens > session.input_tokens
                        || session.reasoning_output_tokens < 0
                        || session.reasoning_output_tokens > session.output_tokens,
                },
            );
        }
    }

    /// OpenCode's database, read at `scanned_at`: its counters are separate
    /// buckets (reasoning beside the output, the cache beside the input),
    /// and it reported its own dollars per turn — carried, never priced
    /// again, so a switch of model inside a conversation is priced already.
    pub fn read_opencode(&mut self, ledger: &usage_ledger::Ledger, scanned_at: i64) {
        self.scanned_at.insert(UsageSource::OpenCode, scanned_at);
        for session in &ledger.sessions {
            let output = session
                .output_tokens
                .checked_add(session.reasoning_output_tokens);
            self.sessions.insert(
                (UsageSource::OpenCode, session.session_id.clone()),
                SessionSpend {
                    input_tokens: session.input_tokens,
                    output_tokens: output.unwrap_or(0),
                    cache_read_tokens: session.cached_input_tokens,
                    cache_write_tokens: 0,
                    usd: session.estimated_cost_usd,
                    mixed_models: false,
                    invalid_usage: output.is_none()
                        || session.output_tokens < 0
                        || session.reasoning_output_tokens < 0,
                },
            );
        }
    }

    /// When `source`'s scan was read, or `None` when none is held.
    #[must_use]
    pub fn scanned_at(&self, source: UsageSource) -> Option<i64> {
        self.scanned_at.get(&source).copied()
    }

    fn spend(&self, source: UsageSource, id: &str) -> Option<&SessionSpend> {
        self.sessions.get(&(source, id.to_string()))
    }
}

/// What one task's stamped Jev rows add up to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct JevTally {
    /// Rows that asked something ([`summary::asked_something`]).
    pub rows: u64,
    /// The requests those rows spent ([`summary::REQUESTS`]).
    pub requests: u64,
    /// The input tokens the rows that recorded any billed.
    pub input_tokens: u64,
    /// How many of the rows recorded them.
    pub rows_with_tokens: u64,
}

impl std::ops::Add for JevTally {
    type Output = Self;

    /// Two ledgers' tallies of one task, together.
    fn add(self, other: Self) -> Self {
        Self {
            rows: self.rows + other.rows,
            requests: self.requests + other.requests,
            input_tokens: self.input_tokens + other.input_tokens,
            rows_with_tokens: self.rows_with_tokens + other.rows_with_tokens,
        }
    }
}

/// The Jev requests every task was asked for, read off a stamping seat's
/// rows one row at a time, as they are appended.
#[derive(Clone, Debug, Default)]
pub struct JevBook {
    by_task: HashMap<String, JevTally>,
}

impl JevBook {
    /// One row of a stamping seat's ledger. A row that asked nothing — a
    /// label, the judge's own note, a control ([`summary::asked_something`])
    /// — and a row stamped with no task add nothing.
    pub fn read(&mut self, row: &Value) {
        let Some(task) = row.get(TASK_STAMP).and_then(Value::as_str) else {
            return;
        };
        let Some(outcome) = summary::asked_something(row) else {
            return;
        };
        let usage = summary::summarize_rows([row], i64::MIN);
        let tally = self.by_task.entry(task.to_string()).or_default();
        tally.rows += 1;
        if summary::CACHED.read(row).and_then(Value::as_bool) != Some(true)
            && !summary::is_refusal(outcome)
        {
            tally.requests += usage.requests;
        }
        tally.input_tokens += usage.input_tokens;
        if usage.unmetered_requests == 0 {
            tally.rows_with_tokens += 1;
        }
    }

    /// What `task_id`'s stamped rows came to.
    #[must_use]
    pub fn tally(&self, task_id: &str) -> JevTally {
        self.by_task.get(task_id).copied().unwrap_or_default()
    }
}

/// Why a task's generation dollars are not a number, most fundamental first
/// — the one a task carries is the first that holds for any of its attempts.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum UsdReason {
    /// An attempt ran on an agent with no usage ledger (zo, and every CLI no
    /// scan reads): its tokens exist and nothing here can count them.
    UnsupportedAgent,
    /// No scan of that agent's ledger was read after the attempt ended —
    /// none is held, or the one held was read before the work was over.
    Unscanned,
    /// A conversation could not be tied to this task alone: the attempt has
    /// no worker row here or reported none, the scan does not hold it, or
    /// its worker carried another task's attempt in it too.
    Unlinked,
    IncompleteSessionHistory,
    InvalidUsage,
    /// A conversation switched models mid-way — a switch the ledger wrote
    /// against the attempt, or more than one model in the vendor's own
    /// ledger — and its vendor keeps one price per conversation.
    MixedModels,
    /// A model with no price on file.
    UnpricedModel,
}

/// What the generation model spent in the task's recorded conversations.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationCost {
    pub history_complete: bool,
    /// Distinct provider conversations recorded for the task's attempts.
    pub sessions_known: usize,
    /// Those of them found in a scan and tied to this task alone.
    pub sessions_linked: usize,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// What the linked conversations would have cost on the API — an
    /// equivalent, never a bill for a subscription. `None` with
    /// [`Self::usd_reason`] wherever one attempt's price cannot be read whole.
    pub usd: Option<f64>,
    pub usd_reason: Option<UsdReason>,
}

impl GenerationCost {
    #[must_use]
    pub fn measured_tokens(&self) -> Option<i64> {
        if !self.history_complete
            || self.sessions_linked == 0
            || self.sessions_linked != self.sessions_known
            || matches!(
                self.usd_reason,
                Some(
                    UsdReason::UnsupportedAgent
                        | UsdReason::Unscanned
                        | UsdReason::Unlinked
                        | UsdReason::IncompleteSessionHistory
                        | UsdReason::InvalidUsage
                )
            )
        {
            return None;
        }
        token_sum([
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ])
    }

    fn add_tokens(&mut self, spend: &SessionSpend) -> Option<()> {
        let [input, output, cache_read, cache_write] = spend.counters()?;
        let counters = [
            self.input_tokens.checked_add(input)?,
            self.output_tokens.checked_add(output)?,
            self.cache_read_tokens.checked_add(cache_read)?,
            self.cache_write_tokens.checked_add(cache_write)?,
        ];
        token_sum(counters)?;
        [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ] = counters;
        Some(())
    }
}

/// What Jev spent on the task: the requests of the rows stamped with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevCost {
    pub requests: u64,
    /// The seats that stamp a task ([`TASK_STAMPED`]) — the only ones read.
    pub stamped_seats: usize,
    /// The seats that do not, and so are never counted toward a task.
    pub unstamped_seats: usize,
    /// The input tokens the rows billed, where every counted row recorded
    /// them; `None` where one did not.
    pub input_tokens: Option<u64>,
}

impl JevCost {
    fn of(tally: JevTally) -> Self {
        Self {
            requests: tally.requests,
            stamped_seats: TASK_STAMPED.len(),
            unstamped_seats: jev::JEV_USES.len().saturating_sub(TASK_STAMPED.len()),
            input_tokens: (tally.rows_with_tokens == tally.rows).then_some(tally.input_tokens),
        }
    }
}

/// What one task cost.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCost {
    /// Every attempt at the task, open or ended.
    pub attempts: usize,
    /// From the first attempt's start to the last one's end, waits included;
    /// `None` while an attempt is open, and for a task never attempted.
    pub wall_ms: Option<i64>,
    pub completion_wall_ms: Option<i64>,
    pub generation: GenerationCost,
    pub jev: JevCost,
}

/// Every attempt at `task_id`, oldest first — the dispatches
/// [`Run::newest_attempt`] picks the newest of.
#[must_use]
pub fn attempts<'run>(run: &'run Run, task_id: &str) -> Vec<&'run Dispatch> {
    run.dispatches
        .iter()
        .filter(|one| one.task == task_id)
        .collect()
}

/// What `task_id` cost in `run`, read off its attempts, its workers'
/// conversations in `sessions`, and what its stamped Jev rows came to
/// (`jev`, [`JevBook::tally`] — summed over the stamping seats' ledgers).
#[must_use]
pub fn task_cost(
    run: &Run,
    task_id: &str,
    sessions: &SessionBook,
    jev: JevTally,
    attribution: &SessionAttribution,
) -> TaskCost {
    let attempts = attempts(run, task_id);
    TaskCost {
        attempts: attempts.len(),
        wall_ms: wall_ms(&attempts),
        completion_wall_ms: run.task(task_id).and_then(|task| {
            let completed = run.completion_ms(task)?;
            let first = attempts.iter().map(|attempt| attempt.started_ms).min()?;
            completed.checked_sub(first).filter(|elapsed| *elapsed >= 0)
        }),
        generation: generation(run, &attempts, sessions, attribution),
        jev: JevCost::of(jev),
    }
}

/// One attempt's usage when its recorded conversations belong to it alone.
/// Session totals shared with another attempt remain unlinked.
#[must_use]
pub fn attempt_generation(
    run: &Run,
    attempt: &Dispatch,
    sessions: &SessionBook,
    attribution: &SessionAttribution,
) -> GenerationCost {
    generation(run, &[attempt], sessions, attribution)
}

fn reported_sessions<'run>(
    run: &'run Run,
    attempt: &'run Dispatch,
) -> Vec<&'run crate::ProviderSession> {
    if let Some(history) = &attempt.session_history {
        history.sessions.iter().collect()
    } else {
        run.worker(&attempt.worker)
            .and_then(|worker| worker.session.as_ref())
            .into_iter()
            .collect()
    }
}

#[must_use]
pub fn generation_fingerprint(run: &Run) -> u64 {
    let mut digest = std::hash::DefaultHasher::new();
    for worker in &run.workers {
        (&worker.id, &worker.agent).hash(&mut digest);
        worker
            .session
            .as_ref()
            .map(|session| (session.key, session.id.as_str()))
            .hash(&mut digest);
    }
    for attempt in &run.dispatches {
        (
            &attempt.id,
            &attempt.task,
            &attempt.worker,
            attempt.started_ms,
            attempt.ended_ms,
        )
            .hash(&mut digest);
        attempt
            .session_history
            .as_ref()
            .map(|history| history.complete)
            .hash(&mut digest);
        for session in reported_sessions(run, attempt) {
            (session.key, session.id.as_str()).hash(&mut digest);
        }
    }
    for message in run
        .messages()
        .iter()
        .filter(|message| message.kind == MessageKind::ModelDeviated)
    {
        message.dispatch.hash(&mut digest);
    }
    digest.finish()
}

/// First start to last end, or `None` while any attempt is open.
fn wall_ms(attempts: &[&Dispatch]) -> Option<i64> {
    let first = attempts.iter().map(|one| one.started_ms).min()?;
    let last = attempts.iter().try_fold(first, |latest, attempt| {
        let ended = attempt.ended_ms?;
        (attempt.started_ms >= 0 && ended >= attempt.started_ms).then_some(latest.max(ended))
    })?;
    last.checked_sub(first)
}

/// The linked conversations' tokens and dollars, and the first reason the
/// dollars are not a number.
fn generation(
    run: &Run,
    attempts: &[&Dispatch],
    sessions: &SessionBook,
    attribution: &SessionAttribution,
) -> GenerationCost {
    let switched: HashSet<&str> = run
        .messages()
        .iter()
        .filter(|message| message.kind == MessageKind::ModelDeviated)
        .filter_map(|message| message.dispatch.as_deref())
        .collect();
    let included: HashSet<&str> = attempts.iter().map(|attempt| attempt.id.as_str()).collect();
    let mut cost = GenerationCost {
        history_complete: true,
        ..GenerationCost::default()
    };
    let mut dollars = 0.0;
    let mut reason: Option<UsdReason> = None;
    let mut worse = |why: UsdReason| reason = Some(reason.map_or(why, |held| held.min(why)));
    let mut counted = HashSet::new();
    for attempt in attempts {
        let Some(worker) = run.worker(&attempt.worker) else {
            cost.history_complete = false;
            worse(UsdReason::Unlinked);
            continue;
        };
        if !attempt
            .session_history
            .as_ref()
            .is_some_and(|history| history.complete)
        {
            cost.history_complete = false;
            worse(UsdReason::IncompleteSessionHistory);
        }
        let conversations = reported_sessions(run, attempt);
        if conversations.is_empty() {
            worse(UsdReason::Unlinked);
            continue;
        }
        let source = UsageSource::of_agent(&worker.agent);
        for session in conversations {
            if !counted.insert((
                source,
                source.is_none().then_some(worker.agent.as_str()),
                session.id.as_str(),
            )) {
                continue;
            }
            cost.sessions_known += 1;
            let Some(source) = source else {
                worse(UsdReason::UnsupportedAgent);
                continue;
            };
            let carried = attribution.owners(source, session);
            if carried.is_empty()
                || carried
                    .iter()
                    .any(|other| other.run != run.id || !included.contains(other.dispatch.as_str()))
            {
                worse(UsdReason::Unlinked);
                continue;
            }
            let ended = carried
                .iter()
                .map(|other| other.ended_ms)
                .collect::<Option<Vec<_>>>()
                .and_then(|ends| ends.into_iter().max());
            if sessions
                .scanned_at(source)
                .zip(ended)
                .is_none_or(|(read, ended)| read < ended)
            {
                worse(UsdReason::Unscanned);
                continue;
            }
            let Some(spend) = sessions.spend(source, &session.id) else {
                worse(UsdReason::Unlinked);
                continue;
            };
            if cost.add_tokens(spend).is_none() {
                worse(UsdReason::InvalidUsage);
                continue;
            }
            cost.sessions_linked += 1;
            if spend.mixed_models
                || carried
                    .iter()
                    .any(|other| switched.contains(other.dispatch.as_str()))
            {
                worse(UsdReason::MixedModels);
            } else if let Some(usd) = spend.usd {
                dollars += usd;
                if !dollars.is_finite() {
                    worse(UsdReason::InvalidUsage);
                }
            } else {
                worse(UsdReason::UnpricedModel);
            }
        }
    }
    cost.usd = reason.is_none().then_some(dollars);
    cost.usd_reason = reason;
    cost
}

#[cfg(test)]
mod tests;
