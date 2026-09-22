//! The routing probe's typed twin, in shadow — the tools-layer half of
//! TypeSafe System One judgments for routing.
//!
//! `smart.decisionShadow` in a record-only mode (`shadow`, `auto`) records the
//! typed judgment beside the chat probe without affecting routing. `on` asks
//! System One first and applies a validated verdict through the existing
//! probe-fusion boundary;
//! any missing or invalid answer falls back to the unchanged chat probe for
//! that task. Both modes use the same typed request, validation, bounded memo,
//! and fingerprint-only ledger.
//!
//! Shadow work stays detached. Active batches wait once for all unique tasks
//! concurrently under a short wall, then return typed assessments to the one
//! shared routing entry. No raw task text or secret is logged or persisted.
//!
//! Every request goes through the Jev door (`jev_gate`): no key, Jev switched
//! off, a workspace nobody consented to or a spent day's budget is a row that
//! names the refusal and sends nothing, and what is sent has lost every line
//! that may carry a credential. A memo answer sends nothing and so meets no
//! door.
//!
//! An acting seat skips the chat probe, so its rows alone can never say how
//! often the judgment agrees with it. One active turn in
//! [`PROBE_CONTROL_EVERY`] therefore runs the probe once more after its answer
//! is written — detached, like the shadow — and appends a control row: the
//! same shape, the judgment copied from the row it acted on, the probe's
//! answer where `not_run` stood. The judge reads it for agreement and for
//! nothing else ([`zerocode_core::jev::summary::CONTROL`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SYSTEMONE_MODEL};
use zerocode_core::jev::door::Refused;
use runtime::{
    DecisionVerdict, ProbeAssessment, RubricAxis, SwitchTrigger,
    DECISION_RUBRIC_VERSION,
};
use serde::{Deserialize, Serialize};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, ProbeSlot, ProbeUse, PROBE_TIMEOUT};
use super::settings::decision_shadow_mode_from;
use zerocode_core::jev::ROUTING;
use zerocode_core::jev::promote::{self, Verdict};
use zerocode_core::jev::summary::{self as jev_ledger};

use super::jev_summary;
use super::shadow_ledger::{append_shadow_row, last_shadow_lines, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};
use crate::misc_tools::agent_tools::shared_agent_runtime;

/// The decision shadow's ledger file, under the shared shadow-ledger directory
/// — the Jev use table's name for the routing row's ledger.
pub const DECISION_SHADOW_FILE: &str = zerocode_core::jev::ROUTING.ledger;

/// The outcome of a row whose judgment arrived and passed every check. Every
/// other outcome is a failure token from [`SystemOneFailure`].
///
/// The word is the door's, because the hedge rule's sample reads it out of
/// this ledger (`JevDoor::hedge_for`) and a reader that spelled it
/// differently from the writer would find no answers at all.
pub const OUTCOME_ANSWERED: &str = zerocode_core::jev::door::ANSWERED_OUTCOME;

/// The record-only judgment wall. It matches the chat probe so the comparison
/// ledger can say whether Jev would have arrived in time.
pub(super) const DECISION_SHADOW_DEADLINE: Duration = PROBE_TIMEOUT;

/// The shorter wall for a judgment that is allowed to delay routing. All
/// unique tasks run concurrently, so a batch pays at most this wall once.
pub(super) const DECISION_ACTIVE_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::ROUTING_APPLY_DEADLINE_MS);

/// An active row uses this probe cell when no chat probe ran. It is an explicit
/// absence, not a fabricated chat-model verdict, and comparison metrics skip it.
const PROBE_NOT_RUN: &str = "not_run";

/// The outcome a control row carries — the counter's word, read from where
/// the counter that leaves such a row out of every window spells it.
pub const OUTCOME_CONTROL: &str = jev_ledger::CONTROL;

/// The key a row's task fingerprint is written under, for a reader that joins
/// rows by it without deserializing them whole.
const TASK: &str = "task";

/// One active turn in this many runs the chat probe it skipped, detached and
/// after its answer is written, and appends a control row.
///
/// Why any: an acting seat writes `not_run` for the probe on every row, so
/// the agreement line — which needs rows where both readers answered — stops
/// filling the moment the seat acts, and an `auto` that rose could never be
/// shown to still agree (this machine's last seven rows, every one `applied`
/// with `not_run`, 2026-09-21).
///
/// Why five. The judge wants
/// [`zerocode_core::jev::summary::JUDGED_EVERY_ROWS`] compared axes over the
/// window its floor can be cleared on
/// ([`zerocode_core::jev::promote::window_wanted_for`]: 73 rows for routing's
/// 0.95, which forgives nothing). One turn in five is 14 control rows a window, three
/// judged axes each — 42 compared if the probe answers every time; at the
/// probe's measured share of timeouts (11 rows of 28, 2026-09-20) eight still
/// answer, 24 axes, clear of the twenty with a row to spare. One in ten would
/// be seven rows, four answering, twelve axes: under the line every window.
/// The cost is one probe call per five active turns, off the turn's clock;
/// the sample's arithmetic is pinned by a test beside the judge's line.
pub const PROBE_CONTROL_EVERY: u64 = 5;

/// Whether a task's turn is the one in [`PROBE_CONTROL_EVERY`] that runs the
/// control probe. Decided on the fingerprint, so the same task is always the
/// same answer: a fan-out retry or a memo hit does not move the sample, and a
/// test can pick a task that is in it or out of it.
#[must_use]
pub fn control_sampled(task: u64) -> bool {
    task.is_multiple_of(PROBE_CONTROL_EVERY)
}

/// Failure: the merged settings could not be read, so nothing was sent.
const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// Whether the routing seat has been raised to acting by its own evidence.
///
/// Read from the ledger it writes, so the rows a promotion was decided on and
/// the promotion itself cannot be found apart.
#[must_use]
pub fn raised_here(cwd: &Path) -> bool {
    raised_at(&decision_shadow_path(cwd))
}

