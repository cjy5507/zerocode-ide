//! The step effort governor's seat and ledger — the tools-layer half of
//! `runtime::conversation::step_effort` (docs/design/zo-step-effort-governor-20260921.md).
//!
//! The runtime decides a request's effort per step and tells an observer;
//! this module is that observer's pen and the seat the runtime asks. Three
//! kinds of line share one file, `step-effort-zo.jsonl`, beside the routing
//! seat's: the governor's own `step` rows (a decision each; no `outcome`, so
//! the seat's counter never reads one as a request), the `judgment` rows a
//! Jev question left (the same vocabulary every other seat's rows carry), and
//! the `label` rows written one step after a judgment was consulted — the
//! seat's own `agreed`, which is what `auto` rises on.
//!
//! Every judgment goes through the Jev door (`jev_gate`) as the routing seat's
//! does: no key, Jev off, a workspace nobody consented to or a spent day is a
//! row that names the refusal and sends nothing. The question is detached —
//! the runtime reads the answer at its next step — so a slow answer costs the
//! turn nothing, and one that comes after the next step is simply late.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use api::{SystemOneConfig, SystemOneFailure, SYSTEMONE_MODEL};
use runtime::{RouteTaskComplexity, StepAskContext, StepEffortSeat, StepEvent, StepJudgment};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::promote;
use zerocode_core::jev::{JevMode, SMART_SETTINGS_KEY, ZO_STEP_EFFORT};

use super::decision_shadow::{judged_axes, JudgedAxis, OUTCOME_ANSWERED};
use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, task_fingerprint};
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};
use crate::misc_tools::agent_tools::shared_agent_runtime;

/// The governor's ledger file — the use table's name for the seat's ledger.
pub const STEP_EFFORT_FILE: &str = ZO_STEP_EFFORT.ledger;

/// The settings key the seat's mode is read from, under `smart`.
pub const STEP_EFFORT_SETTING: &str = ZO_STEP_EFFORT.setting;

/// The word a judgment row carries as its kind.
pub const JUDGMENT_ROW_KIND: &str = "judgment";

/// The wall a step judgment is held to: an answer past it is late for the
/// next request, which is the only request it could have moved.
pub const STEP_JUDGMENT_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::STEP_EFFORT_APPLY_DEADLINE_MS);

/// Where a project's step effort ledger lives.
#[must_use]
pub fn step_effort_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, STEP_EFFORT_FILE)
}

/// How `smart.zoStepEffort` reads — told apart from an absent key, because the
/// two mean different things to the governor: a word is the seat's mode
/// (the use table's reading, where anything unknown is `off`), and no word
/// at all leaves the table running in shadow and the seat unasked
/// (docs/design/zo-step-effort-governor-20260921.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepEffortWord {
    /// The key is not in the merged settings.
    Absent,
    /// The key is set; the mode is the use table's reading of it.
    Set(JevMode),
}

impl StepEffortWord {
    /// Whether the governor's table runs at all: everything but a written
    /// `off`.
    #[must_use]
    pub fn governs(self) -> bool {
        match self {
            Self::Absent => true,
            Self::Set(mode) => mode.asks(),
        }
    }

    /// Whether the seat is asked: only a written mode that asks.
    #[must_use]
    pub fn asks(self) -> bool {
        matches!(self, Self::Set(mode) if mode.asks())
    }

    /// Whether the requests carry the governor's effort, given the seat's
    /// standing for `auto`.
    #[must_use]
    pub fn applies_with(self, raised: bool) -> bool {
        matches!(self, Self::Set(mode) if mode.applies_with(raised))
    }
}

/// `smart.zoStepEffort` in a merged settings document.
#[must_use]
pub fn step_effort_word_in(root: &Value) -> StepEffortWord {
    match root
        .get(SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get(ZO_STEP_EFFORT.setting))
    {
        None => StepEffortWord::Absent,
        value @ Some(_) => StepEffortWord::Set(ZO_STEP_EFFORT.mode_of(value)),
    }
}

/// `smart.zoStepEffort` for a project; `None` when the merged settings cannot
/// be read, which installs no governor at all.
#[must_use]
pub fn step_effort_word(cwd: &Path) -> Option<StepEffortWord> {
    super::settings::merged_settings_root(cwd).map(|root| step_effort_word_in(&root))
}

/// Whether the seat has been raised to acting by its own evidence, read from
/// the ledger it writes.
#[must_use]
pub fn step_effort_raised(cwd: &Path) -> bool {
    raised_at(&step_effort_path(cwd))
}

fn raised_at(ledger: &Path) -> bool {
    let rows = super::jev_summary::read_rows(ledger);
    promote::stand_from(&rows) == promote::Stand::Applying
}

