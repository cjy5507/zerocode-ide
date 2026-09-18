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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SYSTEMONE_MODEL};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::ROUTING;
use runtime::{
    DecisionVerdict, ProbeAssessment, RouteConfidence, RubricAxis,
    DECISION_RUBRIC_VERSION,
};
use serde::{Deserialize, Serialize};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, ProbeSlot, PROBE_TIMEOUT};
use super::settings::decision_shadow_mode_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};
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
pub(super) const DECISION_ACTIVE_DEADLINE: Duration = Duration::from_millis(1_500);

/// An active row uses this probe cell when no chat probe ran. It is an explicit
/// absence, not a fabricated chat-model verdict, and comparison metrics skip it.
const PROBE_NOT_RUN: &str = "not_run";

/// Failure: the merged settings could not be read, so nothing was sent.
const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

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
/// long the call took and how often it was re-sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemOneCheck {
    pub outcome: Result<String, CheckFailure>,
    pub elapsed: Duration,
    pub retries: u32,
}

/// Put the shadow's own question about [`KEY_CHECK_TASK`] to System One, once:
/// the key this process would use (`TYPESAFE_API_KEY`, or the one the window's
/// settings keep), the door's key check, the rubric's request, the shadow's
/// deadline and its answer check — so a key that passes here is one the shadow
/// can use, and one that fails is named by the same token a ledger row would
/// carry. The memo is not consulted: a check that recalled an answer would
/// prove nothing about the key.
pub async fn check_system_one() -> SystemOneCheck {
    let unsent = |failure| SystemOneCheck { outcome: Err(failure), elapsed: Duration::ZERO, retries: 0 };
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
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
    SystemOneCheck { outcome, elapsed: call.elapsed, retries: call.retries }
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
    /// answered or before a failed Jev judgment falls back.
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
fn judged_axes(verdict: &DecisionVerdict) -> BTreeMap<String, JudgedAxis> {
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
/// therefore gives every validated verdict the existing conservative Medium
/// fusion authority: risk cannot fall, complexity cannot fall, and complexity
/// can rise by at most one band.
fn active_assessment(verdict: &DecisionVerdict) -> ProbeAssessment {
    ProbeAssessment {
        complexity: verdict.complexity.choice,
        risk: verdict.risk.choice,
        confidence: RouteConfidence::Medium,
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
    ledger: PathBuf,
    config: Result<SystemOneConfig, SystemOneFailure>,
    attempt: Option<String>,
    deadline: Duration,
    shots: Vec<Shot>,
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
/// empty for the caller's unchanged chat-probe fallback.
pub(super) fn active_assessments(
    tasks: &[(&str, &str)],
    attempt: &str,
    deadline: Duration,
) -> Option<Vec<Option<ProbeAssessment>>> {
    let cwd = std::env::current_dir().ok()?;
    let mode = decision_shadow_mode_from(&runtime::ConfigLoader::default_for(&cwd))?;
    if !mode.applies() {
        return None;
    }
    let mut results = vec![None; tasks.len()];
    if telemetry::attest_ablated(telemetry::HarnessFeature::DecisionShadow) {
        return Some(results);
    }
    let mut named = HashSet::with_capacity(tasks.len());
    let shots: Vec<Shot> = tasks
        .iter()
        .filter_map(|(description, prompt)| {
            if description.trim().is_empty() && prompt.trim().is_empty() {
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
        return Some(results);
    }
    let attempt = Some(attempt.trim()).filter(|attempt| !attempt.is_empty());
    let door = JevDoor::open(&cwd);
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    let judgments = api::sync_bridge::run_blocking(futures_util::future::join_all(
        shots
            .into_iter()
            .map(|shot| judge(&door, client.as_ref(), shot, attempt, deadline, true)),
    ));
    let mut by_task = HashMap::with_capacity(judgments.len());
    let rows: Vec<DecisionShadowRow> = judgments
        .into_iter()
        .map(|judgment| {
            by_task.insert(judgment.task, judgment.assessment);
            judgment.row
        })
        .collect();
    write_rows(&decision_shadow_path(&cwd), &rows);
    for (slot, (description, prompt)) in results.iter_mut().zip(tasks) {
        if description.trim().is_empty() && prompt.trim().is_empty() {
            continue;
        }
        *slot = by_task
            .get(&super::probe_exec::task_fingerprint(description, prompt))
            .copied()
            .flatten();
    }
    Some(results)
}

/// Read the setting, judge every shadow shot concurrently, and write the rows.
async fn run_shadow_batch(batch: ShadowBatch) {
    let ShadowBatch { settings, cwd, ledger, config, attempt, deadline, shots } = batch;
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
    let _ = tokio::task::spawn_blocking(move || write_rows(&ledger, &rows)).await;
}

fn write_rows(ledger: &Path, rows: &[DecisionShadowRow]) {
    for row in rows {
        let _ = append_shadow_row(ledger, row, SHADOW_LEDGER_MAX_BYTES);
    }
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