/// The same question asked of a ledger by its path — where the project's
/// state directory is answered by the environment, and a test that had to
/// pin that would be racing every other test in this binary for it.
#[must_use]
pub fn raised_at(ledger: &Path) -> bool {
    let rows = super::jev_summary::read_rows(ledger);
    zerocode_core::jev::promote::stand_from(&rows) == zerocode_core::jev::promote::Stand::Applying
}

/// Where a project's decision shadow ledger lives.
#[must_use]
pub fn decision_shadow_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, DECISION_SHADOW_FILE)
}

/// The task a key check asks about — words no person wrote, so checking a key
/// sends nothing of anyone's work off the machine.
pub const KEY_CHECK_TASK: &str = "Rename one local variable inside a single function.";

/// Why a key check names no model: the door sent nothing, or the wire failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckFailure {
    Refused(Refused),
    Wire(SystemOneFailure),
}

impl CheckFailure {
    /// The token a ledger row would carry for the same failure.
    #[must_use]
    pub fn ledger_token(self) -> String {
        match self {
            Self::Refused(refused) => refused.token().to_string(),
            Self::Wire(failure) => failure.ledger_token(),
        }
    }
}

/// What one key check saw: the model that answered, or why none did — and how
/// long the call took, how often it was re-sent, and which rung of the key
/// ladder held the key it was sent with.
///
/// The rung is here because "it worked" and "it worked with the key the window
/// keeps, which no launch handed me" are different answers, and the second is
/// the one a zo run outside the window is being asked about (t-5805). `None`
/// is no key at all, which is the `no_key` the outcome already names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemOneCheck {
    pub outcome: Result<String, CheckFailure>,
    pub elapsed: Duration,
    pub retries: u32,
    pub key_source: Option<api::KeySource>,
}

/// Put the shadow's own question about [`KEY_CHECK_TASK`] to System One, once:
/// the key this process would use (`TYPESAFE_API_KEY`, or the one the window's
/// settings keep), the door's key check, the rubric's request, the shadow's
/// deadline and its answer check — so a key that passes here is one the shadow
/// can use, and one that fails is named by the same token a ledger row would
/// carry. The memo is not consulted: a check that recalled an answer would
/// prove nothing about the key.
pub async fn check_system_one() -> SystemOneCheck {
    let config = SystemOneConfig::from_env().ok();
    let key_source = config.as_ref().map(SystemOneConfig::key_source);
    let unsent = |failure| SystemOneCheck {
        outcome: Err(failure),
        elapsed: Duration::ZERO,
        retries: 0,
        key_source,
    };
    let client = config.map(SystemOneConfig::into_client);
    let state = runtime::rubric_task_text("", KEY_CHECK_TASK);
    let request = runtime::decision_request(SYSTEMONE_MODEL, &state);
    let Some(body) = jev_gate::body_of(&request) else {
        return unsent(CheckFailure::Wire(SystemOneFailure::InvalidRequest));
    };
    let passed = JevDoor::for_key_check().pass_key_check(client.is_some(), &body);
    let call = match (passed, &client) {
        (Ok(cleared), Some(client)) => {
            jev_gate::send(client, cleared, DECISION_SHADOW_DEADLINE, None).await
        }
        // The door refuses a keyless check before anything else it asks.
        (passed, _) => return unsent(CheckFailure::Refused(passed.err().unwrap_or(Refused::NoKey))),
    };
    let outcome = call.outcome.map_err(CheckFailure::Wire).and_then(|response| {
        runtime::validate_decision(&response)
            .map(|_| response.model)
            .map_err(|_| CheckFailure::Wire(SystemOneFailure::Schema))
    });
    SystemOneCheck { outcome, elapsed: call.elapsed, retries: call.retries, key_source }
}

/// The probe's side of a row: the token it answered on every rubric axis, by
/// axis name, or the token of why it said nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProbeCell {
    Answered(BTreeMap<String, String>),
    Failed(String),
}

impl ProbeCell {
    fn from_verdict(verdict: Result<ProbeAssessment, &'static str>) -> Self {
        match verdict {
            Ok(probe) => Self::Answered(
                probe
                    .tokens()
                    .iter()
                    .map(|(axis, token)| (axis.name.to_string(), (*token).to_string()))
                    .collect(),
            ),
            Err(token) => Self::Failed(token.to_string()),
        }
    }

    /// The token the probe answered on `axis`, when it answered.
    #[must_use]
    pub fn token(&self, axis: &RubricAxis) -> Option<&str> {
        match self {
            Self::Answered(tokens) => tokens.get(axis.name).map(String::as_str),
            Self::Failed(_) => None,
        }
    }
}

/// One axis of the typed judgment as the ledger keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgedAxis {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// How a ledger row participated in routing. Old rows deserialize as
/// `record_only`, preserving the original comparison-only schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionRouteUse {
    #[default]
    RecordOnly,
    Applied,
    Fallback,
    /// The control sample: the probe run once more, after the turn, beside a
    /// judgment an `applied` row already acted on. It routed nothing and its
    /// outcome is `OUTCOME_CONTROL`, not a judgment's.
    Control,
}

