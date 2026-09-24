//! P2 verdict→worker attribution: fold a validator's judgement of an agent's
//! output back onto that agent's routed model, in the SAME bounded
//! route-outcome history the spawn recorder writes.
//!
//! Run-level outcomes (the spawn recorder) say "the model finished"; verdict
//! outcomes say "the work was right". Both are decisive samples for the
//! feedback scorer, so a model that reliably completes but produces work that
//! fails verification stops winning its role on run-completions alone.
//!
//! Only WELL-BOUND (verdict, worker) pairs are recorded — pairs where the
//! judged output is structurally known to belong to one agent:
//! - the repair loop's focused reverify judging the FIXER's change, and
//! - a validator emitting unusable output (a quality failure of the
//!   VALIDATOR itself).
//!
//! Anything with ambiguous provenance (a new unrelated finding, an initial
//! validator sweep over merged worker output) records nothing.
//!
//! A verification about ANOTHER agent's work is bound to one attempt of it
//! before the verifier runs, and the verdict speaks for that attempt as it
//! was then: [`bind_attempt`] freezes the judged agent's manifest — its
//! attempt identity and its route/model metadata — and the `_for_attempt`
//! recorders write from that frozen copy, never from whatever the store
//! holds when the verdict is published. A resume in between advances the
//! store to a new generation, possibly on another model; neither may speak
//! for the attempt that was judged.

use std::path::Path;

use runtime::RouteOutcomeRecord;
use serde_json::Value;

use crate::misc_tools::AgentOutput;

/// Weight a verdict signal contributes to the learned-specialty scorer
/// (Phase 6 consumer of the v2 `signalWeight` field). A strict pass/fail
/// judgement — the repair loop's focused reverify, a validator's own
/// usable-output check, the deep-gate VERIFY panel, a planner-bound
/// reviewer→worker pair, or an ad-hoc standalone review — is direct evidence
/// about correctness, so it counts at full weight. A preference judgement
/// (e.g. a council/self-consistency "pick the better of N" — no caller wired
/// yet, reserved for a future source) only says "better than its peers", a
/// strictly weaker claim than "correct in isolation", so it counts at half
/// weight. Constants are simple, documented, fixed points — not fit to any
/// live data — so a future caller can rely on them without re-deriving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerdictKind {
    PassFail,
    #[allow(
        dead_code,
        reason = "reserved for a future preference-based verdict source (Phase 6 \
                  council/self-consistency winner) — no caller constructs it yet, \
                  exercised only by the attribution unit test"
    )]
    Preference,
    /// The validator's OWN output was unusable (`SemanticVerdict::Invalid`):
    /// a decisive failure for the validator's model, recorded against it, but
    /// stamped `verdict_subject: validator` so the accuracy report never counts
    /// it as a defect caught in the work.
    ValidatorFault,
}

impl VerdictKind {
    const fn weight(self) -> f32 {
        match self {
            Self::PassFail | Self::ValidatorFault => 1.0,
            Self::Preference => 0.5,
        }
    }

    /// Who the recorded verdict is about.
    const fn subject(self) -> runtime::VerdictSubject {
        match self {
            Self::PassFail | Self::Preference => runtime::VerdictSubject::Work,
            Self::ValidatorFault => runtime::VerdictSubject::Validator,
        }
    }
}

/// Best-effort production entry: resolve the agent store and project cwd, then
/// attribute `passed` to the agent's manifest model. Silent on any failure —
/// attribution must never disturb the workflow that produced the verdict.
pub(crate) fn record_verdict_outcome_for_agent(
    registry: Option<&crate::misc_tools::AgentRegistry>,
    agent_id: &str,
    passed: bool,
    kind: VerdictKind,
    basis: runtime::VerdictBasis,
) {
    let Some((cwd, store)) = resolve_cwd_and_store(registry, agent_id) else {
        return;
    };
    record_verdict_outcome_at(&cwd, &store, agent_id, passed, kind, basis);
}