/// Append one of the governor's events — a decision, or a progress mark —
/// to the project's ledger.
pub fn record_step_event(cwd: &Path, event: &StepEvent) -> std::io::Result<()> {
    append_shadow_row(&step_effort_path(cwd), event, SHADOW_LEDGER_MAX_BYTES)
}

/// One judgment the seat asked, as the ledger keeps it: the seat table's
/// vocabulary, so the one counter every seat shares reads it unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepJudgmentRow {
    pub kind: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    pub step: u32,
    /// Why the seat was asked: the governor's own word.
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub outcome: String,
    pub elapsed_ms: u64,
    pub retries: u32,
    pub cached: bool,
    pub requests: u32,
    pub redacted_lines: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<std::collections::BTreeMap<String, JudgedAxis>>,
}

impl StepJudgmentRow {
    fn new(ask: &OwnedAsk, outcome: String) -> Self {
        Self {
            kind: JUDGMENT_ROW_KIND.to_string(),
            at: super::decision_shadow::unix_millis(),
            attempt: ask.attempt.clone(),
            step: ask.step,
            why: ask.why.clone(),
            model: None,
            outcome,
            elapsed_ms: 0,
            retries: 0,
            cached: false,
            requests: 0,
            redacted_lines: 0,
            input_tokens: None,
            jev: None,
        }
    }
}

/// One question, owned: it outlives the runtime's borrow.
#[derive(Debug, Clone)]
struct OwnedAsk {
    step: u32,
    why: String,
    state: String,
    attempt: Option<String>,
}

/// What a judgment left for the runtime to read at its next step.
#[derive(Debug, Clone)]
struct Remembered {
    model: String,
    complexity: RouteTaskComplexity,
    jev: std::collections::BTreeMap<String, JudgedAxis>,
}

fn memo() -> &'static Mutex<HashMap<u64, Remembered>> {
    static MEMO: OnceLock<Mutex<HashMap<u64, Remembered>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The seat the host installs on the runtime: asked detached, read at the
/// next step. The answer slot is shared with the detached task that fills
/// it, so the seat itself never has to be cloned into a future.
pub struct StepSeat {
    cwd: PathBuf,
    ledger: PathBuf,
    answer: SlotHandle,
}

impl std::fmt::Debug for StepSeat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepSeat").field("ledger", &self.ledger).finish_non_exhaustive()
    }
}

impl StepSeat {
    /// The seat for a project — its ledger resolved now, while the process
    /// still stands where the caller does (the judgment runs detached).
    #[must_use]
    pub fn open(cwd: &Path) -> Arc<Self> {
        Arc::new(Self {
            cwd: cwd.to_path_buf(),
            ledger: step_effort_path(cwd),
            answer: SlotHandle::default(),
        })
    }
}

impl StepEffortSeat for StepSeat {
    fn ask(&self, ask: &StepAskContext<'_>) {
        if telemetry::attest_ablated(telemetry::HarnessFeature::DecisionShadow) {
            return;
        }
        let owned = OwnedAsk {
            step: ask.step,
            why: ask.why.token().to_string(),
            state: ask.state.to_string(),
            attempt: Some(ask.attempt.trim())
                .filter(|attempt| !attempt.is_empty())
                .map(str::to_string),
        };
        let cwd = self.cwd.clone();
        let ledger = self.ledger.clone();
        let slot = self.answer.clone();
        shared_agent_runtime().spawn(async move {
            let door = tokio::task::spawn_blocking(move || JevDoor::open(&cwd)).await.ok();
            let Some(door) = door else {
                return;
            };
            let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
            let (row, answer) = judge_step(&door, client.as_ref(), &owned).await;
            if let Some(answer) = answer {
                slot.leave(answer);
            }
            let _ = tokio::task::spawn_blocking(move || {
                let _ = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
                let _ = judge_ledger(&ledger, super::decision_shadow::now_ms());
            })
            .await;
        });
    }

    fn take(&self) -> Option<StepJudgment> {
        self.answer.take()
    }
}

/// The answer slot a seat and its detached questions share: written by the
/// task that judged, read once by the runtime at its next step.
#[derive(Clone, Default)]
struct SlotHandle(Arc<Mutex<Option<StepJudgment>>>);

impl SlotHandle {
    fn leave(&self, judgment: StepJudgment) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(judgment);
        }
    }

    fn take(&self) -> Option<StepJudgment> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// Ask the door and the wire about one step, and say what happened: the row