/// One task of the decision judgment ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionShadowRow {
    /// Unix milliseconds the row was written.
    pub at: u64,
    /// The attempt the probe was spent for, when a turn declared one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// The task's fingerprint, sixteen hex digits — never its text.
    pub task: String,
    pub rubric_version: u32,
    /// The model the response says answered; absent when nothing did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// [`OUTCOME_ANSWERED`], or the failure's ledger token.
    pub outcome: String,
    pub elapsed_ms: u64,
    pub retries: u32,
    /// Answered from this process's memo, with no request.
    pub cached: bool,
    /// What the call billed, when a response arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Whether this row was comparison-only, applied, or fell back to the
    /// existing chat probe.
    #[serde(default)]
    pub route_use: DecisionRouteUse,
    /// Requests this row's judgment sent: none when the door refused it or the
    /// memo answered, one plus its retries when it left. Absent on a row
    /// written before the door.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<u32>,
    /// Lines the door withheld from what was sent. Every row since the door
    /// carries it, which is how a row from before the door is told apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_lines: Option<u32>,
    /// When a second request of this judgment was planned to leave, on a road
    /// that planned one. Absent where the rule named no delay — too few
    /// samples, an ordinary answer already past the wall, or a day whose
    /// budget could not carry a second request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hedge_delay_ms: Option<u64>,
    /// Whether that second request actually left: no answer had come by the
    /// delay. Written wherever a delay was planned, so the share of plans
    /// that cost anything is readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hedge_fired: Option<bool>,
    /// Whether the second copy is the one that answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hedge_won: Option<bool>,
    /// The losing copy's own latency, when it had answered by the time the
    /// winner was read. The column the published rules do not have: two
    /// copies that are slow together make a hedge worthless, and this is what
    /// lets that be read off a real ledger instead of assumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loser_ms: Option<u64>,
    /// The chat probe's actual result. Active rows use `not_run` when Jev
    /// answered or before a failed Jev judgment falls back; a control row is
    /// where that probe's answer lands, one active turn in
    /// `PROBE_CONTROL_EVERY`.
    pub probe: ProbeCell,
    /// The typed judgment, axis by axis, when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<BTreeMap<String, JudgedAxis>>,
}

impl DecisionShadowRow {
    /// Whether the judgment answered, fresh or from the memo.
    #[must_use]
    pub fn answered(&self) -> bool {
        self.outcome == OUTCOME_ANSWERED
    }

    /// Whether this row's judgment went over the wire: it sent a request. A row
    /// from before the door says so the only way it can: not recalled, and not
    /// stopped before a socket for want of a key.
    #[must_use]
    pub fn called(&self) -> bool {
        self.requests.map_or_else(
            || !self.cached && self.outcome != SystemOneFailure::NoKey.token(),
            |requests| requests > 0,
        )
    }

    /// Whether the door refused this row's judgment before a byte left.
    #[must_use]
    pub fn refused(&self) -> bool {
        Refused::from_token(&self.outcome).is_some()
    }

    /// What this row's call cost, as the client counted it.
    ///
    /// `hedge` is the delay that was planned, whether or not a second request
    /// reached it; `call.hedge` is the one that left.
    fn spent(&mut self, call: &SystemOneCall, hedge: Option<Duration>, withheld: u32) {
        self.elapsed_ms = jev_gate::millis(call.elapsed);
        self.retries = call.retries;
        // What left the machine, as the client counted it. One plus the
        // retries stopped being that number the moment a judgment could be
        // asked twice.
        self.requests = Some(call.requests);
        self.redacted_lines = Some(withheld);
        self.hedge_delay_ms = hedge.map(jev_gate::millis);
        self.hedge_fired = hedge.map(|_| call.hedge.is_some());
        self.hedge_won = call.hedge.map(|ran| ran.won);
        self.loser_ms = call.hedge.and_then(|ran| ran.loser_ms);
    }

    fn new(
        task: u64,
        probe: ProbeCell,
        attempt: Option<&str>,
        outcome: String,
        route_use: DecisionRouteUse,
    ) -> Self {
        Self {
            at: unix_millis(),
            attempt: attempt.map(str::to_string),
            task: format!("{task:016x}"),
            rubric_version: DECISION_RUBRIC_VERSION,
            model: None,
            outcome,
            elapsed_ms: 0,
            retries: 0,
            cached: false,
            input_tokens: None,
            route_use,
            requests: Some(0),
            redacted_lines: Some(0),
            hedge_delay_ms: None,
            hedge_fired: None,
            hedge_won: None,
            loser_ms: None,
            probe,
            jev: None,
        }
    }
}

pub(super) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// The ledger's axes for a verdict: each token and its probability by name.
pub(super) fn judged_axes(verdict: &DecisionVerdict) -> BTreeMap<String, JudgedAxis> {
    verdict
        .readings()
        .iter()
        .map(|reading| {
            let axis = reading.axis;
            let judged = JudgedAxis {
                choice: axis.tokens[reading.position].to_string(),
                probabilities: axis
                    .tokens
                    .iter()
                    .zip(reading.probabilities)
                    .map(|(token, probability)| ((*token).to_string(), *probability))
                    .collect(),
                confidence: reading.confidence,
            };
            (axis.name.to_string(), judged)
        })
        .collect()
}

/// A task's judgment as this process remembers it. The key carries the rubric
/// version and the requested model, so a judgment made under other words or by
/// another model is never recalled for this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    task: u64,
    rubric: u32,
    model: &'static str,
}

#[derive(Debug, Clone)]
struct Remembered {
    model: String,
    verdict: DecisionVerdict,
    jev: BTreeMap<String, JudgedAxis>,
}

fn memo() -> &'static Mutex<HashMap<MemoKey, Remembered>> {
    static MEMO: OnceLock<Mutex<HashMap<MemoKey, Remembered>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Jev's distribution confidence is not calibrated accuracy. Active mode
/// therefore gives a validated verdict at most the conservative Medium fusion
/// authority — risk cannot fall, complexity cannot fall, and complexity can
/// rise by at most one band — and only when the verdict is more likely than
/// not on every axis ([`runtime::ROUTE_TRUST_FLOOR`]); under that it is a
/// guess the reader does not back, and the deterministic assessment stands.
fn active_assessment(verdict: &DecisionVerdict) -> ProbeAssessment {
    ProbeAssessment {
        complexity: verdict.complexity.choice,
        risk: verdict.risk.choice,
        confidence: verdict.route_confidence(),
        intent: verdict.intent.choice,
    }
}

/// One task of a batch, owned: it outlives the caller's borrow. `state` is the
/// whole task, which the door withholds from and cuts.
struct Shot {
    task: u64,
    state: String,
    probe: ProbeCell,
}

struct Judgment {
    task: u64,
    row: DecisionShadowRow,
    assessment: Option<ProbeAssessment>,
}

/// Everything one probe batch's shadow carries off the calling thread.
struct ShadowBatch {
    settings: runtime::ConfigLoader,
    cwd: PathBuf,
    /// Resolved before the batch detaches — see `fire`.
    ledger: PathBuf,
    config: Result<SystemOneConfig, SystemOneFailure>,
    attempt: Option<String>,
    deadline: Duration,
    shots: Vec<Shot>,
}