/// Bind a verification to `agent_id`'s CURRENT attempt: its manifest as the
/// store holds it now, found the same way the recorders find it. Call it
/// before the verifier runs; the returned copy is what the verdict about that
/// attempt is written from ([`record_verdict_outcome_for_attempt`],
/// [`record_verification_unavailable_for_attempt`]). `None` when the agent
/// has no readable manifest — there is no attempt to bind, and the verdict
/// then records nothing.
pub(crate) fn bind_attempt(
    registry: Option<&crate::misc_tools::AgentRegistry>,
    agent_id: &str,
) -> Option<AgentOutput> {
    match registry {
        Some(registry) => registry.manifest_by_id(agent_id),
        None => crate::misc_tools::AgentRegistry::unowned_from_cwd().manifest_by_id(agent_id),
    }
}

/// [`record_verdict_outcome_for_agent`] for a verdict about a BOUND attempt:
/// written from the manifest frozen when the verification was bound
/// ([`bind_attempt`], or the spawn path's own copy), so the attempt it names
/// and the model and route it credits are the judged run's, whatever the
/// store says by now. `None` (nothing was bound) records nothing.
///
/// `source` is the source state the verifier saw when it settled
/// (`RouteOutcomeRecord::source`), where the recorder knows it — a bound
/// verifier's own working tree — and `None` where it does not: a verdict
/// that cannot say which work it judged is never a receipt for a
/// comparison of the attempt's (t-6263), and counts for the router as
/// before.
pub(crate) fn record_verdict_outcome_for_attempt(
    attempt: Option<&AgentOutput>,
    passed: bool,
    kind: VerdictKind,
    basis: runtime::VerdictBasis,
    source: Option<String>,
) {
    let (Some(attempt), Ok(cwd)) = (attempt, std::env::current_dir()) else {
        return;
    };
    record_attempt_verdict_at(&cwd, attempt, passed, kind, basis, source);
}

/// Record that a verification BOUND to an attempt ended without a settled
/// verdict — the verifier's output was unusable, or it never finished. That
/// attempt is UNVERIFIED: not evidence its work failed, so it costs the
/// worker nothing, but no longer a bare completion win either. The record is
/// a `verify` decision with the store's one terminal, non-decisive status
/// (`stopped`); the router's learning rule lets it speak over the attempt's
/// run outcome and settle nothing, while a real pass or failure about the
/// same attempt still outranks it. Written, like every bound verdict, from
/// the manifest frozen at binding. Same best-effort doctrine as the verdict
/// recorder. The VERIFIER's own fault, when its output was unusable, is a
/// separate [`VerdictKind::ValidatorFault`] verdict on its own attempt.
pub(crate) fn record_verification_unavailable_for_attempt(attempt: Option<&AgentOutput>) {
    let (Some(attempt), Ok(cwd)) = (attempt, std::env::current_dir()) else {
        return;
    };
    record_verification_unavailable_at(&cwd, attempt);
}

/// The provenance label a fold record carries in `signal`, beside the
/// verdict recorder's `"verdict"`: this lane ran, and the union already held
/// everything it produced.
pub(crate) const FOLD_SIGNAL: &str = "fold";

/// Record a FOLD decision outcome for `agent_id`: its fan-out lane completed
/// but was dropped as a duplicate (design P1 `fold` — the wasted-spawn signal
/// the accuracy report's fold rate reads). Same best-effort, never-disturb
/// doctrine as the verdict recorder.
pub(crate) fn record_fold_outcome_for_agent(
    registry: Option<&crate::misc_tools::AgentRegistry>,
    agent_id: &str,
) {
    let Some((cwd, store)) = resolve_cwd_and_store(registry, agent_id) else {
        return;
    };
    record_fold_outcome_at(&cwd, &store, agent_id);
}

