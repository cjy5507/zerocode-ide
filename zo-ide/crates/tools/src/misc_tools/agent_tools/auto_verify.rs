//! Verification by default for unattended implementation spawns (design P2,
//! live). The workflow engine already runs a repair loop for its own phases;
//! this brings the same "done means verified" discipline to a plain Agent
//! spawn that writes code while nobody is at the keyboard:
//!
//! - an UNATTENDED implementation spawn that completes gets a verifier
//!   (`code-reviewer`, the workflow verdict schema) bound to it as
//!   `judged_agent`, so its verdict lands on the implementer's model the way
//!   every other verdict does;
//! - a FAILING verdict re-spawns the implementer with the finding carried in
//!   the prompt, up to the difficulty ceiling (`completion_ceiling_for`);
//! - every spawn of the loop — each verifier, each repair — runs under the
//!   implementer's own [`ExecutionContract`], read once from the first
//!   attempt: its checkout, its permission ceiling, its MCP tools and ONE
//!   wall-clock deadline that never restarts; a repair also keeps the model,
//!   effort and route labels it ran on, re-offered to the same resolver, so a
//!   person's pin stays binding and the complexity ceiling stays the one the
//!   router stamped (t-3854);
//! - a repair that leaves the SAME finding standing stops the loop — by the
//!   workflow's own same-finding identity ([`FindingEvidence`]), not a second
//!   similarity rule;
//! - every way the loop ends without a pass — ceiling, repeated finding, an
//!   unusable or unfinished verifier, a spent deadline, a spawn that could not
//!   start — publishes one explicit "NOT verified" receipt to the owning
//!   session. The loop never paints a red result green;
//! - an ATTENDED turn is untouched (Esc is the breaker; the completion-receipt
//!   rule asks once), and `ZO_AUTO_VERIFY=0` opts out entirely.
//!
//! Everything that decides is a pure function over primitives or over the job
//! and completion in hand, with the clock passed in; the two hooks at the
//! bottom only carry out what a plan returned.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use core_types::helper_run::HelperRun;
use runtime::{Attendance, PermissionMode};

use super::completion::AgentCompletion;
use super::spawn::AgentJob;
use super::AgentInput;
use crate::workflow_tools::engine::items::{finding_evidence, FindingEvidence};
use crate::ToolError;

/// Opt-out switch: `0`, `off`, `false` or `no` turns verification by default
/// off for the process. Absent or anything else keeps it on.
pub(crate) const AUTO_VERIFY_ENV: &str = "ZO_AUTO_VERIFY";
/// The reviewer role every auto-spawned verifier runs as.
pub(crate) const VERIFIER_SUBAGENT_TYPE: &str = "code-reviewer";
/// Marker the retry prompt carries so the implementer (and the transcript)
/// can see the round came from a verification finding, not from the person.
pub(crate) const VERIFY_FINDING_MARKER: &str = "[verify finding]";
/// The wording a verdict record uses when the verifier gave no evidence.
const NO_EVIDENCE: &str = "verification failed without stated evidence";
/// Suffix of a stop receipt's id, `<agent id>#verify`: an id no agent owns
/// (the `#starved`/`#message` idiom), so the receipt never answers a wait.
const RECEIPT_ID_SUFFIX: &str = "#verify";
/// The label a stop receipt carries in the owning session.
const RECEIPT_NAME: &str = "auto-verify";