/// One sampled task of an active batch, owned: the words the probe reads and
/// the judgment the turn already acted on, copied from its row — never asked
/// again.
struct ControlShot {
    task: u64,
    description: String,
    prompt: String,
    jev: BTreeMap<String, JudgedAxis>,
}

/// Everything one active batch's control sample carries off the calling
/// thread. As with `ShadowBatch`, every path is resolved before it detaches.
pub(super) struct ControlBatch {
    ledger: PathBuf,
    attempt: Option<String>,
    shots: Vec<ControlShot>,
}

/// What the active road hands back: one assessment per original slot, and
/// the control sample it drew from the rows it just wrote.
pub(super) struct Active {
    /// Failed slots are left empty for the caller's unchanged chat-probe
    /// fallback.
    pub(super) assessments: Vec<Option<ProbeAssessment>>,
    /// The tasks whose probe is to run once more, detached, beside the
    /// judgment they were routed on. `None` when the batch drew none.
    pub(super) control: Option<ControlBatch>,
}

/// A task slot the funnel filled in for nothing — never probed, never judged.
fn is_blank_task(description: &str, prompt: &str) -> bool {
    description.trim().is_empty() && prompt.trim().is_empty()
}

/// Fire the shadow for one probe batch, detached, and hand back its task.
///
/// `probed` holds the probe's verdicts aligned with `tasks`; an empty task (a
/// `None`) was never probed and is never judged, and a task the batch names
/// twice is judged — and written — once. `None` when there is
/// nothing to judge, when an ablation holds the shadow out, or when the
/// working directory — where the settings and the ledger are — cannot be read.
pub(super) fn fire(
    tasks: &[(&str, &str)],
    probed: &[Option<ProbeSlot>],
    attempt: &str,
    deadline: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    if probed.iter().all(Option::is_none) {
        return None;
    }
    if telemetry::attest_ablated(telemetry::HarnessFeature::DecisionShadow) {
        return None;
    }
    // A task the batch names twice is judged once: both calls would start
    // before either could be recalled, and the second only bills the same words.
    let mut named = HashSet::with_capacity(probed.len());
    let shots: Vec<Shot> = tasks
        .iter()
        .zip(probed)
        .filter_map(|((description, prompt), slot)| {
            let slot = slot.as_ref().filter(|slot| named.insert(slot.fingerprint))?;
            Some(Shot {
                task: slot.fingerprint,
                state: runtime::rubric_task_whole(description, prompt),
                probe: ProbeCell::from_verdict(slot.verdict),
            })
        })
        .collect();
    let Ok(cwd) = std::env::current_dir() else {
        for _ in &shots {
            telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, FAIL_SETTINGS_UNAVAILABLE);
        }
        return None;
    };
    draft_labels(&cwd, tasks, &shots);
    // Every path this batch will use is resolved NOW, while the process still
    // stands where the caller does. The batch runs detached: by the time it
    // writes, the environment that answers "where does this project's state
    // live" may have been put back by whoever changed it — and a test's rows
    // then land in a person's ledger. Thirty of them did (2026-09-19).
    let batch = ShadowBatch {
        settings: runtime::ConfigLoader::default_for(&cwd),
        ledger: decision_shadow_path(&cwd),
        cwd,
        config: SystemOneConfig::from_env(),
        attempt: Some(attempt.trim()).filter(|attempt| !attempt.is_empty()).map(str::to_string),
        deadline,
        shots,
    };
    Some(shared_agent_runtime().spawn(run_shadow_batch(batch)))
}

/// In actual-use mode, judge all unique non-empty tasks concurrently and
/// return one assessment per original slot. `None` means this is not `on`
/// mode; `Some` means active mode owned the attempt, with failed slots left
/// empty for the caller's unchanged chat-probe fallback — and with the
/// control sample the caller fires once routing is settled
/// ([`fire_control`]).
pub(super) fn active_assessments(
    tasks: &[(&str, &str)],
    attempt: &str,
    deadline: Duration,
) -> Option<Active> {
    let cwd = std::env::current_dir().ok()?;
    let settings = runtime::ConfigLoader::default_for(&cwd);
    let ledger = decision_shadow_path(&cwd);
    let judged = super::settings::merged_settings_root_from(&settings);
    let mode = decision_shadow_mode_from(&settings)?;
    // `auto` acts on the standing its own ledger recorded (§4): the judge
    // wrote a rise there when the window cleared every line, and reading it
    // back here is what makes `auto` a word that decides rather than a second
    // spelling of `shadow`. A person's `on` needs no ledger to say so.
    if !mode.applies_with(raised_here(&cwd)) {
        return None;
    }
    let mut results = vec![None; tasks.len()];
    if telemetry::attest_ablated(telemetry::HarnessFeature::DecisionShadow) {
        return Some(Active { assessments: results, control: None });
    }
    let mut named = HashSet::with_capacity(tasks.len());
    let shots: Vec<Shot> = tasks
        .iter()
        .filter_map(|(description, prompt)| {
            if is_blank_task(description, prompt) {
                return None;
            }
            let task = super::probe_exec::task_fingerprint(description, prompt);
            named.insert(task).then(|| Shot {
                task,
                state: runtime::rubric_task_whole(description, prompt),
                probe: ProbeCell::Failed(PROBE_NOT_RUN.to_string()),
            })
        })
        .collect();
    if shots.is_empty() {
        return Some(Active { assessments: results, control: None });
    }
    // Both roads draft, because both judge: a seat in `on` never reaches
    // `fire` at all, and drafting only there left a person who had turned the
    // seat up with no labels to write — the one mode that makes the rule
    // matter was the one mode it did not run in (2026-09-19).
    draft_labels(&cwd, tasks, &shots);
    let attempt = Some(attempt.trim()).filter(|attempt| !attempt.is_empty());
    let door = JevDoor::open(&cwd);
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    let judgments = api::sync_bridge::run_blocking(futures_util::future::join_all(
        shots
            .into_iter()
            .map(|shot| judge(&door, client.as_ref(), shot, attempt, deadline, true)),
    ));
    let mut by_task = HashMap::with_capacity(judgments.len());
    // The sample is drawn from what was judged, not from what was asked: a
    // control row compares the probe with a judgment, and a task the judgment
    // failed on has nothing for it to stand beside.
    let mut sampled: HashMap<u64, BTreeMap<String, JudgedAxis>> = HashMap::new();
    let rows: Vec<DecisionShadowRow> = judgments
        .into_iter()
        .map(|judgment| {
            by_task.insert(judgment.task, judgment.assessment);
            if control_sampled(judgment.task) {
                if let Some(jev) = judgment.row.jev.clone() {
                    sampled.insert(judgment.task, jev);
                }
            }
            judgment.row
        })
        .collect();
    write_rows(&ledger, judged.as_ref(), &rows);
    for (slot, (description, prompt)) in results.iter_mut().zip(tasks) {
        if is_blank_task(description, prompt) {
            continue;
        }
        *slot = by_task
            .get(&super::probe_exec::task_fingerprint(description, prompt))
            .copied()
            .flatten();
    }
    // The words leave with the sample, because this is the last place they
    // are in hand (the ledger keeps fingerprints); `remove` is what judges a
    // task the batch named twice once here too.
    let control_shots: Vec<ControlShot> = tasks
        .iter()
        .filter_map(|(description, prompt)| {
            let task = super::probe_exec::task_fingerprint(description, prompt);
            let jev = sampled.remove(&task)?;
            Some(ControlShot {
                task,
                description: (*description).to_string(),
                prompt: (*prompt).to_string(),
                jev,
            })
        })
        .collect();
    let control = (!control_shots.is_empty()).then(|| ControlBatch {
        ledger,
        attempt: attempt.map(str::to_string),
        shots: control_shots,
    });
    Some(Active { assessments: results, control })
}