/// for the ledger and, when the judgment answered, the band for the runtime.
async fn judge_step(
    door: &JevDoor,
    client: Option<&api::SystemOneClient>,
    ask: &OwnedAsk,
) -> (StepJudgmentRow, Option<StepJudgment>) {
    let key = task_fingerprint("", &ask.state);
    let recalled = memo().lock().ok().and_then(|memo| memo.get(&key).cloned());
    if let Some(remembered) = recalled {
        telemetry::attest_fired(telemetry::HarnessFeature::DecisionShadow);
        let mut row = StepJudgmentRow::new(ask, OUTCOME_ANSWERED.to_string());
        row.cached = true;
        row.model = Some(remembered.model);
        row.jev = Some(remembered.jev);
        let answer = StepJudgment {
            complexity: remembered.complexity,
            at_step: ask.step,
        };
        return (row, Some(answer));
    }
    let request = runtime::decision_request(SYSTEMONE_MODEL, &ask.state);
    let Some(body) = jev_gate::body_of(&request) else {
        let failure = SystemOneFailure::InvalidRequest;
        telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, failure.token());
        return (StepJudgmentRow::new(ask, failure.ledger_token()), None);
    };
    let (cleared, client) = match (door.pass(&ZO_STEP_EFFORT, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            let refused = passed.err().unwrap_or(Refused::NoKey);
            telemetry::attest_declined(telemetry::HarnessFeature::DecisionShadow, refused.token());
            return (StepJudgmentRow::new(ask, refused.token().to_string()), None);
        }
    };
    let withheld = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    let call = jev_gate::send(client, cleared, STEP_JUDGMENT_DEADLINE, None).await;
    let judged = match &call.outcome {
        Err(failure) => Err(*failure),
        Ok(response) => runtime::validate_decision(response)
            .map(|verdict| (response, verdict))
            .map_err(|_| SystemOneFailure::Schema),
    };
    let (mut row, answer) = match judged {
        Ok((response, verdict)) => {
            telemetry::attest_fired(telemetry::HarnessFeature::DecisionShadow);
            let jev = judged_axes(&verdict);
            let complexity = verdict.complexity.choice;
            if let Ok(mut memo) = memo().lock() {
                remember_bounded(
                    &mut memo,
                    vec![(
                        key,
                        Remembered {
                            model: response.model.clone(),
                            complexity,
                            jev: jev.clone(),
                        },
                    )],
                );
            }
            let mut row = StepJudgmentRow::new(ask, OUTCOME_ANSWERED.to_string());
            row.model = Some(response.model.clone());
            row.input_tokens = Some(response.usage.input_tokens);
            row.jev = Some(jev);
            (
                row,
                Some(StepJudgment {
                    complexity,
                    at_step: ask.step,
                }),
            )
        }
        Err(failure) => {
            telemetry::attest_failed(telemetry::HarnessFeature::DecisionShadow, failure.token());
            let mut row = StepJudgmentRow::new(ask, failure.ledger_token());
            if let Ok(response) = &call.outcome {
                row.model = Some(response.model.clone());
                row.input_tokens = Some(response.usage.input_tokens);
            }
            (row, None)
        }
    };
    row.elapsed_ms = jev_gate::millis(call.elapsed);
    row.retries = call.retries;
    row.requests = call.requests;
    row.redacted_lines = withheld;
    (row, answer)
}