/// What the implementer was launched under — read ONCE, from the first
/// implementation job, and carried unchanged through every verifier and
/// repair of the loop.
///
/// Never re-read from a later job: the job a failing verdict arrives on is the
/// VERIFIER's, whose model, route and budget are the reviewer's own. Rebuilding
/// a repair from it is how a job rooted in another checkout was repaired in the
/// process cwd, how its budget restarted every round, and how a `large` label
/// fell to `Unknown`'s ceiling at round two.
#[derive(Debug, Clone)]
pub(crate) struct ExecutionContract {
    /// The checkout the implementer worked in; `None` = the process cwd, as
    /// it was for the implementer.
    pub cwd: Option<PathBuf>,
    /// The implementer's effective permission mode, re-applied as the PARENT
    /// ceiling of every loop spawn: the reviewer and each repair run no
    /// broader than the implementer did (a clamp to one's own mode is a no-op,
    /// so the repair's harness resolves the same mode it had).
    pub permission_ceiling: Option<PermissionMode>,
    /// When the loop's wall-clock budget runs out: the first implementer's
    /// start plus its budget. `None` = the implementer had no budget either.
    pub deadline: Option<SystemTime>,
    /// The parent-session MCP tools the implementer could call.
    pub mcp_passthrough: Option<crate::registry::McpPassthrough>,
    /// The model the implementer ran on, re-offered to the spawn resolver as a
    /// trusted route — the gate it passed the first time decides again, and a
    /// `pin`/`explicit` source keeps it a person pin.
    pub model: Option<String>,
    pub route_effort: Option<api::EffortLevel>,
    pub route_fallback_models: Vec<String>,
    pub route_role: Option<String>,
    /// The routed complexity label — the one the repair ceiling reads.
    pub route_complexity: Option<String>,
    pub route_risk: Option<String>,
    pub route_source: Option<String>,
    pub route_probe_confidence: Option<String>,
}

/// What is left of the loop's one wall-clock budget at a given moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Budget {
    /// The implementer had no budget; neither does the loop.
    Unbounded,
    /// This much remains for the next spawn — never a fresh window.
    Left(Duration),
    /// The deadline has passed: nothing more may be spawned.
    Spent,
}

impl Budget {
    fn window(self) -> Option<Duration> {
        match self {
            Self::Left(left) => Some(left),
            Self::Unbounded | Self::Spent => None,
        }
    }
}

impl ExecutionContract {
    /// The contract of a FIRST implementation job. `now` stands in for a
    /// start the manifest failed to record, so a finite budget is never
    /// silently dropped.
    #[must_use]
    pub(crate) fn of(job: &AgentJob, now: SystemTime) -> Self {
        let started = job
            .manifest
            .started_at
            .as_deref()
            .unwrap_or(job.manifest.created_at.as_str())
            .parse::<u64>()
            .ok()
            .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
            .unwrap_or(now);
        Self {
            cwd: job.cwd.clone(),
            permission_ceiling: job.permission_mode,
            // An unrepresentable finite budget must not become unbounded.
            deadline: job.time_budget.map(|budget| started.checked_add(budget).unwrap_or(now)),
            mcp_passthrough: job.mcp_passthrough.clone(),
            model: job.manifest.model.clone().or_else(|| job.manifest.resolved_model.clone()),
            route_effort: job.route_effort,
            route_fallback_models: job.route_fallback_models.clone(),
            route_role: job.manifest.route_role.clone(),
            route_complexity: job.manifest.route_complexity.clone(),
            route_risk: job.manifest.route_risk.clone(),
            route_source: job.manifest.route_source.clone(),
            route_probe_confidence: job.manifest.route_probe_confidence.clone(),
        }
    }

    /// What the next spawn of the loop may still spend at `now`.
    #[must_use]
    pub(crate) fn budget_at(&self, now: SystemTime) -> Budget {
        match self.deadline {
            None => Budget::Unbounded,
            Some(deadline) => match deadline.duration_since(now) {
                Ok(left) if !left.is_zero() => Budget::Left(left),
                _ => Budget::Spent,
            },
        }
    }

    /// Where every spawn of the loop works and what it may touch: the
    /// checkout, the permission ceiling, the MCP tools and what is left of
    /// the budget. The one helper both generated inputs use.
    fn carry_environment(&self, input: &mut AgentInput, budget: Budget) {
        input.cwd.clone_from(&self.cwd);
        input.parent_permission_mode = self.permission_ceiling;
        input.mcp_passthrough.clone_from(&self.mcp_passthrough);
        input.time_budget = budget.window();
    }

    /// A repair only: the model, effort and route the implementer ran on.
    /// The reviewer keeps its own route — it is a different role.
    fn carry_route(&self, input: &mut AgentInput) {
        input.route_model.clone_from(&self.model);
        input.route_effort = self.route_effort;
        input.route_fallback_models.clone_from(&self.route_fallback_models);
        input.route_role.clone_from(&self.route_role);
        input.route_complexity.clone_from(&self.route_complexity);
        input.route_risk.clone_from(&self.route_risk);
        input.route_source.clone_from(&self.route_source);
        input.route_probe_confidence.clone_from(&self.route_probe_confidence);
    }
}