/// Read the setting, judge every shadow shot concurrently, and write the rows.
async fn run_shadow_batch(batch: ShadowBatch) {
    let ShadowBatch { settings, cwd, ledger, config, attempt, deadline, shots } = batch;
    let judged = super::settings::merged_settings_root_from(&settings);
    // The door opens only for a mode that asks: a switched-off shadow reads its
    // one setting and nothing else.
    let read = tokio::task::spawn_blocking(move || {
        decision_shadow_mode_from(&settings).map(|mode| (mode, mode.asks().then(|| JevDoor::open(&cwd))))
    })
    .await
    .ok()
    .flatten();
    let Some((mode, door)) = read else {
        for _ in &shots {
            telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, FAIL_SETTINGS_UNAVAILABLE);
        }
        return;
    };
    // Record-only modes judge here; `off` asks nothing, and `on` is the active
    // path's, so a mode that turned `on` since the funnel looked is declined.
    let Some(door) = door.filter(|_| !mode.applies()) else {
        for _ in &shots {
            telemetry::attest_declined(telemetry::HarnessFeature::DecisionShadow, mode.key());
        }
        return;
    };
    let client = config.ok().map(SystemOneConfig::into_client);
    let judgments: Vec<Judgment> = futures_util::future::join_all(
        shots
            .into_iter()
            .map(|shot| judge(&door, client.as_ref(), shot, attempt.as_deref(), deadline, false)),
    )
    .await;
    let rows: Vec<DecisionShadowRow> = judgments.into_iter().map(|judgment| judgment.row).collect();
    let _ = tokio::task::spawn_blocking(move || write_rows(&ledger, judged.as_ref(), &rows)).await;
}

/// Fire an active batch's control sample, detached, and hand back its task.
///
/// Called by the funnel once routing is settled, because the probe needs
/// what the funnel has and the shadow does not: the inventory and the
/// parent model the probe's own model is resolved from. The same detach as
/// [`fire`]; the batch's ledger was resolved by the active road before it
/// wrote its rows.
pub(super) fn fire_control(
    batch: ControlBatch,
    inventory: &runtime::ModelInventory,
    parent_model: &str,
) -> tokio::task::JoinHandle<()> {
    shared_agent_runtime().spawn(run_control_batch(batch, inventory.clone(), parent_model.to_string()))
}

/// Run the chat probe for the sampled tasks and append one control row each
/// — the same call the routing road makes ([`super::probe_exec`], on its
/// control road), then [`append_shadow_row`] as every other row. Nothing is
/// judged here: the rows hold no request the seat was asked, and the judge
/// reads them at its next boundary.
async fn run_control_batch(
    batch: ControlBatch,
    inventory: runtime::ModelInventory,
    parent_model: String,
) {
    let ControlBatch { ledger, attempt, shots } = batch;
    let words: Vec<(String, String)> =
        shots.iter().map(|shot| (shot.description.clone(), shot.prompt.clone())).collect();
    let billed = attempt.clone().unwrap_or_default();
    // The probe is a blocking executor — it bridges into the shared runtime
    // itself — so it runs off this runtime's workers, as `write_rows` does.
    let probed = tokio::task::spawn_blocking(move || {
        let tasks: Vec<(&str, &str)> =
            words.iter().map(|(description, prompt)| (description.as_str(), prompt.as_str())).collect();
        super::probe_exec::probe_slots(&inventory, &parent_model, &tasks, &billed, ProbeUse::Control)
    })
    .await
    .unwrap_or_default();
    let rows: Vec<DecisionShadowRow> = shots
        .into_iter()
        .zip(probed)
        .filter_map(|(shot, slot)| {
            let slot = slot?;
            let mut row = DecisionShadowRow::new(
                shot.task,
                ProbeCell::from_verdict(slot.verdict),
                attempt.as_deref(),
                OUTCOME_CONTROL.to_string(),
                DecisionRouteUse::Control,
            );
            row.jev = Some(shot.jev);
            Some(row)
        })
        .collect();
    let _ = tokio::task::spawn_blocking(move || {
        for row in &rows {
            let _ = append_shadow_row(&ledger, row, SHADOW_LEDGER_MAX_BYTES);
        }
    })
    .await;
}