/// The store holding the agent's manifest — found through the session's
/// registry (root and adopted mirrors), never re-derived from the process cwd
/// — and the project cwd the outcome store hangs off. `None` when either is
/// unknowable; recorders then write nothing.
fn resolve_cwd_and_store(
    registry: Option<&crate::misc_tools::AgentRegistry>,
    agent_id: &str,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let store = match registry {
        Some(registry) => registry.store_of(agent_id),
        None => crate::misc_tools::AgentRegistry::unowned_from_cwd().store_of(agent_id),
    }?;
    let cwd = std::env::current_dir().ok()?;
    Some((cwd, store))
}

/// The record every attribution shares: the agent's manifest read once, its
/// canonical model, target and P3 route metadata copied onto a fresh record
/// with the given terminal `status`, stamped with the attempt it is about —
/// the JUDGED agent's id and the run generation its manifest holds, the same
/// attempt identity the spawn recorder stamped on that run's own outcome.
/// `None` when the manifest is missing, unparsable, or names no model — there
/// is nothing to credit or blame.
fn base_record_for_agent(store: &Path, agent_id: &str, status: &str) -> Option<RouteOutcomeRecord> {
    let agent_id = agent_id.trim();
    if agent_id.is_empty() {
        return None;
    }
    let raw = std::fs::read_to_string(store.join(format!("{agent_id}.json"))).ok()?;
    let manifest = serde_json::from_str::<Value>(&raw).ok()?;
    base_record_from_manifest(&manifest, agent_id, status)
}

/// [`base_record_for_agent`] for a bound attempt: the same reader over the
/// frozen manifest's own wire form, so a bound verdict and a by-id one can
/// never read a field differently.
fn base_record_for_attempt(attempt: &AgentOutput, status: &str) -> Option<RouteOutcomeRecord> {
    let agent_id = attempt.agent_id.trim();
    if agent_id.is_empty() {
        return None;
    }
    let manifest = serde_json::to_value(attempt).ok()?;
    base_record_from_manifest(&manifest, agent_id, status)
}

fn base_record_from_manifest(manifest: &Value, agent_id: &str, status: &str) -> Option<RouteOutcomeRecord> {
    let model = manifest_model(manifest)?;
    let target = manifest
        .get("subagentType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("general-purpose");
    Some(
        RouteOutcomeRecord::new(
            "subagent",
            target,
            // P3 canonicalization-at-write, same as the spawn recorder.
            crate::misc_tools::canonicalize_route_model_id(&model),
            status,
        )
        .with_role(manifest_string_field(manifest, "routeRole"))
        .with_complexity(manifest_string_field(manifest, "routeComplexity"))
        .with_risk(manifest_string_field(manifest, "routeRisk"))
        .with_route_source(manifest_string_field(manifest, "routeSource"))
        .with_requested_model(manifest_string_field(manifest, "requestedModel"))
        .with_output_tokens(manifest_output_tokens(manifest))
        // `runGeneration` is omitted from a manifest while it is zero (a
        // legacy manifest), exactly as `AgentOutput` reads it back.
        .with_attempt(
            agent_id,
            manifest.get("runGeneration").and_then(Value::as_u64).unwrap_or(0),
        ),
    )
}

/// Cwd-injected "verification unavailable" recorder, the seam the tests
/// exercise.
fn record_verification_unavailable_at(cwd: &Path, attempt: &AgentOutput) {
    let Some(record) = base_record_for_attempt(attempt, "stopped") else {
        return;
    };
    let record = record
        .with_signal("verdict")
        .with_decision(runtime::DecisionKind::Verify);
    let _ = runtime::record_route_outcome(cwd, &record);
}

/// Store/cwd-injected fold recorder, the seam the tests exercise.
fn record_fold_outcome_at(cwd: &Path, store: &Path, agent_id: &str) {
    // A folded lane finished — `completed` is its terminal status; the FOLD
    // decision is what the record judges.
    let Some(record) = base_record_for_agent(store, agent_id, "completed") else {
        return;
    };
    let record = record
        .with_signal(FOLD_SIGNAL)
        .with_decision(runtime::DecisionKind::Fold);
    let _ = runtime::record_route_outcome(cwd, &record);
}

/// Store/cwd-injected core, split out so tests can exercise the seam against
/// temp directories without touching process env or real project state.
fn record_verdict_outcome_at(
    cwd: &Path,
    store: &Path,
    agent_id: &str,
    passed: bool,
    kind: VerdictKind,
    basis: runtime::VerdictBasis,
) {
    let Some(record) = base_record_for_agent(store, agent_id, verdict_status(passed)) else {
        return;
    };
    write_verdict_record(cwd, record, kind, basis);
}

/// Cwd-injected bound-attempt verdict recorder, the seam the tests exercise.
fn record_attempt_verdict_at(
    cwd: &Path,
    attempt: &AgentOutput,
    passed: bool,
    kind: VerdictKind,
    basis: runtime::VerdictBasis,
    source: Option<String>,
) {
    let Some(record) = base_record_for_attempt(attempt, verdict_status(passed)) else {
        return;
    };
    write_verdict_record(cwd, record.with_source(source), kind, basis);
}

/// Terminal-only by construction: a verdict is always a settled pass/fail
/// judgement, never `still_running` — the shared recorder-side doctrine guard
/// in `runtime::record_route_outcome` would skip-write (debug-assert in dev)
/// if this ever produced anything else.
const fn verdict_status(passed: bool) -> &'static str {
    if passed {
        "completed"
    } else {
        "failed"
    }
}