/// The implementer's task as the loop must remember it: the original request,
/// the execution contract it ran under, the round this job belongs to (1 = the
/// first implementation attempt) and — kept apart from the original request —
/// the finding the latest repair was asked to fix.
#[derive(Debug, Clone)]
pub(crate) struct VerifyLoop {
    pub description: String,
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub contract: ExecutionContract,
    pub round: u32,
    /// `None` until a repair is spawned; then the finding that repair targets.
    pub repair_target: Option<FindingEvidence>,
}

impl VerifyLoop {
    /// The loop a completed FIRST implementation attempt starts.
    fn start(job: &AgentJob, now: SystemTime) -> Self {
        Self {
            description: job.manifest.description.clone(),
            prompt: job.prompt.clone(),
            subagent_type: job.manifest.subagent_type.clone(),
            contract: ExecutionContract::of(job, now),
            round: 1,
            repair_target: None,
        }
    }
}

/// What the loop does after one completion.
#[derive(Debug)]
pub(crate) enum LoopPlan {
    /// Spawn this verifier or repair.
    Launch(Box<AgentInput>),
    /// End here, without a pass.
    Stop(LoopStop),
}

/// Why the loop ended without a passing verdict. Every variant means the work
/// is NOT verified; none of them is a completion of the work itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoopStop {
    /// The loop's one deadline passed before the next spawn.
    DeadlineSpent,
    /// A repair the loop spawned ended without completing.
    RepairDidNotFinish { status: String },
    /// The verifier ended without completing.
    VerifierDidNotFinish { status: String },
    /// The verifier completed without a usable pass/fail verdict.
    UnusableVerdict,
    /// The repair left the same finding standing.
    RepeatedFinding { finding: String },
    /// The difficulty ceiling is reached and the work still fails.
    CeilingReached { finding: String },
    /// The next verifier or repair could not start.
    LaunchFailed { error: String },
}

impl LoopStop {
    fn reason(&self) -> String {
        match self {
            Self::DeadlineSpent => "the loop's time budget is spent".to_string(),
            Self::RepairDidNotFinish { status } => format!("the repair ended `{status}`"),
            Self::VerifierDidNotFinish { status } => format!("the verifier ended `{status}`"),
            Self::UnusableVerdict => "the verifier returned no usable verdict".to_string(),
            Self::RepeatedFinding { finding } => format!("the repair left the same finding standing — {finding}"),
            Self::CeilingReached { finding } => format!("the repair ceiling is reached and the work still fails — {finding}"),
            Self::LaunchFailed { error } => format!("the next verification step could not start — {error}"),
        }
    }
}