fn write_rows(ledger: &Path, settings: Option<&serde_json::Value>, rows: &[DecisionShadowRow]) {
    for row in rows {
        let _ = append_shadow_row(ledger, row, SHADOW_LEDGER_MAX_BYTES);
    }
    judge_ledger(ledger, settings, now_ms());
}

/// The `followed` word of a turn the route stood through: nothing unseated
/// the model the judgment routed to before the turn ended.
pub const ROUTE_STOOD: &str = "stood";

/// How far back a turn's own rows are looked for when its label is written.
///
/// A turn's routing rows are the newest in the ledger when the turn ends —
/// its own judgment, the judgments of the agents it spawned, and the control
/// row a sampled active turn drew — and the widest turn this machine has
/// routed spawned twenty-two workers (run-4275). Two hundred and fifty-six
/// rows holds that turn ten times over without reading a ledger that runs to
/// megabytes back to its first line, which the judge already does once every
/// twenty requests and the label does not have to do again.
const LABEL_LOOKBACK_ROWS: usize = 256;

/// One turn's label: what became of the route the judgment took part in.
///
/// A row of its own rather than a column on the judgment's row, for the
/// reason the skill seat gives (`skill_search::SkillLabelRow`): the
/// judgment's row is written before anybody knows how the turn will end.
/// Named by the turn's attempt twice — as [`jev_ledger::LABEL`], which says
/// what kind of row this is, and as `attempt`, the column the judgment's
/// rows carry, so the judge joins the two on one spelling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteLabelRow {
    pub at: u64,
    /// The attempt this label grades — the value every routing row of that
    /// turn carries as `attempt`.
    pub label: String,
    pub attempt: String,
    /// [`ROUTE_STOOD`], or the door the wire left through
    /// (`SwitchTrigger::as_str`): `quota`, `refusal`, `starvation`, `person`.
    pub followed: String,
    /// The mark the judge counts: the route stood.
    pub agreed: bool,
}

/// Whether a model switch says the route the judgment took part in did not
/// stand: the model it routed to could not serve ([`SwitchTrigger::forced`]
/// — a quota wall, a refusal, an overload shed), or the person named another
/// one themselves. A deep gate's leg borrowing a client for one sub-turn and
/// the step governor moving a rung are the turn's own design, not a route
/// unseated, and leave the label alone.
#[must_use]
pub fn route_unseated_by(trigger: SwitchTrigger) -> bool {
    trigger.forced() || trigger == SwitchTrigger::Person
}

/// Write the routing seat's `agreed` mark for the turn `attempt` names: the
/// route stood, or `unseated` moved the wire off it first.
///
/// Nothing is written when the ledger's tail holds no judgment of that
/// attempt — a turn the seat was never asked about is not one it agreed or
/// disagreed with — and nothing is written twice: a label already standing
/// for the attempt is left as it is. Answers whether a row was written.
#[must_use]
pub fn note_route_followed(cwd: &Path, attempt: &str, unseated: Option<SwitchTrigger>) -> bool {
    let attempt = attempt.trim();
    if attempt.is_empty() {
        return false;
    }
    let ledger = decision_shadow_path(cwd);
    let tail: Vec<serde_json::Value> = last_shadow_lines(&ledger, LABEL_LOOKBACK_ROWS)
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let of_attempt = |row: &serde_json::Value| row.get(ATTEMPT).and_then(serde_json::Value::as_str) == Some(attempt);
    let judged = tail.iter().any(|row| of_attempt(row) && jev_ledger::asked_something(row).is_some());
    let labeled = tail.iter().any(|row| of_attempt(row) && jev_ledger::LABEL.read(row).is_some());
    if !judged || labeled {
        return false;
    }
    let row = RouteLabelRow {
        at: u64::try_from(now_ms()).unwrap_or_default(),
        label: attempt.to_string(),
        attempt: attempt.to_string(),
        followed: unseated.map_or(ROUTE_STOOD, SwitchTrigger::as_str).to_string(),
        agreed: unseated.is_none(),
    };
    append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).is_ok()
}

/// The routing row's attempt column, as the row spells it.
const ATTEMPT: &str = "attempt";

/// The file label drafts are appended to, beside the ledger they will be
/// joined to.
pub const LABEL_DRAFTS_FILE: &str = "decision-labels.draft.jsonl";

/// Write down the words each judged task was judged on, for a person to put
/// labels against (`smart.jev.labelDrafts`).
///
/// Here and nowhere else, because this is the last place the words exist: the
/// ledger keeps a fingerprint and no text, on purpose, and the transcripts on
/// this machine held none of the ten judged tasks either. Each line is already
/// the shape `zo decision-shadow eval --labels` reads — the task as the probe
/// read it, and one empty slot per judged axis, named from the rubric so a new
/// axis arrives in the draft with it.
///
/// A fingerprint already drafted is not drafted again: a person's filled-in
/// answer must not be buried under a second blank copy of the same task.
fn draft_labels(cwd: &Path, tasks: &[(&str, &str)], shots: &[Shot]) {
    let Some(root) = super::settings::merged_settings_root(cwd) else {
        return;
    };
    if !promote::label_drafts_wanted(&root) {
        return;
    }
    let drafts = shadow_ledger_path(cwd, LABEL_DRAFTS_FILE);
    let already: std::collections::HashSet<u64> = super::jev_summary::read_rows(&drafts)
        .iter()
        .filter_map(|row| row.get(TASK).and_then(serde_json::Value::as_u64))
        .collect();
    for (description, prompt) in tasks {
        let task = super::probe_exec::task_fingerprint(description, prompt);
        if already.contains(&task) || !shots.iter().any(|shot| shot.task == task) {
            continue;
        }
        let mut row = serde_json::json!({
            TASK: task,
            "description": description,
            "prompt": prompt,
        });
        for axis in runtime::judged_axes() {
            row[axis.name] = serde_json::Value::String(String::new());
        }
        let _ = append_shadow_row(&drafts, &row, SHADOW_LEDGER_MAX_BYTES);
    }
}