fn write_verdict_record(
    cwd: &Path,
    record: RouteOutcomeRecord,
    kind: VerdictKind,
    basis: runtime::VerdictBasis,
) {
    let record = record
        .with_signal("verdict")
        .with_decision(runtime::DecisionKind::Verify)
        .with_verdict_subject(kind.subject())
        .with_verdict_basis(basis)
        .with_signal_weight(Some(kind.weight()));
    let _ = runtime::record_route_outcome(cwd, &record);
    // A verdict about an attempt is the receipt the challenger arm's
    // comparison of that attempt has been waiting for (t-6263): its label is
    // written here, where the verdict lands, and never off a completion.
    let _labelled = crate::misc_tools::note_challenger_verdicts(cwd);
}

/// The objective verdict a command gate settles for a single-item phase: a
/// green exit is a pass, a red exit is a fail, and no command (no checker
/// configured) is no verdict at all — never a fabricated one.
pub(crate) fn objective_verdict_from_exit(command_exit: Option<i32>) -> Option<bool> {
    command_exit.map(|exit| exit == 0)
}

/// Best-effort total output tokens for the judged agent's whole run, summed
/// from the manifest's persisted `tokenHistory` (per-turn output-token
/// deltas — see `AgentOutput::token_history`). Cheaper and simpler than
/// threading a live token counter through the attribution seam; `0` when the
/// manifest has no history (legacy manifest, or the run recorded none).
fn manifest_output_tokens(manifest: &Value) -> u64 {
    manifest
        .get("tokenHistory")
        .and_then(Value::as_array)
        .map_or(0, |entries| entries.iter().filter_map(Value::as_u64).sum())
}