/// Judge the seat on what it has just written, and write down a rise or a
/// fall — the routing seat's cadence (`decision_shadow::judge_ledger`), read
/// through the table's own judge because this seat's agreement is the
/// `agreed` marks its own rows carry, not a probe beside a judgment.
#[must_use]
pub fn judge_ledger(ledger: &Path, now_ms: i64) -> Option<promote::Verdict> {
    let rows = super::jev_summary::read_rows(ledger);
    if !promote::judgment_due(&rows) {
        return None;
    }
    let judged = promote::judge_seat(&ZO_STEP_EFFORT, &rows)?;
    if let Some(row) = promote::transition_row(now_ms, judged.verdict, &judged.window) {
        let _ = append_shadow_row(ledger, &row, SHADOW_LEDGER_MAX_BYTES);
    }
    Some(judged.verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_word_is_told_apart_from_no_word_and_reads_the_tables_modes() {
        assert_eq!(step_effort_word_in(&json!({})), StepEffortWord::Absent);
        assert_eq!(
            step_effort_word_in(&json!({ SMART_SETTINGS_KEY: { STEP_EFFORT_SETTING: "shadow" } })),
            StepEffortWord::Set(JevMode::Shadow)
        );
        assert_eq!(
            step_effort_word_in(&json!({ SMART_SETTINGS_KEY: { STEP_EFFORT_SETTING: "shadwo" } })),
            StepEffortWord::Set(JevMode::Off),
            "a slip is off, as every seat reads it"
        );
        // No word: the table runs and records, the seat is not asked, nothing
        // applies. A written `off` stops the table too.
        let absent = StepEffortWord::Absent;
        assert!(absent.governs() && !absent.asks() && !absent.applies_with(true));
        let off = StepEffortWord::Set(JevMode::Off);
        assert!(!off.governs() && !off.asks() && !off.applies_with(true));
        let shadow = StepEffortWord::Set(JevMode::Shadow);
        assert!(shadow.governs() && shadow.asks() && !shadow.applies_with(true));
        let on = StepEffortWord::Set(JevMode::On);
        assert!(on.governs() && on.asks() && on.applies_with(false));
        let auto = StepEffortWord::Set(JevMode::Auto);
        assert!(auto.governs() && auto.asks());
        assert!(!auto.applies_with(false) && auto.applies_with(true));
    }

    /// Written at an explicit path: `record_step_event(cwd)` resolves the
    /// project's state directory under the config home, and a test that
    /// handed it a temporary cwd would leave a row in the person's own home
    /// (one did, 2026-09-21).
    #[test]
    fn a_decision_row_is_not_a_request_and_a_judgment_row_is() {
        let dir = tempfile::tempdir().expect("tmp");
        let ledger = dir.path().join(STEP_EFFORT_FILE);
        let step = runtime::StepRow {
            kind: runtime::STEP_ROW_KIND,
            at: 1,
            attempt: "s@1".to_string(),
            step: 1,
            model: None,
            band: "medium",
            batch: "none",
            repeats: 0,
            error_streak: 0,
            check_red: false,
            routine_streak: 0,
            delta: 0,
            effort_before: "xhigh",
            effort_after: "xhigh",
            ceiling_before: None,
            ceiling_after: None,
            reason: "working",
            applied: false,
            held: None,
            ask: None,
            jev: None,
            rung_move: None,
        };
        append_shadow_row(&ledger, &StepEvent::Step(Box::new(step)), SHADOW_LEDGER_MAX_BYTES).expect("row");
        append_shadow_row(
            &ledger,
            &StepEvent::Label(runtime::StepLabel {
                kind: runtime::LABEL_ROW_KIND,
                at: 2,
                attempt: "s@1".to_string(),
                step: 1,
                agreed: true,
            }),
            SHADOW_LEDGER_MAX_BYTES,
        )
        .expect("label");
        let ask = OwnedAsk {
            step: 1,
            why: "cadence".to_string(),
            state: "words".to_string(),
            attempt: Some("s@1".to_string()),
        };
        let judgment = StepJudgmentRow::new(&ask, OUTCOME_ANSWERED.to_string());
        append_shadow_row(&ledger, &judgment, SHADOW_LEDGER_MAX_BYTES).expect("judgment");
        let rows = super::super::jev_summary::read_rows(&ledger);
        assert_eq!(rows.len(), 3);
        let asked: Vec<&Value> = rows
            .iter()
            .filter(|row| zerocode_core::jev::summary::asked_something(row).is_some())
            .collect();
        assert_eq!(asked.len(), 1, "only the judgment row is a request");
        assert_eq!(asked[0]["kind"], JUDGMENT_ROW_KIND);
        let agreement = zerocode_core::jev::summary::agreement_since(&rows, i64::MIN);
        assert_eq!((agreement.compared, agreement.agreed), (1, 1));
        assert!(!raised_at(&ledger));
    }

    #[tokio::test]
    async fn a_keyless_question_is_refused_at_the_door_and_leaves_no_answer() {
        let home = tempfile::tempdir().expect("tmp");
        let settings = zerocode_core::jev::door::JevSettings {
            enabled: true,
            workspaces: vec![home.path().display().to_string()],
            daily_requests: None,
        };
        let door = JevDoor::at(settings, home.path(), home.path());
        let ask = OwnedAsk {
            step: 5,
            why: "cadence".to_string(),
            state: "rename one variable\n[step 5] batch=read_only repeats=1 errors_in_a_row=0 check_red=false"
                .to_string(),
            attempt: None,
        };
        let (row, answer) = judge_step(&door, None, &ask).await;
        assert_eq!(answer, None);
        assert_eq!(row.outcome, Refused::NoKey.token());
        assert_eq!(row.kind, JUDGMENT_ROW_KIND);
        assert_eq!((row.step, row.requests, row.cached), (5, 0, false));
        assert!(row.jev.is_none());
    }

    #[test]
    fn the_seat_hands_an_answer_over_once() {
        let dir = tempfile::tempdir().expect("tmp");
        let seat = StepSeat::open(dir.path());
        assert_eq!(seat.take(), None);
        seat.answer.leave(StepJudgment {
            complexity: RouteTaskComplexity::Large,
            at_step: 5,
        });
        assert_eq!(
            seat.take(),
            Some(StepJudgment {
                complexity: RouteTaskComplexity::Large,
                at_step: 5
            })
        );
        assert_eq!(seat.take(), None, "an answer is read once");
    }
}