/// Judge the seat on what it has just written, and write down a rise or a
/// fall (docs/design/jev-settings-20260917.md §4).
///
/// Once every [`zerocode_core::jev::summary::JUDGED_EVERY_ROWS`] requests and not
/// at the end of every turn
/// ([`promote::judgment_due`]): a bound that moved on every row would rise and
/// fall on a single answer. The window is the last requests a floor of the
/// seat's can be cleared on
/// ([`zerocode_core::jev::summary::rows_that_can_clear_forgiving`]), the
/// standing is the last transition this same file recorded, and nothing is
/// written unless the answer changed — a ledger of "still recording" every
/// twenty rows is a ledger nobody reads.
///
/// The ledger and the settings are handed in, both resolved before the batch
/// that calls this detached: the environment that answers where either lives
/// may have moved by now, and a test that had to pin it would be racing every
/// other test in this binary.
pub fn judge_ledger(
    ledger: &Path,
    settings: Option<&serde_json::Value>,
    now_ms: i64,
) -> Option<Verdict> {
    let rows = super::jev_summary::read_rows(ledger);
    // Two clocks, and both of them the table's: the lines are judged once
    // every window because a bound that moved on every row would rise and
    // fall on a single answer, and an acting seat that has fallen back three
    // times running stops now rather than after nineteen more requests
    // nobody will get an answer to. `promote::judgment_due` holds both, so
    // this seat and the window's seats cannot keep different time.
    if !promote::judgment_due(&ROUTING, &rows) {
        return None;
    }
    let judged = judge_rows(&rows, settings)?;
    if let Some(row) = promote::transition_row(now_ms, judged.verdict, &judged.window) {
        let _ = append_shadow_row(ledger, &row, SHADOW_LEDGER_MAX_BYTES);
    }
    Some(judged.verdict)
}

/// What the judge said of a ledger's rows — the core's own reading, so the
/// window's seats and this one carry the same shape to the screen.
pub use zerocode_core::jev::promote::Judged;

/// Judge the routing seat on a ledger's rows — the one reading of the
/// evidence, which the judge that writes transitions and the summary that
/// shows a person the same numbers both take, so the screen cannot say "hold"
/// on one window while the ledger rose on another (it did: the summary
/// judged the week with no labels, the judge the last twenty with them,
/// 2026-09-20). `None` for a ledger of a seat that never rises.
#[must_use]
pub fn judge_rows(rows: &[serde_json::Value], settings: Option<&serde_json::Value>) -> Option<Judged> {
    let floor = ROUTING.answer_floor_permille?;
    let agreement_floor = ROUTING.agreement_floor_permille?;
    let deadline_ms = ROUTING.apply_deadline_ms?;
    let window_wanted = promote::window_wanted_for(&ROUTING)?;
    let held = jev_ledger::last_asked(rows, window_wanted);
    let window = jev_ledger::summarize_rows(held.iter().copied(), i64::MIN);
    let (compared, control_rows) = with_control_rows(rows, &held);
    let agreement = agreement_in(&compared);
    let verdict = promote::judge(
        promote::stand_from(rows),
        &promote::Evidence {
            window: &window,
            floor_permille: floor,
            deadline_ms,
            agreement_floor_permille: agreement_floor,
            agreement,
            agreement_rows_wanted: ROUTING
                .agreement_rows_wanted
                .unwrap_or(zerocode_core::jev::A_WINDOW_OF_COMPARISONS),
            window_forgives: ROUTING.window_forgives.unwrap_or(0),
            labels: labels_standing(settings, rows),
            fallbacks_in_a_row: jev_summary::failures_in_a_row(rows),
        },
    );
    Some(Judged { verdict, window, window_wanted, agreement, control_rows })
}

/// The window's rows and, after them, every row joined to them for the
/// agreement — the control row of a task the window holds, and the turn
/// label of an attempt it holds ([`RouteLabelRow`]) — with how many control
/// rows joined.
///
/// Joined by task and by attempt rather than taken whole: a control row
/// stands for the active row it was drawn beside, a label for the turn its
/// rows were judged in, and one whose row has left the window has left with
/// it. A window of `applied` rows compares nothing on its own (its probe
/// cell is `not_run`); these are where its comparisons come from.
fn with_control_rows<'a>(
    rows: &'a [serde_json::Value],
    held: &[&'a serde_json::Value],
) -> (Vec<&'a serde_json::Value>, usize) {
    let tasks: HashSet<&str> = held
        .iter()
        .filter_map(|row| row.get(TASK).and_then(serde_json::Value::as_str))
        .collect();
    let attempts: HashSet<&str> = held
        .iter()
        .filter_map(|row| row.get(ATTEMPT).and_then(serde_json::Value::as_str))
        .collect();
    let mut compared = held.to_vec();
    let mut joined = 0;
    for row in rows {
        if jev_ledger::is_control_row(row) {
            if row.get(TASK).and_then(serde_json::Value::as_str).is_some_and(|task| tasks.contains(task)) {
                compared.push(row);
                joined += 1;
            }
            continue;
        }
        let is_label = jev_ledger::LABEL.read(row).is_some() && jev_ledger::AGREED.read(row).is_some();
        if is_label
            && row
                .get(ATTEMPT)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|attempt| attempts.contains(attempt))
        {
            compared.push(row);
        }
    }
    (compared, joined)
}