fn manifest_model(manifest: &Value) -> Option<String> {
    ["resolvedModel", "model"]
        .iter()
        .filter_map(|key| manifest.get(*key))
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// P3 v2 route-decision metadata, read straight off the same on-disk
/// manifest JSON already used for `manifest_model` — the spawn recorder's
/// `AgentOutput::route_role`/`route_complexity`/`route_risk`/`route_source`
/// fields, under their `routeRole`/`routeComplexity`/`routeRisk`/
/// `routeSource` wire names. `None` on legacy manifests, explicit models, or
/// routing-off spawns — same absence conditions as `routeReason`.
fn manifest_string_field(manifest: &Value, key: &str) -> Option<String> {
    manifest
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::{record_verdict_outcome_at, VerdictKind};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Same seam as the CLI's `isolate_global_zo_home_for_tests` (test cfg
    /// does not cross crates): route the global Zo home at one per-process
    /// temp dir so `record_verdict_outcome_at(cwd, …)` writes its
    /// route-outcomes under temp instead of the developer's real
    /// `~/.zo/projects/`.
    fn isolate_global_zo_home() {
        use std::sync::OnceLock;
        static HOME: OnceLock<PathBuf> = OnceLock::new();
        let home = HOME.get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("zo-test-home-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            std::env::set_var("ZO_CONFIG_HOME", &dir);
            dir
        });
        if std::env::var_os("ZO_CONFIG_HOME").is_none_or(|value| value.is_empty()) {
            std::env::set_var("ZO_CONFIG_HOME", home);
        }
    }

    fn unique_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        isolate_global_zo_home();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-verdict-attr-{tag}-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// These tests depend on `ZO_CONFIG_HOME` staying stable for their whole
    /// body (`record_verdict_outcome_at` resolves the outcome store through it
    /// per call) — hold the crate-wide env lock so tests in other modules that
    /// legitimately swap that variable under the same lock (e.g. the spawn
    /// round-trips) cannot interleave and split our records across two homes.
    /// Without this the suite is order-dependent: green in a full run, flaky
    /// under a filtered `cargo test -p tools verdict`.
    fn locked_env() -> std::sync::MutexGuard<'static, ()> {
        crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_manifest(store: &std::path::Path, id: &str, manifest: &serde_json::Value) {
        std::fs::write(
            store.join(format!("{id}.json")),
            serde_json::to_string(manifest).expect("manifest json"),
        )
        .expect("write manifest");
    }

    fn read_outcomes(cwd: &std::path::Path) -> Vec<runtime::RouteOutcomeRecord> {
        runtime::read_route_outcomes(cwd).unwrap_or_default()
    }

    #[test]
    fn verdict_outcome_attributes_to_the_worker_manifest_model() {
        let _env = locked_env();
        let cwd = unique_dir("cwd");
        let store = unique_dir("store");
        write_manifest(
            &store,
            "agent-1",
            &json!({
                "agentId": "agent-1",
                "subagentType": "Refactor",
                "model": "requested-model",
                "resolvedModel": "worker-model",
                "requestedModel": "requested-model",
                "tokenHistory": [100, 250]
            }),
        );

        record_verdict_outcome_at(&cwd, &store, "agent-1", true, VerdictKind::PassFail, runtime::VerdictBasis::Model);
        record_verdict_outcome_at(&cwd, &store, "agent-1", false, VerdictKind::PassFail, runtime::VerdictBasis::Model);

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 2, "one record per verdict");
        assert!(outcomes.iter().all(|record| {
            record.route_key == "subagent:Refactor"
                && record.selected_model == "worker-model"
                && record.signal.as_deref() == Some("verdict")
                && record.requested_model.as_deref() == Some("requested-model")
                && record.output_tokens == 350
                && record.signal_weight == Some(1.0)
        }));
        assert_eq!(outcomes[0].status, "completed");
        assert_eq!(outcomes[1].status, "failed");
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    /// A validator whose own output was unusable is recorded against ITSELF
    /// with `verdict_subject: validator`, so the accuracy report never reads
    /// it as a defect caught in the work.
    #[test]
    fn validator_fault_is_stamped_as_the_validators_own_verdict() {
        let _env = locked_env();
        let cwd = unique_dir("vfault-cwd");
        let store = unique_dir("vfault-store");
        write_manifest(
            &store,
            "agent-val",
            &json!({"agentId": "agent-val", "subagentType": "code-reviewer", "resolvedModel": "validator-model"}),
        );

        record_verdict_outcome_at(
            &cwd, &store, "agent-val", false, VerdictKind::ValidatorFault, runtime::VerdictBasis::Model,
        );

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, "failed", "still a decisive loss for the validator's model");
        assert_eq!(outcomes[0].verdict_subject_kind(), runtime::VerdictSubject::Validator);
        let metrics = runtime::verify_metrics(&outcomes);
        assert_eq!(metrics.caught, 0, "a validator fault is not a caught defect");
        assert_eq!(metrics.validator_faults, 1);
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    /// A single-item command gate's result is an OBJECTIVE verdict: stamped as
    /// such, and derived only when a command actually ran.
    #[test]
    fn an_objective_command_verdict_is_stamped_objective() {
        let _env = locked_env();
        let cwd = unique_dir("obj-cwd");
        let store = unique_dir("obj-store");
        write_manifest(
            &store,
            "agent-obj",
            &json!({"agentId": "agent-obj", "subagentType": "Refactor", "resolvedModel": "worker-model"}),
        );

        record_verdict_outcome_at(
            &cwd, &store, "agent-obj", true, VerdictKind::PassFail, runtime::VerdictBasis::Objective,
        );

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].verdict_basis_kind(), runtime::VerdictBasis::Objective);
        assert_eq!(runtime::verify_metrics(&outcomes).objective, 1);
        assert_eq!(super::objective_verdict_from_exit(Some(0)), Some(true));
        assert_eq!(super::objective_verdict_from_exit(Some(2)), Some(false));
        assert_eq!(super::objective_verdict_from_exit(None), None, "no command ran, no objective verdict");
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    /// A folded fan-out lane records a FOLD decision against its own model:
    /// completed (it ran), signal `fold`, decision `fold` — the wasted-spawn
    /// sample the accuracy report's fold rate reads.
    #[test]
    fn a_folded_lane_records_a_fold_decision_against_its_model() {
        let _env = locked_env();
        let cwd = unique_dir("fold-cwd");
        let store = unique_dir("fold-store");
        write_manifest(
            &store,
            "lane-dup",
            &json!({"agentId": "lane-dup", "subagentType": "Explore", "resolvedModel": "lane-model"}),
        );

        super::record_fold_outcome_at(&cwd, &store, "lane-dup");

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 1);
        let record = &outcomes[0];
        assert_eq!(record.status, "completed", "a folded lane still finished");
        assert_eq!(record.signal.as_deref(), Some(super::FOLD_SIGNAL));
        assert_eq!(record.decision_kind(), runtime::DecisionKind::Fold);
        assert_eq!(record.selected_model, "lane-model");
        assert_eq!(record.route_key, "subagent:Explore");
        // A missing manifest records nothing, never a guess.
        super::record_fold_outcome_at(&cwd, &store, "nobody");
        assert_eq!(read_outcomes(&cwd).len(), 1);
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    /// Every attribution names the JUDGED attempt — the agent id and the run
    /// generation its manifest holds, the identity the spawn recorder stamps
    /// on that run's own outcome — so a resumed agent's verdict lands on the
    /// resumed run, and a legacy manifest (generation omitted) reads as 0.
    #[test]
    fn attributions_name_the_judged_attempt() {
        let _env = locked_env();
        let cwd = unique_dir("attempt-cwd");
        let store = unique_dir("attempt-store");
        write_manifest(
            &store,
            "agent-resumed",
            &json!({"agentId": "agent-resumed", "subagentType": "Refactor", "resolvedModel": "worker-model", "runGeneration": 3}),
        );
        write_manifest(
            &store,
            "agent-legacy",
            &json!({"agentId": "agent-legacy", "subagentType": "Refactor", "resolvedModel": "worker-model"}),
        );

        record_verdict_outcome_at(&cwd, &store, "agent-resumed", false, VerdictKind::PassFail, runtime::VerdictBasis::Model);
        super::record_fold_outcome_at(&cwd, &store, "agent-legacy");

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].run_id.as_deref(), Some("agent-resumed#3"));
        assert_eq!(outcomes[1].run_id.as_deref(), Some("agent-legacy#0"));
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    /// A verification bound to an attempt that ended without a verdict marks
    /// that attempt UNVERIFIED: a `verify` decision with the non-decisive
    /// `stopped` status — not a caught defect, not a validator fault, no win
    /// and no loss — which the router's learning rule lets speak over the
    /// attempt's bare completion.
    #[test]
    fn an_unavailable_verification_marks_the_attempt_unverified() {
        let _env = locked_env();
        let cwd = unique_dir("unverified-cwd");
        let fixer = bound_attempt(
            json!({"agentId": "fixer-1", "subagentType": "Refactor", "resolvedModel": "fixer-model", "runGeneration": 1}),
        );
        let modelless = bound_attempt(json!({"agentId": "nobody", "subagentType": "Refactor"}));

        super::record_verification_unavailable_at(&cwd, &fixer);
        super::record_verification_unavailable_at(&cwd, &modelless);

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 1, "an attempt that names no model records nothing");
        let record = &outcomes[0];
        assert_eq!(record.status, "stopped");
        assert_eq!(record.decision_kind(), runtime::DecisionKind::Verify);
        assert_eq!(record.run_id.as_deref(), Some("fixer-1#1"));
        let metrics = runtime::verify_metrics(&outcomes);
        assert_eq!((metrics.caught, metrics.validator_faults), (0, 0));
        let summary = runtime::summarize_route_outcomes(&outcomes);
        assert_eq!((summary.by_route[0].completed, summary.by_route[0].failed), (0, 0));
        let _ = std::fs::remove_dir_all(cwd);
    }

    /// The judged agent's manifest as a verification bound to it: the wire
    /// form the store writes, with the fields every manifest carries.
    fn bound_attempt(mut fields: serde_json::Value) -> super::AgentOutput {
        let id = fields["agentId"].as_str().expect("agentId").to_string();
        let object = fields.as_object_mut().expect("manifest object");
        for (key, value) in [
            ("name", id.clone()),
            ("description", "agent".to_string()),
            ("status", "completed".to_string()),
            ("outputFile", format!("/tmp/{id}.md")),
            ("manifestFile", format!("/tmp/{id}.json")),
            ("createdAt", "100".to_string()),
        ] {
            object.entry(key).or_insert(json!(value));
        }
        serde_json::from_value(fields).expect("manifest")
    }

    /// A verdict about a bound attempt speaks for that attempt as it was
    /// BOUND, however far the store has moved by the time the verdict is
    /// published: the judged agent resumed (generation 2) on another model
    /// and route between binding and verdict, and the pass, the failure and
    /// the unavailable verification all still name generation 1, its model
    /// and its route (t-3959).
    #[test]
    fn a_bound_verdict_names_the_attempt_frozen_at_binding() {
        let _env = locked_env();
        let cwd = unique_dir("frozen-cwd");
        let store = unique_dir("frozen-store");
        let judged = bound_attempt(json!({
            "agentId": "fixer-7", "subagentType": "Refactor", "resolvedModel": "bound-model",
            "requestedModel": "bound-request", "routeRole": "coding", "routeComplexity": "large",
            "routeSource": "auto", "runGeneration": 1, "tokenHistory": [30, 12],
        }));
        write_manifest(&store, "fixer-7", &json!({
            "agentId": "fixer-7", "subagentType": "Debug", "resolvedModel": "resumed-model",
            "requestedModel": "resumed-request", "routeRole": "debugging", "routeComplexity": "small",
            "routeSource": "pin", "runGeneration": 2, "tokenHistory": [999],
        }));

        super::record_attempt_verdict_at(&cwd, &judged, true, VerdictKind::PassFail, runtime::VerdictBasis::Model, None);
        super::record_attempt_verdict_at(&cwd, &judged, false, VerdictKind::PassFail, runtime::VerdictBasis::Model, None);
        super::record_verification_unavailable_at(&cwd, &judged);

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 3, "{outcomes:?}");
        for record in &outcomes {
            assert_eq!(record.run_id.as_deref(), Some("fixer-7#1"), "{record:?}");
            assert_eq!(record.route_key, "subagent:Refactor");
            assert_eq!(record.selected_model, "bound-model");
            assert_eq!(record.requested_model.as_deref(), Some("bound-request"));
            assert_eq!(record.role.as_deref(), Some("coding"));
            assert_eq!(record.complexity.as_deref(), Some("large"));
            assert_eq!(record.route_source.as_deref(), Some("auto"));
            assert_eq!(record.output_tokens, 42);
        }
        assert_eq!(
            outcomes.iter().map(|record| record.status.as_str()).collect::<Vec<_>>(),
            ["completed", "failed", "stopped"]
        );
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    #[test]
    fn verdict_outcome_weights_a_preference_signal_at_half() {
        let _env = locked_env();
        let cwd = unique_dir("pref-cwd");
        let store = unique_dir("pref-store");
        write_manifest(
            &store,
            "agent-pref",
            &json!({
                "agentId": "agent-pref",
                "subagentType": "Refactor",
                "resolvedModel": "worker-model"
            }),
        );

        record_verdict_outcome_at(&cwd, &store, "agent-pref", true, VerdictKind::Preference, runtime::VerdictBasis::Model);

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].signal_weight, Some(0.5));
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    #[test]
    fn verdict_outcome_canonicalizes_model_and_stamps_v2_route_metadata() {
        let _env = locked_env();
        let cwd = unique_dir("v2-cwd");
        let store = unique_dir("v2-store");
        write_manifest(
            &store,
            "agent-2",
            &json!({
                "agentId": "agent-2",
                "subagentType": "Plan",
                "model": "claude-opus-4.8",
                "resolvedModel": "claude-opus-4.8",
                "routeRole": "analysis",
                "routeComplexity": "large",
                "routeRisk": "medium",
                "routeSource": "auto"
            }),
        );

        record_verdict_outcome_at(&cwd, &store, "agent-2", true, VerdictKind::PassFail, runtime::VerdictBasis::Model);

        let outcomes = read_outcomes(&cwd);
        assert_eq!(outcomes.len(), 1);
        let record = &outcomes[0];
        assert_eq!(
            record.selected_model, "claude-opus-4-8",
            "write-time canonicalization must dash-normalize the dot variant"
        );
        assert_eq!(record.role.as_deref(), Some("analysis"));
        assert_eq!(record.complexity.as_deref(), Some("large"));
        assert_eq!(record.risk.as_deref(), Some("medium"));
        assert_eq!(record.route_source.as_deref(), Some("auto"));
        // A cross-check attribution IS a verify decision, stamped explicitly.
        assert_eq!(record.decision.as_deref(), Some("verify"));
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }

    #[test]
    fn verdict_outcome_is_silent_without_manifest_or_model() {
        let _env = locked_env();
        let cwd = unique_dir("silent-cwd");
        let store = unique_dir("silent-store");

        // No manifest at all.
        record_verdict_outcome_at(&cwd, &store, "missing", false, VerdictKind::PassFail, runtime::VerdictBasis::Model);
        // Manifest without any model — nothing to credit or blame.
        write_manifest(&store, "modelless", &json!({"subagentType": "Explore"}));
        record_verdict_outcome_at(&cwd, &store, "modelless", true, VerdictKind::PassFail, runtime::VerdictBasis::Model);
        // Blank agent id.
        record_verdict_outcome_at(&cwd, &store, "  ", true, VerdictKind::PassFail, runtime::VerdictBasis::Model);

        assert!(
            read_outcomes(&cwd).is_empty(),
            "best-effort attribution must record nothing on missing provenance"
        );
        let _ = std::fs::remove_dir_all(cwd);
        let _ = std::fs::remove_dir_all(store);
    }
}