/// Whether verification by default is on, from the raw `ZO_AUTO_VERIFY` value.
#[must_use]
pub(crate) fn auto_verify_enabled(env_value: Option<&str>) -> bool {
    !matches!(
        env_value.map(|value| value.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// Whether a just-finished spawn should be verified by default. Over
/// primitives so the table below is the whole truth. A retried implementer
/// (it carries loop state but judges nobody) is verified again; only a
/// verifier itself is never verified.
#[must_use]
pub(crate) fn eligible(
    attendance: Attendance,
    status: &str,
    is_judge: bool,
    workflow_member: bool,
    is_implementation: bool,
) -> bool {
    attendance == Attendance::Unattended
        && status == "completed"
        && !is_judge
        && !workflow_member
        && is_implementation
}

/// The repair-round ceiling for an implementer's persisted complexity label;
/// an absent or unknown label takes the `Unknown` row — never a guess.
#[must_use]
pub(crate) fn ceiling_for(complexity_label: Option<&str>) -> usize {
    let complexity = complexity_label
        .and_then(runtime::RouteTaskComplexity::from_label)
        .unwrap_or_default();
    runtime::completion_ceiling_for(complexity)
}

/// The finding a failing verdict carries, from the structured verdict's
/// `title`/`evidence` (the workflow verdict schema); a fixed phrase when the
/// verifier stated none, so the retry prompt is never empty.
#[must_use]
pub(crate) fn finding_text(structured: Option<&serde_json::Value>) -> String {
    let field = |key: &str| {
        structured
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    match (field("title"), field("evidence")) {
        (Some(title), Some(evidence)) => format!("{title}: {evidence}"),
        (Some(one), None) | (None, Some(one)) => one.to_string(),
        (None, None) => NO_EVIDENCE.to_string(),
    }
}

/// What a completed (or ended) implementer earns: its verifier, a stop, or
/// nothing when it is not part of an unattended implementation loop. The
/// first attempt starts the loop; a repair CARRIES it — contract, round and
/// the finding it was fixing — so nothing here is re-derived from the
/// repair's own job.
pub(crate) fn plan_auto_verify(
    job: &AgentJob,
    completion: &AgentCompletion,
    attendance: Attendance,
    env_value: Option<&str>,
    now: SystemTime,
) -> Option<LoopPlan> {
    let enabled = auto_verify_enabled(env_value) && attendance == Attendance::Unattended;
    // A repair the loop spawned that did not finish ends the loop here: the
    // work it was fixing is still unverified, and nothing else would say so.
    if enabled && job.judged_agent.is_none() && job.verify_loop.is_some() && completion.status != "completed" {
        return Some(LoopPlan::Stop(LoopStop::RepairDidNotFinish { status: completion.status.clone() }));
    }
    let kind = job.manifest.subagent_type.as_deref().unwrap_or_default();
    let is_implementation =
        super::subagent_profile::implementation_task(kind, &job.manifest.description, &job.prompt);
    if !enabled
        || !eligible(
            attendance,
            &completion.status,
            job.judged_agent.is_some(),
            job.workflow_member,
            is_implementation,
        )
    {
        return None;
    }
    let state = job.verify_loop.clone().unwrap_or_else(|| VerifyLoop::start(job, now));
    let budget = state.contract.budget_at(now);
    if budget == Budget::Spent {
        return Some(LoopPlan::Stop(LoopStop::DeadlineSpent));
    }
    Some(match verifier_input(job, completion.result.as_deref(), state, budget) {
        Ok(input) => LoopPlan::Launch(Box::new(input)),
        Err(error) => LoopPlan::Stop(LoopStop::LaunchFailed { error: error.to_string() }),
    })
}

/// What a verifier of this loop earns once it ends: nothing on a pass (the
/// work is verified), a repair carrying the finding while the ceiling allows,
/// or a stop. `None` also when the job is no verifier of this loop.
pub(crate) fn plan_retry(
    job: &AgentJob,
    completion: &AgentCompletion,
    attendance: Attendance,
    env_value: Option<&str>,
    now: SystemTime,
) -> Option<LoopPlan> {
    let state = job.verify_loop.as_ref()?;
    // Only a VERIFIER continues the loop; a retried implementer also carries
    // loop state, and its completion is handled by `plan_auto_verify`.
    job.judged_agent.as_ref()?;
    if !auto_verify_enabled(env_value) || attendance != Attendance::Unattended {
        return None;
    }
    if completion.status != "completed" {
        return Some(LoopPlan::Stop(LoopStop::VerifierDidNotFinish { status: completion.status.clone() }));
    }
    match crate::workflow_tools::engine::items::semantic_verdict(&completion.status, completion.structured.as_ref())
        .and_then(super::spawn::verdict_passed)
    {
        Some(true) => return None,
        None => return Some(LoopPlan::Stop(LoopStop::UnusableVerdict)),
        Some(false) => {}
    }
    let finding = finding_text(completion.structured.as_ref());
    let evidence = finding_evidence(&completion.status, completion.structured.as_ref());
    if evidence.is_some() && evidence == state.repair_target {
        return Some(LoopPlan::Stop(LoopStop::RepeatedFinding { finding }));
    }
    let ceiling = ceiling_for(state.contract.route_complexity.as_deref());
    let runtime::CompletionStep::Retry { next_round } =
        runtime::completion_loop_step(false, state.round as usize, ceiling)
    else {
        return Some(LoopPlan::Stop(LoopStop::CeilingReached { finding }));
    };
    let budget = state.contract.budget_at(now);
    if budget == Budget::Spent {
        return Some(LoopPlan::Stop(LoopStop::DeadlineSpent));
    }
    Some(match retry_input(job, state, &finding, evidence, next_round, budget) {
        Ok(input) => LoopPlan::Launch(Box::new(input)),
        Err(error) => LoopPlan::Stop(LoopStop::LaunchFailed { error: error.to_string() }),
    })
}

fn verifier_input(
    job: &AgentJob,
    result: Option<&str>,
    state: VerifyLoop,
    budget: Budget,
) -> Result<AgentInput, ToolError> {
    let description = format!("verify: {}", job.manifest.description);
    let prompt = format!(
        "Verify the work agent `{id}` just finished for this task, as a strict reviewer.\n\n\
         Task: {description}\n\nOriginal request:\n{request}\n\nThe agent reported:\n{result}\n\n\
         Inspect the current diff and relevant tests. Return the verdict JSON only: \
         `pass` (with `coverage`) when the work is correct and complete, `fail` with `title`, \
         `evidence` and `affected_paths` when it is not.",
        id = job.manifest.agent_id,
        description = job.manifest.description,
        request = job.prompt,
        result = result.unwrap_or("(no result text)"),
    );
    let mut input: AgentInput = serde_json::from_value(serde_json::json!({
        "description": description,
        "prompt": prompt,
        "subagent_type": VERIFIER_SUBAGENT_TYPE,
        "schema": crate::workflow_tools::verdict_schema(),
    }))
    .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    // Bound to the attempt that just finished, frozen now: a resume of the
    // implementer while this verifier runs must not re-label the verdict.
    input.judged_agent = Some(super::spawn::current_on_disk_manifest_or_spawn_time(&job.manifest));
    input.one_shot = true;
    input.registry.clone_from(&job.registry);
    input.parent_session_id.clone_from(&job.manifest.parent_session_id);
    state.contract.carry_environment(&mut input, budget);
    input.verify_loop = Some(state);
    Ok(input)
}

fn retry_input(
    job: &AgentJob,
    state: &VerifyLoop,
    finding: &str,
    target: Option<FindingEvidence>,
    next_round: usize,
    budget: Budget,
) -> Result<AgentInput, ToolError> {
    let prompt = format!(
        "{original}\n\n{marker} round {round}: the previous attempt did not pass verification.\n{finding}\n\
         Fix it and make sure the relevant tests pass.",
        original = state.prompt,
        marker = VERIFY_FINDING_MARKER,
        round = state.round,
        finding = finding,
    );
    let mut input: AgentInput = serde_json::from_value(serde_json::json!({
        "description": state.description,
        "prompt": prompt,
        "subagent_type": state.subagent_type,
    }))
    .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    input.registry.clone_from(&job.registry);
    input.parent_session_id.clone_from(&job.manifest.parent_session_id);
    state.contract.carry_environment(&mut input, budget);
    state.contract.carry_route(&mut input);
    let round = u32::try_from(next_round).unwrap_or(u32::MAX);
    input.prior_failures = round.saturating_sub(1);
    // The retry carries the loop state forward (it judges nobody), so its own
    // completion is verified again at this round — against this finding.
    input.verify_loop = Some(VerifyLoop {
        round,
        repair_target: target,
        ..state.clone()
    });
    Ok(input)
}

/// The receipt the owning session gets when the loop ends without a pass.
/// Pure. It names the work the loop was verifying — the verifier's judged
/// agent, or the implementer itself — and never reads as a completion of it.
#[must_use]
pub(crate) fn stop_receipt(job: &AgentJob, stop: &LoopStop) -> AgentCompletion {
    let subject = job.judged_agent.as_ref().unwrap_or(&job.manifest).agent_id.as_str();
    let round = job.verify_loop.as_ref().map_or(1, |state| state.round);
    let reason = stop.reason();
    AgentCompletion {
        agent_id: format!("{subject}{RECEIPT_ID_SUFFIX}"),
        name: RECEIPT_NAME.to_string(),
        status: "failed".to_string(),
        result: Some(format!(
            "Verification by default stopped at round {round} for agent `{subject}`: {reason}. \
             The work is NOT verified; do not report it as done without checking it."
        )),
        structured: None,
        error: Some(reason),
        run: HelperRun::default(),
    }
}

/// Carry out a plan: launch its spawn, or hand back the stop it already is.
/// Returns the stop the loop ENDS on — a launch that could not start is one —
/// so no ending goes unsaid.
pub(crate) fn settle<F>(plan: LoopPlan, job: &AgentJob, spawn: F) -> Option<LoopStop>
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    match plan {
        LoopPlan::Launch(input) => super::execute_agent_with_spawn_and_parent_model_and_hooks(
            *input,
            spawn,
            job.parent_model.as_deref(),
            job.lsp.as_ref(),
            Some(&job.hook_config),
        )
        .err()
        .map(|error| LoopStop::LaunchFailed { error: error.to_string() }),
        LoopPlan::Stop(stop) => Some(stop),
    }
}

/// Hook: spawn the verifier a completed implementer earned, or say why the
/// loop ends here. A spawn that cannot start never disturbs the completion
/// that fired it — it becomes the loop's stop receipt instead.
pub(crate) fn maybe_auto_verify<F>(job: &AgentJob, completion: &AgentCompletion, spawn: F)
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    let env = std::env::var(AUTO_VERIFY_ENV).ok();
    let plan = plan_auto_verify(job, completion, runtime::declared_attendance(), env.as_deref(), SystemTime::now());
    finish(plan, job, spawn);
}

/// Hook: on a verifier's failing verdict, re-spawn the implementer with the
/// finding while the ceiling allows; otherwise say why the loop ends.
pub(crate) fn maybe_continue_verify_loop<F>(job: &AgentJob, completion: &AgentCompletion, spawn: F)
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    let env = std::env::var(AUTO_VERIFY_ENV).ok();
    let plan = plan_retry(job, completion, runtime::declared_attendance(), env.as_deref(), SystemTime::now());
    finish(plan, job, spawn);
}

fn finish<F>(plan: Option<LoopPlan>, job: &AgentJob, spawn: F)
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    if let Some(stop) = plan.and_then(|plan| settle(plan, job, spawn)) {
        super::completion::notify_session_fact(stop_receipt(job, &stop), job.manifest.parent_session_id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opt_out_switch_reads_off_words_only() {
        assert!(auto_verify_enabled(None));
        assert!(auto_verify_enabled(Some("1")));
        assert!(auto_verify_enabled(Some("on")));
        for off in ["0", "off", "FALSE", " no "] {
            assert!(!auto_verify_enabled(Some(off)), "{off:?} must turn it off");
        }
    }

    #[test]
    fn eligibility_is_unattended_completed_first_attempt_implementation_only() {
        let ok = |attendance, status, judge, workflow, implementation| {
            eligible(attendance, status, judge, workflow, implementation)
        };
        assert!(ok(Attendance::Unattended, "completed", false, false, true));
        assert!(!ok(Attendance::Attended, "completed", false, false, true), "attended keeps the ask-once rule");
        assert!(!ok(Attendance::Unattended, "failed", false, false, true));
        assert!(!ok(Attendance::Unattended, "completed", true, false, true), "a verifier is never verified");
        assert!(!ok(Attendance::Unattended, "completed", false, true, true), "workflows own their repair loop");
        assert!(!ok(Attendance::Unattended, "completed", false, false, false), "read-only work is not verified");
    }

    #[test]
    fn the_ceiling_follows_the_persisted_complexity_label() {
        assert_eq!(ceiling_for(Some("large")), runtime::completion_ceiling_for(runtime::RouteTaskComplexity::Large));
        assert_eq!(ceiling_for(Some("trivial")), runtime::completion_ceiling_for(runtime::RouteTaskComplexity::Trivial));
        assert_eq!(ceiling_for(None), runtime::completion_ceiling_for(runtime::RouteTaskComplexity::Unknown));
        assert_eq!(ceiling_for(Some("nonsense")), ceiling_for(None), "unknown label is not a guess");
    }

    #[test]
    fn the_finding_quotes_title_and_evidence_or_says_none_was_given() {
        let both = serde_json::json!({"verdict": "fail", "title": "missing test", "evidence": "parser has no case for empty input"});
        assert_eq!(finding_text(Some(&both)), "missing test: parser has no case for empty input");
        let only_evidence = serde_json::json!({"verdict": "fail", "evidence": "  panics on empty  "});
        assert_eq!(finding_text(Some(&only_evidence)), "panics on empty");
        assert_eq!(finding_text(Some(&serde_json::json!({"verdict": "fail"}))), NO_EVIDENCE);
        assert_eq!(finding_text(None), NO_EVIDENCE);
    }
}