/// How often the judgment named what the reader it would replace named: one
/// comparison per judged axis of each row where both the judgment and the
/// chat probe answered, and one per turn label that carries the seat's
/// `agreed` mark ([`RouteLabelRow`] — the route stood, or did not).
///
/// Read from the rows the window was counted from, and the rows joined to
/// them (`with_control_rows`), so the share and the bound stand on the same
/// requests. A row the probe timed out on (eleven of this machine's 28)
/// compares nothing and counts nothing, and so does an active row on its
/// own: its probe cell is `not_run`, and its control row is where the
/// comparison is.
#[must_use]
pub fn agreement_in(rows: &[&serde_json::Value]) -> promote::Agreement {
    let mut agreement = promote::Agreement::default();
    for row in rows {
        if let Some(agreed) = jev_ledger::AGREED.read(row).and_then(serde_json::Value::as_bool) {
            agreement.compared += 1;
            agreement.agreed += usize::from(agreed);
            continue;
        }
        let Ok(row) = serde_json::from_value::<DecisionShadowRow>((*row).clone()) else {
            continue;
        };
        let Some(jev) = row.jev.as_ref() else {
            continue;
        };
        for axis in runtime::judged_axes() {
            let (Some(probe), Some(judged)) = (row.probe.token(axis), jev.get(axis.name)) else {
                continue;
            };
            agreement.compared += 1;
            agreement.agreed += usize::from(judged.choice == probe);
        }
    }
    agreement
}

/// What the labels a person wrote say about both readers, when they named a
/// file and it can be read. Absent labels are not a bad verdict: §4 holds the
/// seat and the screen asks for twenty.
fn labels_standing(
    settings: Option<&serde_json::Value>,
    rows: &[serde_json::Value],
) -> Option<promote::Labels> {
    let path = promote::labels_path_in(settings?)?;
    let text = std::fs::read_to_string(path).ok()?;
    let judged: Vec<DecisionShadowRow> = rows
        .iter()
        .filter_map(|row| serde_json::from_value(row.clone()).ok())
        .collect();
    let evaluation = super::decision_report::evaluate_decision_labels(&text, &judged).ok()?;
    Some(promote::Labels {
        compared: evaluation.matched,
        judgment_right: evaluation.axes.iter().map(|axis| axis.judgment.correct).sum(),
        probe_right: evaluation.axes.iter().map(|axis| axis.probe.correct).sum(),
    })
}

pub(super) fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// One task's row and reusable typed assessment: recalled from the memo,
/// refused at the door, or asked and checked.
async fn judge(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    shot: Shot,
    attempt: Option<&str>,
    deadline: Duration,
    active: bool,
) -> Judgment {
    let key = MemoKey { task: shot.task, rubric: DECISION_RUBRIC_VERSION, model: SYSTEMONE_MODEL };
    let recalled = memo().lock().ok().and_then(|memo| memo.get(&key).cloned());
    if let Some(remembered) = recalled {
        telemetry::attest_fired(telemetry::HarnessFeature::DecisionShadow);
        let assessment = active.then(|| active_assessment(&remembered.verdict));
        let route_use = if active { DecisionRouteUse::Applied } else { DecisionRouteUse::RecordOnly };
        let mut row = DecisionShadowRow::new(
            shot.task,
            shot.probe,
            attempt,
            OUTCOME_ANSWERED.to_string(),
            route_use,
        );
        row.cached = true;
        row.model = Some(remembered.model);
        row.jev = Some(remembered.jev);
        return Judgment { task: shot.task, row, assessment };
    }
    let not_applied = if active { DecisionRouteUse::Fallback } else { DecisionRouteUse::RecordOnly };
    let request = runtime::decision_request(SYSTEMONE_MODEL, &shot.state);
    let Some(body) = jev_gate::body_of(&request) else {
        let failure = SystemOneFailure::InvalidRequest;
        telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, failure.token());
        let row = DecisionShadowRow::new(shot.task, shot.probe, attempt, failure.ledger_token(), not_applied);
        return Judgment { task: shot.task, row, assessment: None };
    };
    let (cleared, client) = match (door.pass(&ROUTING, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        // The door refuses a keyless request before anything else it asks.
        (passed, _) => {
            let refused = passed.err().unwrap_or(Refused::NoKey);
            telemetry::attest_declined(telemetry::HarnessFeature::DecisionShadow, refused.token());
            let row = DecisionShadowRow::new(shot.task, shot.probe, attempt, refused.token().to_string(), not_applied);
            return Judgment { task: shot.task, row, assessment: None };
        }
    };
    let withheld = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    let hedge = door.hedge_now(&ROUTING, deadline, active);
    let call = jev_gate::send(client, cleared, deadline, hedge).await;
    // Read, not taken: the call is read for its answer here and for what it
    // cost at the end, and a judgment asked twice has more to say about the
    // cost than the answer does.
    let judged = match &call.outcome {
        Err(failure) => Err((*failure, None)),
        Ok(response) => match runtime::validate_decision(response) {
            Ok(verdict) => {
                let jev = judged_axes(&verdict);
                Ok((response, verdict, jev))
            }
            Err(_) => Err((SystemOneFailure::Schema, Some(response))),
        },
    };
    let (mut row, assessment) = match judged {
        Ok((response, verdict, jev)) => {
            telemetry::attest_fired(telemetry::HarnessFeature::DecisionShadow);
            if let Ok(mut memo) = memo().lock() {
                remember_bounded(
                    &mut memo,
                    vec![(
                        key,
                        Remembered {
                            model: response.model.clone(),
                            verdict: verdict.clone(),
                            jev: jev.clone(),
                        },
                    )],
                );
            }
            let assessment = active.then(|| active_assessment(&verdict));
            let route_use = if active { DecisionRouteUse::Applied } else { DecisionRouteUse::RecordOnly };
            let mut row = DecisionShadowRow::new(
                shot.task,
                shot.probe,
                attempt,
                OUTCOME_ANSWERED.to_string(),
                route_use,
            );
            row.model = Some(response.model.clone());
            row.input_tokens = Some(response.usage.input_tokens);
            row.jev = Some(jev);
            (row, assessment)
        }
        Err((failure, response)) => {
            telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, failure.token());
            let mut row = DecisionShadowRow::new(
                shot.task,
                shot.probe,
                attempt,
                failure.ledger_token(),
                not_applied,
            );
            // A response that arrived and failed its checks still billed.
            if let Some(response) = response {
                row.model = Some(response.model.clone());
                row.input_tokens = Some(response.usage.input_tokens);
            }
            (row, None)
        }
    };
    row.spent(&call, hedge, withheld);
    Judgment { task: shot.task, row, assessment }
}
