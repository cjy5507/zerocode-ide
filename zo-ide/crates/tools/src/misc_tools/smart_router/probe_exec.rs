//! Blocking executor for the routing probe (`smart.autoClassifier:
//! "probed"`): one bounded Fast-tier `send_message` call whose parsed
//! `{complexity, risk, confidence}` self-assessment feeds
//! `runtime::fuse_probe_assessment` on top of the deterministic classifier.
//!
//! Everything here fails open to `None` — a missing credential, a timeout, a
//! malformed reply, or a panic-free provider error all leave the caller on
//! the deterministic verdict. Failures remain visible under `ZO_ROUTE_DEBUG`;
//! the probe is a refinement, never a user-facing dependency.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use api::{InputMessage, MessageRequest, OutputContentBlock};
use runtime::{
    parse_probe_response, probe_prompt, route_model, ModelInventory, ProbeAssessment,
    RouteRequest, RouteRole, RoutingTarget,
};

use crate::misc_tools::agent_tools::{build_provider_client_for_agent, shared_agent_runtime};

/// Hard wall for one probe call. A Fast-tier model answers the ~200-token
/// classification prompt well inside this; anything slower forfeits the
/// probe rather than stall a spawn batch.
pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// The reply is one small JSON object; this cap is the cost ceiling per
/// probe, not a target.
const PROBE_MAX_TOKENS: u32 = 512;
/// Process-wide probe memo cap. Entries are keyed by task-text fingerprint,
/// so a fan-out retry or a re-spawned identical task never pays (or waits
/// for) a second probe. Arrived-but-malformed responses are memoized too;
/// transient transport/provider/join failures are not, so a later turn can
/// recover. The decision shadow's memo is bounded by the same cap
/// ([`remember_bounded`]).
pub(super) const PROBE_CACHE_CAP: usize = 256;
const ROUTE_DEBUG_ENV: &str = "ZO_ROUTE_DEBUG";

/// Every way a probe CALL can go wrong, as a fixed token shared VERBATIM by
/// the `ZO_ROUTE_DEBUG` console echo and the always-on attestation counter —
/// so the two observability surfaces can never disagree about what happened.
///
/// All of these are `attest_failed`, never `attest_declined`: reaching this
/// file already means a gate decided the probe was worth running (the decline
/// gates live in `turn.rs`), so anything that goes wrong from here is a real
/// failure and must escalate as one.
///
/// The counter is the load-bearing surface. `ZO_ROUTE_DEBUG` is off by
/// default, so before the ledger existed a probe that failed on EVERY call
/// emitted nothing at all — which is precisely how a 100%-failing probe
/// survived for weeks (see [`PROBE_BASE_URL_ENV`]'s post-mortem below). The
/// counter makes that state readable from `/smart doctor` with no env var set.
const FAIL_TIMEOUT: &str = "timeout";
const FAIL_PROVIDER_FAILURE: &str = "provider_failure";
const FAIL_JOIN_FAILURE: &str = "join_failure";
const FAIL_MALFORMED: &str = "malformed";
const FAIL_CACHED_MALFORMED: &str = "cached_malformed";
const FAIL_CLIENT_UNAVAILABLE: &str = "client_unavailable";
const FAIL_CACHE_UNAVAILABLE: &str = "cache_unavailable";
/// The probe's provider is parked behind its own wall — this process's 429,
/// or a neighbour's through the shared cool-down — so no call is made: it
/// would only hear the wall again, and the row would say `provider_failure`
/// for a reason the ledger already knew (the first control row, 2026-09-21).
const FAIL_PROVIDER_PARKED: &str = "provider_parked";

enum ProbeCallOutcome {
    Response(Box<api::MessageResponse>),
    TimedOut,
    ProviderFailure,
}

/// Which road a probe batch runs on. The call, the memo, the wall and the
/// route tax are the same on both; what differs is where the verdict goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbeUse {
    /// The turn's own probe: its verdict routes, and the attestation counter
    /// that answers "did a probe verdict route a turn" says so.
    Routing,
    /// The decision shadow's control sample: the same call made once more
    /// after an active turn, for a ledger row the judge compares
    /// (`decision_shadow::PROBE_CONTROL_EVERY`). Its verdict routes nothing,
    /// so that counter is left alone — the row is its record, failure token
    /// and all.
    Control,
}

impl ProbeUse {
    /// The road's word in the `ZO_ROUTE_DEBUG` echo.
    const fn word(self) -> &'static str {
        match self {
            Self::Routing => "routing",
            Self::Control => "control",
        }
    }
}

/// What the probe said about one non-empty task — its assessment, or the
/// failure token of why it said nothing — beside the task's fingerprint. The
/// routing results are these verdicts with the failures dropped; the decision
/// shadow reads them whole, so its rows can say what the probe did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProbeSlot {
    pub(super) fingerprint: u64,
    pub(super) verdict: Result<ProbeAssessment, &'static str>,
}

/// Add `fresh` entries to a process memo bounded by [`PROBE_CACHE_CAP`]: a memo
/// that would outgrow the cap starts over rather than grow. One policy for the
/// probe's memo and the decision shadow's, so the two bounds cannot drift.
pub(super) fn remember_bounded<K: std::hash::Hash + Eq, V>(memo: &mut HashMap<K, V>, fresh: Vec<(K, V)>) {
    if memo.len() + fresh.len() > PROBE_CACHE_CAP {
        memo.clear();
    }
    memo.extend(fresh);
}

fn route_debug_enabled() -> bool {
    std::env::var(ROUTE_DEBUG_ENV).is_ok()
}

/// Attest one probe firing — a verdict that actually reached the router. A
/// control verdict reached a ledger row instead, and is not one.
fn attest_probe_fired(road: ProbeUse) {
    if road == ProbeUse::Routing {
        telemetry::attest_fired(telemetry::HarnessFeature::RoutingProbe);
    }
}

/// Attest one probe failure the router felt. The control road's failure is
/// its row's probe cell, where the judge reads it.
fn attest_probe_failed(road: ProbeUse, reason: &'static str) {
    if road == ProbeUse::Routing {
        telemetry::attest_failed(telemetry::HarnessFeature::RoutingProbe, reason);
    }
}

/// Record one per-task failure on both surfaces: the attestation counter
/// (always, on the routing road) and the `ZO_ROUTE_DEBUG` echo (only when
/// enabled).
fn probe_failed(road: ProbeUse, fingerprint: u64, model: &str, reason: &'static str) {
    attest_probe_failed(road, reason);
    if route_debug_enabled() {
        eprintln!(
            "[ROUTE_PROBE] task={fingerprint:016x} model={model} road={} result={reason}",
            road.word()
        );
    }
}

fn record_probe_outcome(
    fresh: &mut Vec<(u64, Option<ProbeAssessment>)>,
    fingerprint: u64,
    model: &str,
    outcome: Result<(ProbeCallOutcome, u64), tokio::task::JoinError>,
    attempt: &str,
    road: ProbeUse,
) -> Result<ProbeAssessment, &'static str> {
    // Transport failures are not memoized: a later turn may recover.
    let response = match outcome {
        Ok((ProbeCallOutcome::Response(response), elapsed_ms)) => {
            record_route_tax(attempt, model, runtime::OUTCOME_COMPLETED, elapsed_ms, response.usage.output_tokens);
            response
        }
        Ok((ProbeCallOutcome::TimedOut, elapsed_ms)) => {
            // A probe the wall cut off is `stopped`, not `failed`: nothing
            // about the call says the model would have answered wrongly.
            record_route_tax(attempt, model, runtime::OUTCOME_STOPPED, elapsed_ms, 0);
            probe_failed(road, fingerprint, model, FAIL_TIMEOUT);
            return Err(FAIL_TIMEOUT);
        }
        Ok((ProbeCallOutcome::ProviderFailure, elapsed_ms)) => {
            record_route_tax(attempt, model, runtime::OUTCOME_FAILED, elapsed_ms, 0);
            probe_failed(road, fingerprint, model, FAIL_PROVIDER_FAILURE);
            return Err(FAIL_PROVIDER_FAILURE);
        }
        Err(_) => {
            probe_failed(road, fingerprint, model, FAIL_JOIN_FAILURE);
            return Err(FAIL_JOIN_FAILURE);
        }
    };
    let text: String = response
        .content
        .iter()
        .filter_map(|block| match block {
            OutputContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let assessment = parse_probe_response(&text);
    match assessment {
        Some(assessment) => {
            attest_probe_fired(road);
            if route_debug_enabled() {
                eprintln!(
                    "[ROUTE_PROBE] task={fingerprint:016x} model={model} road={} result=success \
                     complexity={:?} confidence={:?} intent={:?}",
                    road.word(),
                    assessment.complexity,
                    assessment.confidence,
                    assessment.intent
                );
            }
        }
        None => probe_failed(road, fingerprint, model, FAIL_MALFORMED),
    }
    // An arrived malformed response is stable enough to memoize: retrying the
    // same task would spend tokens on the same invalid answer.
    fresh.push((fingerprint, assessment));
    assessment.ok_or(FAIL_MALFORMED)
}

/// Write the probe's own cost as a routing-tax row, billed to the attempt that
/// paid it.
///
/// Best-effort and silent, like every other outcome recorder: a probe is a
/// refinement and its bookkeeping must never be a reason a turn fails. An
/// attempt nobody declared (a bare harness, a caller outside any turn) writes
/// nothing — a tax row with no payer joins to nothing and would only inflate
/// the ledger.
fn record_route_tax(attempt: &str, model: &str, status: &str, elapsed_ms: u64, output_tokens: u32) {
    if attempt.trim().is_empty() {
        return;
    }
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let record = runtime::RouteOutcomeRecord::route_tax(
        runtime::RouteTaxCall::Probe,
        crate::misc_tools::canonicalize_route_model_id(model),
        status,
    )
    .with_attempt_key(attempt)
    .with_duration_ms(Some(elapsed_ms))
    .with_output_tokens(u64::from(output_tokens));
    let _ = runtime::record_route_outcome(&cwd, &record);
}

/// Test/diagnostic override for the probe's endpoint (`http://host:port`).
///
/// The probe's LIVE request path had no coverage at all, and it failed 100% of
/// the time for weeks: first `stream: false` → Codex `400 Stream must be set to
/// true`, then `store: false` → a terminal frame carrying `"output": []`. Both
/// are wire failures, invisible to the parse-level tests, and the fail-open
/// design below turned each one into silence rather than an error. Only an
/// end-to-end test against a real HTTP endpoint can catch that class of bug,
/// and the single thing preventing one was credential resolution inside
/// [`build_provider_client_for_agent`].
///
/// When set, the probe — and only the probe — talks to this endpoint through a
/// ChatGPT-backend (Codex Responses) client with a placeholder bearer. Every
/// other client build in the process, and every other credential path, is
/// untouched.
const PROBE_BASE_URL_ENV: &str = "ZO_PROBE_BASE_URL";

/// The client one probe batch sends on: the override endpoint when
/// [`PROBE_BASE_URL_ENV`] names one, else the normally-resolved provider client
/// for `model`.
fn probe_client(model: &str) -> Option<api::ProviderClient> {
    if let Some(base_url) = std::env::var(PROBE_BASE_URL_ENV)
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
    {
        return Some(api::ProviderClient::chatgpt_backend_at(
            &base_url,
            "zo-probe-diagnostic-token",
        ));
    }
    build_provider_client_for_agent(model).ok()
}

fn probe_cache() -> &'static Mutex<HashMap<u64, Option<ProbeAssessment>>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<u64, Option<ProbeAssessment>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// FNV-1a over both text fields with a length-prefixed separator, so
/// (`"ab"`, `"c"`) and (`"a"`, `"bc"`) cannot collide by concatenation. The
/// decision shadow's rows and the label evaluation join on this same key.
#[must_use]
pub fn task_fingerprint(description: &str, prompt: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for chunk in [description.len().to_le_bytes().as_slice(), description.as_bytes(), prompt.as_bytes()] {
        for byte in chunk {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

/// Resolve the model the probe itself runs on: the router's own Fast-role
/// pick from the already-loaded inventory — the probe reuses the engine it
/// serves instead of hand-rolling a second "cheap model" table.
/// Every task that lost its probe fails with `reason`, one row each — the
/// one shape both a missing client and a parked provider answer in.
fn fail_misses(
    slots: &mut [Option<ProbeSlot>],
    misses: &[(usize, u64)],
    road: ProbeUse,
    model: &str,
    reason: &'static str,
) {
    for (index, fingerprint) in misses {
        probe_failed(road, *fingerprint, model, reason);
        slots[*index] = Some(ProbeSlot {
            fingerprint: *fingerprint,
            verdict: Err(reason),
        });
    }
}

/// How long `model`'s provider is still parked behind a rate-limit wall
/// (`api::quota`: this process's own 429s and the shared file's), or zero.
fn parked_ms(model: &str) -> u64 {
    parked_ms_for(api::detect_provider_kind(model))
}

/// The same question of a provider by kind — the half a test can ask
/// without the model catalog, which another test's config home may hide.
fn parked_ms_for(kind: api::ProviderKind) -> u64 {
    api::quota::rate_limit_cooldown_remaining_ms(kind)
}

fn probe_model(inventory: &ModelInventory, parent_model: &str) -> String {
    let request = RouteRequest::for_target(
        RoutingTarget::RoleFallback(RouteRole::Fast),
        RouteRole::Fast,
        parent_model,
    );
    route_model(&request, inventory).resolved_model
}

/// Run (or recall) the routing probe for one task. Returns `None` on any
/// failure — the caller stays on the deterministic classification.
pub(super) fn route_probe_assessment(
    inventory: &ModelInventory,
    parent_model: &str,
    description: &str,
    prompt: &str,
    attempt: &str,
) -> Option<ProbeAssessment> {
    route_probe_assessments(inventory, parent_model, &[(description, prompt)], attempt)
        .first()
        .copied()
        .flatten()
}

/// Batch form for a fan-out: Jev actual-use mode first judges all unique tasks
/// concurrently under its short wall, then only failed slots run the unchanged
/// chat probe. Outside actual-use mode all cache misses fire concurrently inside
/// one `block_on` (per-probe timeout each), so an N-member spawn pays one probe
/// round-trip of wall-clock, not N sequential ones. Results align with `tasks`
/// by index; every failure is a `None` slot (fail open).
///
/// Record-only mode fires after the chat probe, detached, and never changes the
/// returned assessments.
pub(super) fn route_probe_assessments(
    inventory: &ModelInventory,
    parent_model: &str,
    tasks: &[(&str, &str)],
    attempt: &str,
) -> Vec<Option<ProbeAssessment>> {
    probe_and_shadow(
        inventory,
        parent_model,
        tasks,
        attempt,
        super::decision_shadow::DECISION_SHADOW_DEADLINE,
    )
    .0
}

/// [`route_probe_assessments`] with the decision shadow's batch handed back
/// rather than detached — the one seam a test holds to await the shadow it
/// fired, and to give it a shorter wall. On the active road the batch handed
/// back is the control sample's, when the batch drew one.
pub(super) fn probe_and_shadow(
    inventory: &ModelInventory,
    parent_model: &str,
    tasks: &[(&str, &str)],
    attempt: &str,
    shadow_deadline: Duration,
) -> (Vec<Option<ProbeAssessment>>, Option<tokio::task::JoinHandle<()>>) {
    // The ablation arm bails out HERE rather than discarding the verdict
    // further down, so the control arm pays none of the probe's latency or
    // tokens either — that cost is part of what an ablation measures. This is
    // also the one entry both the single and batch forms funnel through, so
    // no caller can reach a probe firing around the gate.
    if telemetry::attest_ablated(telemetry::HarnessFeature::RoutingProbe) {
        return (vec![None; tasks.len()], None);
    }
    let active_deadline = shadow_deadline.min(super::decision_shadow::DECISION_ACTIVE_DEADLINE);
    if let Some(active) = super::decision_shadow::active_assessments(tasks, attempt, active_deadline) {
        let fallback_tasks: Vec<(&str, &str)> = tasks
            .iter()
            .zip(&active.assessments)
            .map(|(task, assessment)| if assessment.is_some() { ("", "") } else { *task })
            .collect();
        let fallback = probe_slots(inventory, parent_model, &fallback_tasks, attempt, ProbeUse::Routing);
        let results = active
            .assessments
            .into_iter()
            .zip(fallback)
            .map(|(assessment, slot)| assessment.or_else(|| slot.and_then(|slot| slot.verdict.ok())))
            .collect();
        // Routing is settled; the control sample leaves now, off the turn's
        // clock, for the rows the judge compares.
        let control = active
            .control
            .map(|batch| super::decision_shadow::fire_control(batch, inventory, parent_model));
        return (results, control);
    }
    let slots = probe_slots(inventory, parent_model, tasks, attempt, ProbeUse::Routing);
    let shadow = super::decision_shadow::fire(tasks, &slots, attempt, shadow_deadline);
    let results = slots
        .iter()
        .map(|slot| slot.and_then(|slot| slot.verdict.ok()))
        .collect();
    (results, shadow)
}

/// The probe's verdict for each task, aligned with `tasks`: `None` for an
/// empty task (never probed), else what the probe said or why it said nothing.
///
/// `road` says whose verdicts these are — the turn's, or the decision
/// shadow's control sample, which makes the same call for a row rather than
/// a route.
pub(super) fn probe_slots(
    inventory: &ModelInventory,
    parent_model: &str,
    tasks: &[(&str, &str)],
    attempt: &str,
    road: ProbeUse,
) -> Vec<Option<ProbeSlot>> {
    type ProbeJoin = (usize, u64, tokio::task::JoinHandle<(ProbeCallOutcome, u64)>);
    let is_empty = |description: &str, prompt: &str| description.trim().is_empty() && prompt.trim().is_empty();
    let mut slots: Vec<Option<ProbeSlot>> = vec![None; tasks.len()];
    let mut misses: Vec<(usize, u64)> = Vec::new();
    {
        let Ok(cache) = probe_cache().lock() else {
            // Batch-wide bail-out: no fingerprint is resolved yet, so this is
            // the one failure attributed to the batch rather than to a task.
            attest_probe_failed(road, FAIL_CACHE_UNAVAILABLE);
            if route_debug_enabled() {
                eprintln!("[ROUTE_PROBE] road={} result={FAIL_CACHE_UNAVAILABLE}", road.word());
            }
            for (slot, (description, prompt)) in slots.iter_mut().zip(tasks) {
                if !is_empty(description, prompt) {
                    *slot = Some(ProbeSlot {
                        fingerprint: task_fingerprint(description, prompt),
                        verdict: Err(FAIL_CACHE_UNAVAILABLE),
                    });
                }
            }
            return slots;
        };
        for (index, (description, prompt)) in tasks.iter().enumerate() {
            if is_empty(description, prompt) {
                continue;
            }
            let fingerprint = task_fingerprint(description, prompt);
            match cache.get(&fingerprint) {
                Some(memoized) => {
                    slots[index] = Some(ProbeSlot {
                        fingerprint,
                        verdict: memoized.ok_or(FAIL_CACHED_MALFORMED),
                    });
                    // A memo hit counts as a firing: the question this ledger
                    // answers is whether a probe verdict routed a turn, and a
                    // recalled verdict did exactly that. A memoized MALFORMED
                    // reply stays a failure, since no verdict reached the
                    // router.
                    let result = if memoized.is_some() {
                        attest_probe_fired(road);
                        "cached_success"
                    } else {
                        attest_probe_failed(road, FAIL_CACHED_MALFORMED);
                        FAIL_CACHED_MALFORMED
                    };
                    if route_debug_enabled() {
                        eprintln!("[ROUTE_PROBE] task={fingerprint:016x} road={} result={result}", road.word());
                    }
                }
                None => misses.push((index, fingerprint)),
            }
        }
    }
    if misses.is_empty() {
        return slots;
    }
    let model = probe_model(inventory, parent_model);
    // Alias-normalized like every sibling `build_provider_client_for_agent`
    // caller (`provider_client.rs`), so a pinned/aliased Fast-role model id
    // cannot misroute provider detection or the wire model.
    let model = api::resolve_model_alias(&model);
    if parked_ms(&model) > 0 {
        fail_misses(&mut slots, &misses, road, &model, FAIL_PROVIDER_PARKED);
        return slots;
    }
    let Some(client) = probe_client(&model) else {
        // One failure per task that lost its probe, not one per batch — a
        // credential-less environment should read as "every probe gave up",
        // which is what the count then says.
        fail_misses(&mut slots, &misses, road, &model, FAIL_CLIENT_UNAVAILABLE);
        return slots;
    };
    let client = std::sync::Arc::new(client);
    let handle = shared_agent_runtime().handle().clone();
    // Each miss is SPAWNED onto the shared agent runtime, so all probes run
    // concurrently and the batch pays roughly one probe of wall-clock (each
    // still individually timeout-bounded), instead of N sequential calls
    // blocking the spawn path. The fingerprint rides the tuple so the
    // result↔cache pairing never depends on collection order.
    let probes: Vec<ProbeJoin> = misses
        .iter()
        .map(|(index, fingerprint)| {
            let (description, prompt) = tasks[*index];
            let request = probe_request(&model, description, prompt);
            let task = handle.spawn(timed_probe_call(
                std::sync::Arc::clone(&client),
                request,
            ));
            (*index, *fingerprint, task)
        })
        .collect();
    let collect_all = async move {
        let mut collected = Vec::with_capacity(probes.len());
        for (index, fingerprint, task) in probes {
            collected.push((index, fingerprint, task.await));
        }
        collected
    };
    // Sync→async bridge: `run_blocking` uses `block_in_place` only on a
    // multi-thread ambient runtime and falls back to a dedicated runtime
    // otherwise — the hand-rolled `Handle::try_current().is_ok()` guard this
    // replaces panicked on a `current_thread` ambient runtime (the main TUI
    // and headless hosts are exactly that). The probe tasks themselves
    // already run on the shared agent runtime; this only drives the awaits.
    let responses = api::sync_bridge::run_blocking(collect_all);
    let mut fresh: Vec<(u64, Option<ProbeAssessment>)> = Vec::with_capacity(responses.len());
    for (index, fingerprint, outcome) in responses {
        let verdict = record_probe_outcome(&mut fresh, fingerprint, &model, outcome, attempt, road);
        slots[index] = Some(ProbeSlot { fingerprint, verdict });
    }
    if let Ok(mut cache) = probe_cache().lock() {
        remember_bounded(&mut cache, fresh);
    }
    slots
}

/// One probe call and what it cost in wall clock.
///
/// The tax is measured around the CALL, not around the batch — a batch's span
/// is the slowest probe plus the join, which is not what any one attempt paid.
async fn timed_probe_call(
    client: std::sync::Arc<api::ProviderClient>,
    request: MessageRequest,
) -> (ProbeCallOutcome, u64) {
    let started = std::time::Instant::now();
    let outcome = match tokio::time::timeout(PROBE_TIMEOUT, client.send_message(&request)).await {
        Ok(Ok(response)) => ProbeCallOutcome::Response(Box::new(response)),
        Ok(Err(_)) => ProbeCallOutcome::ProviderFailure,
        Err(_) => ProbeCallOutcome::TimedOut,
    };
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    (outcome, elapsed_ms)
}

fn probe_request(model: &str, description: &str, prompt: &str) -> MessageRequest {
    MessageRequest {
        model: model.to_string(),
        max_tokens: PROBE_MAX_TOKENS,
        messages: vec![InputMessage::user_text(probe_prompt(description, prompt))],
        system: None,
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: None,
        output_config: None,
        effort: Some(api::EffortLevel::Low),
        effort_band_ceiling: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider parked behind its wall is read as parked before the batch
    /// builds a client for it. Asked by kind: a sibling test's config home
    /// hides the model catalog, and a model id read without it falls to the
    /// machine's own credentials — xAI's slot, which the guard serializes.
    #[test]
    fn a_probe_on_a_parked_provider_is_parked_too() {
        let _guard = api::quota::rate_limit_test_guard();
        api::quota::isolate_rate_limit_state_for_tests();
        assert_eq!(parked_ms_for(api::ProviderKind::Xai), 0, "a fresh process is parked nowhere");
        api::quota::mark_rate_limit_cooldown(api::ProviderKind::Xai, 60_000);
        assert!(parked_ms_for(api::ProviderKind::Xai) > 0, "the wall xAI just announced is not read");
    }

    #[test]
    fn fingerprint_separates_field_boundaries() {
        assert_ne!(task_fingerprint("ab", "c"), task_fingerprint("a", "bc"));
        assert_eq!(task_fingerprint("a", "b"), task_fingerprint("a", "b"));
    }
}

/// End-to-end coverage of the probe's LIVE request path, over a real socket.
///
/// This is the test that did not exist while the probe failed 100% of the time
/// for weeks. Both historical failures were wire-shaped and therefore invisible
/// to [`runtime::parse_probe_response`]'s unit tests:
///
/// 1. `stream: false` — the Codex Responses endpoint rejects it with `400
///    {"detail":"Stream must be set to true"}`.
/// 2. `store: false` — the terminal `response.completed` frame carries
///    `"output": []`; the answer exists only in the `response.output_text.delta`
///    events, so folding the terminal frame verbatim yielded a well-formed
///    response with empty content, which parsed as malformed and failed open.
///
/// The mocks below therefore speak the REAL contract, empty terminal `output`
/// and all: a regression on either fix turns these red instead of silent.
#[cfg(test)]
mod live_path_tests {
    /// [`super::route_probe_assessment`] for a test with no turn to bill: the
    /// tax row is skipped (an attempt nobody declared writes none), and the
    /// probe path under test is otherwise identical.
    fn route_probe_assessment_for_tests(
        inventory: &super::ModelInventory,
        parent_model: &str,
        description: &str,
        prompt: &str,
    ) -> Option<super::ProbeAssessment> {
        super::route_probe_assessment(inventory, parent_model, description, prompt, "")
    }

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use runtime::{
        ModelInventory, RouteConfidence, RouteTaskComplexity, RouteTaskIntent, RouteTaskRisk,
    };

    use super::PROBE_BASE_URL_ENV;

    /// `PROBE_BASE_URL_ENV` is process-global and the probe memo is too, so the
    /// live-path cases must exclude every other process-environment user,
    /// including routing tests that can read this endpoint by default.
    fn env_lock() -> MutexGuard<'static, ()> {
        crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Sets `ZO_PROBE_BASE_URL` for the duration of one case and restores the
    /// previous value (or absence) on drop, even if the case panics.
    struct ProbeEnv {
        previous: Option<String>,
        _guard: MutexGuard<'static, ()>,
    }

    impl ProbeEnv {
        fn pointing_at(addr: SocketAddr) -> Self {
            let guard = env_lock();
            let previous = std::env::var(PROBE_BASE_URL_ENV).ok();
            std::env::set_var(PROBE_BASE_URL_ENV, format!("http://{addr}"));
            Self { previous, _guard: guard }
        }
    }

    impl Drop for ProbeEnv {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(PROBE_BASE_URL_ENV, value),
                None => std::env::remove_var(PROBE_BASE_URL_ENV),
            }
        }
    }

    #[test]
    fn probe_override_excludes_other_process_environment_users() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture endpoint");
        let _env = ProbeEnv::pointing_at(listener.local_addr().expect("fixture address"));
        assert!(
            matches!(crate::tests::env_lock().try_lock(), Err(std::sync::TryLockError::WouldBlock)),
            "another environment user can read this fixture's probe endpoint"
        );
    }

    /// A blocking HTTP/1.1 mock that answers every request with the same canned
    /// status + body and records each request body it saw. Recording *every*
    /// request (not just the first) is what makes the 400 case able to prove
    /// "no retry storm".
    struct MockCodex {
        addr: SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl MockCodex {
        fn serving(status_line: &'static str, content_type: &'static str, body: String) -> Self {
            Self::serving_after(std::time::Duration::ZERO, status_line, content_type, body)
        }

        /// [`Self::serving`], holding each reply for `delay` after the request
        /// has been read — a slow endpoint, for the cases about time.
        fn serving_after(
            delay: std::time::Duration,
            status_line: &'static str,
            content_type: &'static str,
            body: String,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Codex endpoint");
            let addr = listener.local_addr().expect("mock endpoint address");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let server_requests = Arc::clone(&requests);
            let server_stop = Arc::clone(&stop);
            let join = std::thread::spawn(move || {
                for connection in listener.incoming() {
                    if server_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut connection) = connection else { break };
                    let Some(request_body) = read_http_request(&mut connection) else {
                        continue;
                    };
                    if let Ok(mut recorded) = server_requests.lock() {
                        recorded.push(request_body);
                    }
                    std::thread::sleep(delay);
                    let head = format!(
                        "{status_line}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = connection.write_all(head.as_bytes());
                    let _ = connection.write_all(body.as_bytes());
                    let _ = connection.flush();
                    let _ = connection.shutdown(Shutdown::Write);
                }
            });
            Self { addr, requests, stop, join: Some(join) }
        }

        fn sse(body: String) -> Self {
            Self::serving("HTTP/1.1 200 OK", "text/event-stream", body)
        }

        /// Like [`Self::serving_after`], but each accepted connection owns its
        /// delay. Used to prove a batch pays one request wall rather than one
        /// wall per task.
        fn serving_after_concurrent(
            delay: std::time::Duration,
            status_line: &'static str,
            content_type: &'static str,
            body: String,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind concurrent mock endpoint");
            let addr = listener.local_addr().expect("mock endpoint address");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let server_requests = Arc::clone(&requests);
            let server_stop = Arc::clone(&stop);
            let join = std::thread::spawn(move || {
                for connection in listener.incoming() {
                    if server_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut connection) = connection else { break };
                    let requests = Arc::clone(&server_requests);
                    let body = body.clone();
                    std::thread::spawn(move || {
                        let Some(request_body) = read_http_request(&mut connection) else {
                            return;
                        };
                        if let Ok(mut recorded) = requests.lock() {
                            recorded.push(request_body);
                        }
                        std::thread::sleep(delay);
                        let head = format!(
                            "{status_line}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\r\n",
                            body.len()
                        );
                        let _ = connection.write_all(head.as_bytes());
                        let _ = connection.write_all(body.as_bytes());
                        let _ = connection.flush();
                        let _ = connection.shutdown(Shutdown::Write);
                    });
                }
            });
            Self { addr, requests, stop, join: Some(join) }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().map(|recorded| recorded.clone()).unwrap_or_default()
        }
    }

    impl Drop for MockCodex {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // Unblock the parked `accept` so the thread observes the stop flag.
            let _ = TcpStream::connect(self.addr);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    /// Read one HTTP request off `connection`, returning its body. Honors
    /// `content-length` so the body is fully drained before the mock replies.
    fn read_http_request(connection: &mut TcpStream) -> Option<String> {
        let mut reader = BufReader::new(connection.try_clone().ok()?);
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed
                .split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim())
            {
                content_length = value.parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).ok()?;
        String::from_utf8(body).ok()
    }

    /// The exact shape the live backend sends under `store: false`: the answer
    /// arrives only as text deltas, and the terminal frame's `output` is empty.
    fn responses_sse(answer: &str) -> String {
        let delta = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": answer,
        });
        let completed = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_probe",
                "status": "completed",
                "output": [],
                "usage": { "input_tokens": 120, "output_tokens": 24 },
            },
        });
        format!("data: {delta}\n\ndata: {completed}\n\ndata: [DONE]\n\n")
    }

    fn inventory() -> ModelInventory {
        ModelInventory::new("gpt-5.5-codex", Vec::new())
    }

    #[test]
    fn live_probe_fuses_a_four_field_assessment_including_intent() {
        let mock = MockCodex::sse(responses_sse(
            "{\"complexity\":\"large\",\"risk\":\"high\",\"confidence\":\"high\",\"intent\":\"design\"}",
        ));
        let _env = ProbeEnv::pointing_at(mock.addr);

        let assessment = route_probe_assessment_for_tests(
            &inventory(),
            "gpt-5.5-codex",
            "e2e-intent",
            "redesign the settings screen",
        )
        .expect("a live SSE probe must produce an assessment");

        assert_eq!(assessment.complexity, RouteTaskComplexity::Large);
        assert_eq!(assessment.risk, RouteTaskRisk::High);
        assert_eq!(assessment.confidence, RouteConfidence::High);
        assert_eq!(
            assessment.intent,
            RouteTaskIntent::Design,
            "the intent axis must survive the wire, not just the parser"
        );

        let requests = mock.requests();
        assert_eq!(requests.len(), 1, "one probe, one request");
        let body: serde_json::Value =
            serde_json::from_str(&requests[0]).expect("probe request body is JSON");
        assert_eq!(
            body.get("stream").and_then(serde_json::Value::as_bool),
            Some(true),
            "regression pin: `stream: false` is rejected outright by Codex Responses"
        );
        assert!(
            requests[0].contains("routing classifier"),
            "the real probe prompt must be what goes on the wire"
        );
    }

    #[test]
    fn live_probe_without_the_intent_field_falls_open_to_other() {
        let mock = MockCodex::sse(responses_sse(
            "{\"complexity\":\"small\",\"risk\":\"low\",\"confidence\":\"medium\"}",
        ));
        let _env = ProbeEnv::pointing_at(mock.addr);

        let assessment = route_probe_assessment_for_tests(
            &inventory(),
            "gpt-5.5-codex",
            "e2e-legacy",
            "rename one config key",
        )
        .expect("a legacy three-field probe must still parse end to end");

        assert_eq!(assessment.complexity, RouteTaskComplexity::Small);
        assert_eq!(assessment.risk, RouteTaskRisk::Low);
        assert_eq!(assessment.confidence, RouteConfidence::Medium);
        assert_eq!(
            assessment.intent,
            RouteTaskIntent::Other,
            "an older probe model that never heard of `intent` must keep working"
        );
        assert_eq!(mock.requests().len(), 1);
    }

    #[test]
    fn live_probe_fails_open_on_a_400_without_panicking_or_retrying() {
        let mock = MockCodex::serving(
            "HTTP/1.1 400 Bad Request",
            "application/json",
            "{\"detail\":\"Stream must be set to true\"}".to_string(),
        );
        let _env = ProbeEnv::pointing_at(mock.addr);

        let assessment = route_probe_assessment_for_tests(
            &inventory(),
            "gpt-5.5-codex",
            "e2e-rejected",
            "anything at all",
        );

        assert!(assessment.is_none(), "a rejected probe must fail open to None");
        assert_eq!(
            mock.requests().len(),
            1,
            "a 400 is terminal for the probe: no retry storm"
        );
    }

    /// The decision shadow beside the live probe, over real sockets on both
    /// sides: a Codex-shaped probe endpoint and a System One endpoint speaking
    /// its documented contract (`docs/design/jev-decision-shadow-20260917.md`
    /// §4 — tools).
    mod decision_shadow {
        use std::ffi::OsString;
        use std::net::SocketAddr;
        use std::path::PathBuf;
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        use runtime::{
            ProbeAssessment, RouteConfidence, RouteTaskComplexity, RouteTaskIntent,
            RouteTaskRisk,
        };

        use zerocode_core::jev::door::Refused;

        use super::super::super::decision_shadow::{
            check_system_one, control_sampled, decision_shadow_path, fire, CheckFailure,
            DecisionRouteUse, DecisionShadowRow, ProbeCell, KEY_CHECK_TASK, OUTCOME_CONTROL,
            PROBE_CONTROL_EVERY,
        };
        use super::super::{probe_and_shadow, task_fingerprint, ProbeSlot};
        use super::{inventory, responses_sse, MockCodex, ProbeEnv};

        /// A wall far past any scripted delay, for the cases not about time.
        const UNHURRIED: Duration = Duration::from_secs(8);
        const PROBE_ANSWER: &str =
            "{\"complexity\":\"medium\",\"risk\":\"low\",\"confidence\":\"high\",\"intent\":\"implementation\"}";

        /// A System One answer exactly as the contract documents one.
        fn judgment_answer() -> String {
            judgment_answer_with("large", "high", "design")
        }

        fn judgment_answer_with(complexity: &str, risk: &str, intent: &str) -> String {
            let distribution = |tokens: &[&str], choice: &str| {
                tokens
                    .iter()
                    .map(|token| ((*token).to_string(), serde_json::json!(if *token == choice { 0.7 } else { 0.1 })))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            };
            serde_json::json!({
                "model": "jev-latest",
                "answers": {
                    "complexity": {"type": "choice", "choice": complexity, "confidence": 0.58,
                        "probabilities": distribution(runtime::COMPLEXITY_AXIS.tokens, complexity)},
                    "risk": {"type": "choice", "choice": risk, "confidence": 0.52,
                        "probabilities": distribution(runtime::RISK_AXIS.tokens, risk)},
                    "intent": {"type": "choice", "choice": intent, "confidence": 0.8,
                        "probabilities": distribution(runtime::INTENT_AXIS.tokens, intent)},
                },
                "usage": {"input_tokens": 431, "output_tokens": 0},
            })
            .to_string()
        }

        fn judging(body: String) -> MockCodex {
            MockCodex::serving("HTTP/1.1 200 OK", "application/json", body)
        }

        /// A task no other case has probed or judged — both memos are
        /// process-wide — and one the control sample leaves alone under an
        /// empty description, so a case that counts probe requests on the
        /// active road is not one time in five a case about the control.
        fn unique(label: &str) -> String {
            unique_for("", label)
        }

        /// [`unique`] for a task probed under `description`: the sample is
        /// drawn on the fingerprint of both fields.
        fn unique_for(description: &str, label: &str) -> String {
            fresh_task(label, |task| !control_sampled(task_fingerprint(description, task)))
        }

        /// A fresh task the control sample picks under `description`.
        fn sampled_for(description: &str, label: &str) -> String {
            fresh_task(label, |task| control_sampled(task_fingerprint(description, task)))
        }

        fn fresh_task(label: &str, wanted: impl Fn(&str) -> bool) -> String {
            loop {
                let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
                let task = format!("decision shadow case {label} {nanos}");
                if wanted(&task) {
                    return task;
                }
            }
        }

        /// The settings, key, endpoints and state directory one case runs
        /// under, restored on drop. The crate's environment lock is taken
        /// before the probe's, the order every holder of both keeps.
        struct ShadowEnv {
            home: tempfile::TempDir,
            _state: tempfile::TempDir,
            /// The ledger under the state directory this case set.
            ledger: PathBuf,
            previous: Vec<(&'static str, Option<OsString>)>,
            _probe: ProbeEnv,
        }

        impl ShadowEnv {
            fn new(probe: SocketAddr, judgment: SocketAddr, key: Option<&str>) -> Self {
                // ProbeEnv owns the one process-wide lock for every override
                // below; acquiring it twice would deadlock this combined case.
                let probe = ProbeEnv::pointing_at(probe);
                let home = tempfile::tempdir().expect("a config home");
                let state = tempfile::tempdir().expect("a state dir");
                let names = [
                    core_types::paths::ZO_CONFIG_HOME_ENV,
                    core_types::paths::ZO_HOME_ENV,
                    "HOME",
                    core_types::paths::ZO_STATE_DIR_ENV,
                    api::SYSTEMONE_API_KEY_ENV,
                    api::SYSTEMONE_BASE_URL_ENV,
                ];
                let previous = names.iter().map(|name| (*name, std::env::var_os(name))).collect();
                std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, home.path());
                std::env::remove_var(core_types::paths::ZO_HOME_ENV);
                std::env::set_var("HOME", home.path());
                std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, state.path());
                match key {
                    Some(key) => std::env::set_var(api::SYSTEMONE_API_KEY_ENV, key),
                    None => std::env::remove_var(api::SYSTEMONE_API_KEY_ENV),
                }
                std::env::set_var(api::SYSTEMONE_BASE_URL_ENV, format!("http://{judgment}"));
                let ledger = decision_shadow_path(&std::env::current_dir().expect("cwd"));
                Self { home, _state: state, ledger, previous, _probe: probe }
            }

            /// Write `smart.decisionShadow`, or leave it unset, with the working
            /// directory consented at the Jev door.
            fn set_mode(&self, mode: Option<&str>) {
                let mut smart = serde_json::json!({ "jev": { "workspaces": [Self::workspace()] } });
                if let Some(mode) = mode {
                    smart["decisionShadow"] = serde_json::json!(mode);
                }
                self.set_smart(&smart);
            }

            /// Write `smart` exactly as given.
            fn set_smart(&self, smart: &serde_json::Value) {
                std::fs::write(self.home.path().join("settings.json"), serde_json::json!({ "smart": smart }).to_string())
                    .expect("write settings");
            }

            fn settings(&self) -> serde_json::Value {
                serde_json::from_str(&std::fs::read_to_string(self.home.path().join("settings.json")).expect("settings"))
                    .expect("json")
            }

            /// The working directory the funnel runs in, as the door spells it.
            fn workspace() -> String {
                zerocode_core::jev::door::resolved_path(&std::env::current_dir().expect("cwd"))
            }

            fn rows(&self) -> Vec<DecisionShadowRow> {
                std::fs::read_to_string(&self.ledger)
                    .map(|text| text.lines().map(|line| serde_json::from_str(line).expect("a ledger row")).collect())
                    .unwrap_or_default()
            }
        }

        impl Drop for ShadowEnv {
            fn drop(&mut self) {
                for (name, value) in self.previous.drain(..) {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }

        /// The funnel, timed to its return, then the shadow it fired awaited to
        /// its end.
        fn funnel(tasks: &[(&str, &str)], deadline: Duration) -> (Vec<Option<ProbeAssessment>>, Duration) {
            let started = Instant::now();
            let (results, shadow) = probe_and_shadow(&inventory(), "gpt-5.5-codex", tasks, "turn-7@1", deadline);
            let returned = started.elapsed();
            if let Some(shadow) = shadow {
                api::sync_bridge::run_blocking(shadow).expect("the shadow batch ran to its end");
            }
            (results, returned)
        }

        fn declines(reason: &str) -> u64 {
            telemetry::harness_attest_snapshot()
                .attestation(telemetry::HarnessFeature::DecisionShadow)
                .declined
                .get(reason)
                .copied()
                .unwrap_or(0)
        }

        #[test]
        fn off_by_default_the_shadow_sends_nothing_and_writes_nothing() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(None);
            let before = declines("off");

            let task = unique("off");
            let (results, _) = funnel(&[("", task.as_str())], UNHURRIED);

            assert!(results[0].is_some(), "the probe still routes");
            assert!(judgment.requests().is_empty(), "off sends nothing");
            assert!(env.rows().is_empty(), "off writes nothing");
            assert!(declines("off") > before, "the shadow was consulted and declined, not skipped");
        }

        #[test]
        fn a_shadow_row_lays_the_probe_and_the_judgment_side_by_side() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("shadow"));

            let task = unique("row");
            let (results, _) = funnel(&[("a description", task.as_str())], UNHURRIED);
            assert!(results[0].is_some());

            let requests = judgment.requests();
            assert_eq!(requests.len(), 1, "one task, one judgment");
            let body: serde_json::Value = serde_json::from_str(&requests[0]).expect("a JSON request");
            assert_eq!(body["state"], runtime::rubric_task_text("a description", &task));
            assert_eq!(body["model"], api::SYSTEMONE_MODEL);
            let asked: Vec<&str> = body["questions"].as_object().expect("questions").keys().map(String::as_str).collect();
            let rubric: Vec<&str> = runtime::decision_questions().keys().map(String::as_str).collect();
            assert_eq!(asked, rubric);

            let rows = env.rows();
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!(row.task, format!("{:016x}", task_fingerprint("a description", &task)));
            assert_eq!(row.attempt.as_deref(), Some("turn-7@1"));
            assert_eq!(row.rubric_version, runtime::DECISION_RUBRIC_VERSION);
            assert_eq!(row.model.as_deref(), Some("jev-latest"));
            assert!(row.answered() && row.called() && !row.cached, "{row:?}");
            assert_eq!((row.retries, row.input_tokens), (0, Some(431)));
            let probed = runtime::parse_probe_response(PROBE_ANSWER).expect("the scripted probe answer");
            for (axis, token) in probed.tokens() {
                assert_eq!(row.probe.token(axis), Some(token), "{}", axis.name);
            }
            assert_eq!(row.probe.token(&runtime::COMPLEXITY_AXIS), Some(RouteTaskComplexity::Medium.as_label()));
            assert_eq!(row.probe.token(&runtime::CONFIDENCE_AXIS), Some(RouteConfidence::High.as_label()));
            let jev = row.jev.as_ref().expect("the judgment");
            assert_eq!(jev[runtime::COMPLEXITY_AXIS.name].choice, RouteTaskComplexity::Large.as_label());
            assert!((jev[runtime::RISK_AXIS.name].probabilities[RouteTaskRisk::High.as_label()] - 0.7).abs() < f64::EPSILON);
            assert!((jev[runtime::INTENT_AXIS.name].confidence - 0.8).abs() < f64::EPSILON);

            // The ledger keeps the fingerprint, never the task's words.
            let text = std::fs::read_to_string(&env.ledger).expect("the ledger");
            assert!(!text.contains(&task) && !text.contains("a description"), "{text}");
        }

        /// `auto` records until something promotes it: the request and the row
        /// `shadow` makes, and routing on the chat probe alone.
        #[test]
        fn auto_records_exactly_as_shadow_does_and_routes_on_the_probe() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("auto"));

            let task = unique("auto");
            let (results, _) = funnel(&[("", task.as_str())], UNHURRIED);

            let probed = runtime::parse_probe_response(PROBE_ANSWER).expect("the scripted probe answer");
            assert_eq!(results, vec![Some(probed)], "the judgment moved no routing");
            assert_eq!(probe.requests().len(), 1, "the chat probe ran, as it does beside a shadow");
            assert_eq!(judgment.requests().len(), 1, "one task, one judgment");
            let rows = env.rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].route_use, DecisionRouteUse::RecordOnly);
            assert!(rows[0].answered(), "{:?}", rows[0]);
        }

        /// A workspace nobody consented to sends nothing: the row names why,
        /// counts no request, and routing is the chat probe's.
        #[test]
        fn a_workspace_nobody_consented_to_sends_nothing_and_its_row_says_why() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_smart(&serde_json::json!({ "decisionShadow": "shadow", "jev": { "workspaces": ["/somewhere/else"] } }));
            let before = declines(Refused::NotConsented.token());

            let (results, _) = funnel(&[("", unique("not-consented").as_str())], UNHURRIED);

            assert!(results[0].is_some(), "the chat probe still routes");
            assert!(judgment.requests().is_empty(), "nothing left the door");
            let rows = env.rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].outcome, Refused::NotConsented.token());
            assert_eq!((rows[0].requests, rows[0].redacted_lines), (Some(0), Some(0)));
            assert!(rows[0].refused() && !rows[0].called());
            assert!(declines(Refused::NotConsented.token()) > before);
        }

        /// A folder under a consented workspace is consented; a folder beside
        /// it is not.
        #[test]
        fn a_folder_under_a_consented_workspace_is_consented_and_one_beside_it_is_not() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            let cwd = std::path::PathBuf::from(ShadowEnv::workspace());
            let parent = cwd.parent().expect("the crate sits in a folder").display().to_string();
            let beside = format!("{}-beside", cwd.display());

            env.set_smart(&serde_json::json!({ "decisionShadow": "shadow", "jev": { "workspaces": [parent] } }));
            funnel(&[("", unique("under").as_str())], UNHURRIED);
            env.set_smart(&serde_json::json!({ "decisionShadow": "shadow", "jev": { "workspaces": [beside] } }));
            funnel(&[("", unique("beside").as_str())], UNHURRIED);

            assert_eq!(judgment.requests().len(), 1, "only the consented folder sent");
            let outcomes: Vec<String> = env.rows().into_iter().map(|row| row.outcome).collect();
            assert_eq!(outcomes, [crate::misc_tools::smart_router::OUTCOME_ANSWERED, Refused::NotConsented.token()]);
        }

        /// Jev switched off, or a day's budget spent, sends nothing whatever
        /// else the settings say.
        #[test]
        fn jev_switched_off_or_a_spent_budget_sends_nothing() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            let workspace = ShadowEnv::workspace();

            env.set_smart(&serde_json::json!({ "decisionShadow": "shadow", "jev": { "enabled": false, "workspaces": [workspace] } }));
            funnel(&[("", unique("switched-off").as_str())], UNHURRIED);
            env.set_smart(&serde_json::json!({ "decisionShadow": "shadow", "jev": { "dailyRequests": 0, "workspaces": [workspace] } }));
            funnel(&[("", unique("budget").as_str())], UNHURRIED);

            assert!(judgment.requests().is_empty());
            let outcomes: Vec<String> = env.rows().into_iter().map(|row| row.outcome).collect();
            assert_eq!(outcomes, [Refused::Off.token(), Refused::Budget.token()]);
        }

        /// A task that carries a credential goes to System One without it: the
        /// bytes the wire received hold none of it, and the row counts the
        /// lines withheld.
        #[test]
        fn a_credential_in_the_task_never_reaches_the_wire() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("shadow"));
            let task = format!(
                "{}\nexport OPENAI_API_KEY=sk-proj-SENTINEL1\nkeep going\nmysql -phunter2SENTINEL2 app",
                // A label with no credential's name in it: only the two lines
                // below are the door's to withhold.
                unique("masked")
            );

            funnel(&[("rotate the deploy key", task.as_str())], UNHURRIED);

            let requests = judgment.requests();
            assert_eq!(requests.len(), 1);
            assert!(!requests[0].contains("SENTINEL"), "a credential reached the wire: {}", requests[0]);
            let body: serde_json::Value = serde_json::from_str(&requests[0]).expect("a JSON request");
            assert!(body["state"].as_str().is_some_and(|state| state.contains("keep going")));
            let rows = env.rows();
            assert_eq!(rows[0].redacted_lines, Some(2));
            assert_eq!(rows[0].requests, Some(1));
        }

        /// A workspace whose ledger holds rows from before the door was one a
        /// person had the judgment on in: it is consented once, in the
        /// person's own settings, and a consent taken back stays taken back.
        #[test]
        fn a_workspace_whose_ledger_predates_the_door_is_consented_once() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            std::fs::create_dir_all(env.ledger.parent().expect("a ledger dir")).expect("ledger dir");
            std::fs::write(
                &env.ledger,
                "{\"at\":1,\"task\":\"0000000000000001\",\"rubricVersion\":1,\"outcome\":\"no_key\",\"elapsedMs\":0,\"retries\":0,\"cached\":false,\"probe\":\"timeout\"}\n",
            )
            .expect("a row from before the door");
            env.set_smart(&serde_json::json!({ "decisionShadow": "shadow" }));

            funnel(&[("", unique("migrated").as_str())], UNHURRIED);

            assert_eq!(judgment.requests().len(), 1, "the migrated workspace sent");
            assert_eq!(env.settings()["smart"]["jev"]["workspaces"], serde_json::json!([ShadowEnv::workspace()]));
            assert_eq!(env.settings()["smart"]["decisionShadow"], "shadow", "nothing else of the file moved");

            // The person takes the consent back: the ledger now holds the door's
            // own rows, so nothing is written again.
            env.set_smart(&serde_json::json!({ "decisionShadow": "shadow", "jev": { "workspaces": [] } }));
            funnel(&[("", unique("taken-back").as_str())], UNHURRIED);

            assert_eq!(judgment.requests().len(), 1, "a consent taken back sends nothing");
            assert_eq!(env.settings()["smart"]["jev"]["workspaces"], serde_json::json!([]));
            let rows = env.rows();
            assert_eq!(rows.last().map(|row| row.outcome.as_str()), Some(Refused::NotConsented.token()));
        }

        #[test]
        fn a_recalled_judgment_is_a_row_with_no_second_request() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("shadow"));

            let task = unique("memo");
            funnel(&[("", task.as_str())], UNHURRIED);
            funnel(&[("", task.as_str())], UNHURRIED);

            assert_eq!(judgment.requests().len(), 1, "the second read recalls, never re-asks");
            let rows = env.rows();
            assert_eq!(rows.len(), 2);
            assert!(!rows[0].cached && rows[1].cached);
            assert!(rows[1].answered() && !rows[1].called());
            assert_eq!(rows[1].jev, rows[0].jev);
            assert_eq!((rows[1].elapsed_ms, rows[1].input_tokens), (0, None), "a recall bills nothing");
        }

        /// A batch that names one task twice — a repeated fan-out member — asks
        /// once: both judgments would start before either could be recalled,
        /// and the second would only bill the same words again.
        #[test]
        fn a_task_a_batch_names_twice_is_judged_once() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("shadow"));

            let task = unique("twice");
            let (results, _) = funnel(&[("", task.as_str()), ("", task.as_str())], UNHURRIED);

            assert!(results.iter().all(Option::is_some), "the probe still answers every slot");
            assert_eq!(judgment.requests().len(), 1, "one task, one judgment");
            assert_eq!(env.rows().len(), 1, "one row per task the batch named");
        }

        #[test]
        fn actual_use_applies_a_validated_judgment_without_spending_probe_tokens() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));

            let task = unique_for("a description", "active-success");
            let (results, _) = funnel(&[("a description", task.as_str())], UNHURRIED);
            let assessment = results[0].expect("validated Jev assessment");
            assert_eq!(
                assessment,
                ProbeAssessment {
                    complexity: RouteTaskComplexity::Large,
                    risk: RouteTaskRisk::High,
                    confidence: RouteConfidence::Medium,
                    intent: RouteTaskIntent::Design,
                }
            );
            assert!(probe.requests().is_empty(), "an answered Jev judgment must skip the chat probe");
            assert_eq!(judgment.requests().len(), 1);

            let fused = runtime::fuse_probe_assessment(
                RouteTaskComplexity::Small,
                RouteTaskRisk::Medium,
                RouteTaskIntent::Implementation,
                &assessment,
            );
            assert_eq!(fused.complexity, RouteTaskComplexity::Medium, "active Jev moves at most one band");
            assert_eq!(fused.risk, RouteTaskRisk::High);
            assert_eq!(fused.intent, RouteTaskIntent::Design);

            let rows = env.rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].route_use, DecisionRouteUse::Applied);
            assert_eq!(rows[0].probe, ProbeCell::Failed("not_run".to_string()));
        }

        #[test]
        fn actual_use_cannot_lower_deterministic_complexity_or_risk() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer_with("trivial", "low", "analysis"));
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));

            let task = unique("active-floor");
            let (results, _) = funnel(&[("", task.as_str())], UNHURRIED);
            let assessment = results[0].expect("validated Jev assessment");
            assert_eq!(assessment.confidence, RouteConfidence::Medium);
            let fused = runtime::fuse_probe_assessment(
                RouteTaskComplexity::Large,
                RouteTaskRisk::Critical,
                RouteTaskIntent::Other,
                &assessment,
            );
            assert_eq!(fused.complexity, RouteTaskComplexity::Large);
            assert_eq!(fused.risk, RouteTaskRisk::Critical);
            assert_eq!(fused.intent, RouteTaskIntent::Analysis);
            assert!(probe.requests().is_empty());
        }

        #[test]
        fn actual_use_deduplicates_a_batch_and_reuses_the_typed_verdict() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));

            let task = unique("active-memo");
            let (first, _) = funnel(&[("", task.as_str()), ("", task.as_str())], UNHURRIED);
            let (second, _) = funnel(&[("", task.as_str())], UNHURRIED);

            assert_eq!(first[0], first[1]);
            assert_eq!(second[0], first[0]);
            assert_eq!(judgment.requests().len(), 1, "duplicate and recalled tasks reuse one typed verdict");
            assert!(probe.requests().is_empty());
            let rows = env.rows();
            assert_eq!(rows.len(), 2, "one row per unique task in each batch");
            assert!(!rows[0].cached && rows[1].cached);
            assert!(rows.iter().all(|row| row.route_use == DecisionRouteUse::Applied));
        }

        #[test]
        fn actual_use_falls_back_for_missing_key_and_invalid_schema() {
            {
                let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
                let judgment = judging(judgment_answer());
                let env = ShadowEnv::new(probe.addr, judgment.addr, None);
                env.set_mode(Some("on"));
                let task = unique("active-no-key");

                let (results, _) = funnel(&[("", task.as_str())], UNHURRIED);

                assert_eq!(results[0].expect("probe fallback").complexity, RouteTaskComplexity::Medium);
                assert_eq!(probe.requests().len(), 1);
                assert!(judgment.requests().is_empty());
                let rows = env.rows();
                assert_eq!(rows[0].outcome, api::SystemOneFailure::NoKey.token());
                assert_eq!(rows[0].route_use, DecisionRouteUse::Fallback);
            }
            {
                let mut broken: serde_json::Value = serde_json::from_str(&judgment_answer()).expect("json");
                broken["answers"]["risk"]["choice"] = serde_json::json!("severe");
                let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
                let judgment = judging(broken.to_string());
                let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
                env.set_mode(Some("on"));
                let task = unique("active-schema");

                let (results, _) = funnel(&[("", task.as_str())], UNHURRIED);

                assert_eq!(results[0].expect("probe fallback").complexity, RouteTaskComplexity::Medium);
                assert_eq!((probe.requests().len(), judgment.requests().len()), (1, 1));
                let rows = env.rows();
                assert_eq!(rows[0].outcome, api::SystemOneFailure::Schema.token());
                assert_eq!(rows[0].route_use, DecisionRouteUse::Fallback);
            }
        }

        #[test]
        fn actual_use_timeout_falls_back_with_one_short_total_wait() {
            const ACTIVE_WALL: Duration = Duration::from_millis(30);
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = MockCodex::serving_after(
                Duration::from_millis(200),
                "HTTP/1.1 200 OK",
                "application/json",
                judgment_answer(),
            );
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));
            let task = unique("active-timeout");

            let (results, waited) = funnel(&[("", task.as_str())], ACTIVE_WALL);

            assert!(results[0].is_some(), "the chat probe supplies the failed slot");
            assert_eq!(probe.requests().len(), 1);
            assert!(waited < Duration::from_millis(150), "active routing exceeded its short wall: {waited:?}");
            let rows = env.rows();
            assert_eq!(rows[0].outcome, api::SystemOneFailure::Timeout.token());
            assert_eq!(rows[0].route_use, DecisionRouteUse::Fallback);
        }

        #[test]
        fn actual_use_batch_waits_for_concurrent_judgments_not_one_wall_per_task() {
            const REPLY_DELAY: Duration = Duration::from_millis(150);
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = MockCodex::serving_after_concurrent(
                REPLY_DELAY,
                "HTTP/1.1 200 OK",
                "application/json",
                judgment_answer(),
            );
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));
            let prompts: Vec<String> = (0..6).map(|index| unique(&format!("active-concurrent-{index}"))).collect();
            let tasks: Vec<(&str, &str)> = prompts.iter().map(|prompt| ("", prompt.as_str())).collect();

            let (results, waited) = funnel(&tasks, Duration::from_millis(500));

            assert!(results.iter().all(|result| result.is_some_and(|assessment| assessment.complexity == RouteTaskComplexity::Large)));
            assert_eq!(judgment.requests().len(), tasks.len());
            assert!(probe.requests().is_empty());
            assert!(waited < REPLY_DELAY * 4, "the batch waited serially: {waited:?}");
            assert!(env.rows().iter().all(|row| row.route_use == DecisionRouteUse::Applied));
        }

        #[test]
        fn actual_use_http_and_json_failures_fall_back_without_retrying() {
            for (status, body, failure) in [
                ("HTTP/1.1 401 Unauthorized", "{}", "unauthorized"),
                ("HTTP/1.1 500 Internal Server Error", "{}", "http_500"),
                ("HTTP/1.1 200 OK", "not json", "schema"),
            ] {
                let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
                let judgment = MockCodex::serving(status, "application/json", body.to_string());
                let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
                env.set_mode(Some("on"));
                let task = unique(failure);

                let (results, _) = funnel(&[("", task.as_str())], UNHURRIED);

                assert_eq!(results[0].expect("probe fallback").complexity, RouteTaskComplexity::Medium);
                assert_eq!((probe.requests().len(), judgment.requests().len()), (1, 1));
                let rows = env.rows();
                assert_eq!(rows[0].outcome, failure);
                assert_eq!(rows[0].route_use, DecisionRouteUse::Fallback);
            }
        }

        #[test]
        fn actual_use_mixed_batch_probes_only_the_failed_slot() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));
            let cached = unique("active-mixed-cached");
            funnel(&[("", cached.as_str())], UNHURRIED);
            drop(judgment);
            let failed = unique("active-mixed-failed");

            let (results, _) = funnel(&[("", cached.as_str()), ("", failed.as_str())], Duration::from_millis(50));

            assert_eq!(results[0].expect("cached Jev").complexity, RouteTaskComplexity::Large);
            assert_eq!(results[1].expect("probe fallback").complexity, RouteTaskComplexity::Medium);
            assert_eq!(probe.requests().len(), 1, "only the failed active slot may spend chat-probe tokens");
            let rows = env.rows();
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[1].route_use, DecisionRouteUse::Applied);
            assert!(rows[1].cached);
            assert_eq!(rows[2].route_use, DecisionRouteUse::Fallback);
        }

        /// One active task in five runs the chat probe it skipped — after the
        /// turn, detached — and writes a control row beside the judgment it
        /// was routed on. The other four spend nothing on the probe.
        #[test]
        fn one_active_task_in_five_runs_the_probe_as_a_control_row_after_the_turn() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));
            let picked = sampled_for("", "control-picked");
            let passed: Vec<String> = (1..PROBE_CONTROL_EVERY)
                .map(|index| unique(&format!("control-passed-{index}")))
                .collect();
            let mut tasks: Vec<(&str, &str)> = vec![("", picked.as_str())];
            tasks.extend(passed.iter().map(|prompt| ("", prompt.as_str())));

            let before = env.rows().len();
            let (results, _) = funnel(&tasks, UNHURRIED);

            assert!(
                results.iter().all(|result| result.is_some_and(|assessment| assessment.complexity == RouteTaskComplexity::Large)),
                "the control moved routing: {results:?}"
            );
            assert_eq!(judgment.requests().len(), tasks.len(), "one judgment per task, none for the control");
            assert_eq!(probe.requests().len(), 1, "one probe for five active tasks: the sampled one's control");

            let rows = env.rows();
            assert_eq!(rows.len() - before, tasks.len() + 1, "five active rows and one control row");
            let picked_task = format!("{:016x}", task_fingerprint("", &picked));
            let active = rows
                .iter()
                .find(|row| row.task == picked_task && row.route_use == DecisionRouteUse::Applied)
                .expect("the sampled task's active row");
            let control = rows.last().expect("a row");
            assert_eq!(control.route_use, DecisionRouteUse::Control, "{control:?}");
            assert_eq!(control.outcome, OUTCOME_CONTROL);
            assert!(!control.answered() && !control.called(), "a control row is not a request");
            assert_eq!(control.task, active.task, "the control stands beside the row it was drawn from");
            assert_eq!((control.requests, control.cached, control.elapsed_ms), (Some(0), false, 0));
            assert_eq!(control.jev, active.jev, "the judgment is copied, never asked again");
            assert_eq!(
                [
                    control.probe.token(&runtime::COMPLEXITY_AXIS),
                    control.probe.token(&runtime::RISK_AXIS),
                    control.probe.token(&runtime::INTENT_AXIS)
                ],
                [Some("medium"), Some("low"), Some("implementation")],
                "the probe's answer, where `not_run` stood"
            );
            assert!(
                rows.iter().filter(|row| row.route_use == DecisionRouteUse::Applied).all(|row| row.probe == ProbeCell::Failed("not_run".to_string())),
                "an active row's own probe cell is still `not_run`"
            );
            // The counter leaves it out; the judge reads it for agreement and
            // for nothing else. This judgment names none of the probe's
            // three answers.
            let ledger = super::super::super::jev_summary::read_rows(&env.ledger);
            assert_eq!(
                ledger.iter().filter(|row| super::super::super::jev_summary::asked_something(row).is_some()).count(),
                before + tasks.len()
            );
            let judged = super::super::super::decision_shadow::judge_rows(&ledger, None).expect("routing is judged");
            assert_eq!((judged.agreement.compared, judged.agreement.agreed, judged.control_rows), (3, 0, 1));
            eprintln!("[decision-shadow control] {}", serde_json::to_string(control).expect("a row serializes"));
        }

        /// A sampled task the judgment failed on has no control row: there is
        /// nothing for the probe to stand beside, and the fallback probe
        /// already ran for routing.
        #[test]
        fn a_sampled_task_the_judgment_failed_on_draws_no_control_row() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = MockCodex::serving("HTTP/1.1 500 Internal Server Error", "application/json", "{}".to_string());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("on"));
            let picked = sampled_for("", "control-failed");

            let (results, _) = funnel(&[("", picked.as_str())], UNHURRIED);

            assert_eq!(results[0].expect("the probe's fallback").complexity, RouteTaskComplexity::Medium);
            assert_eq!(probe.requests().len(), 1, "the routing fallback, and no control after it");
            let rows = env.rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].route_use, DecisionRouteUse::Fallback);
        }

        #[test]
        fn routing_is_the_same_with_the_shadow_on_or_off() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));

            env.set_mode(Some("off"));
            let (off_a, off_b) = (unique("route-off-a"), unique("route-off-b"));
            let (off, _) = funnel(&[("", off_a.as_str()), ("", ""), ("described", off_b.as_str())], UNHURRIED);
            assert!(judgment.requests().is_empty());

            env.set_mode(Some("shadow"));
            let (on_a, on_b) = (unique("route-on-a"), unique("route-on-b"));
            let (on, _) = funnel(&[("", on_a.as_str()), ("", ""), ("described", on_b.as_str())], UNHURRIED);

            assert_eq!(on, off, "the shadow changes no routing result");
            assert_eq!(on[1], None, "an empty task is neither probed nor judged");
            assert_eq!(judgment.requests().len(), 2, "the shadow did run: one judgment per non-empty task");
            assert_eq!(env.rows().len(), 2);
        }

        #[test]
        fn a_failed_probe_is_named_in_the_row_and_still_routes_nothing() {
            let probe = MockCodex::serving("HTTP/1.1 400 Bad Request", "application/json", "{}".to_string());
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("shadow"));

            let (results, _) = funnel(&[("", unique("probe-400").as_str())], UNHURRIED);

            assert_eq!(results, vec![None]);
            let rows = env.rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].probe, ProbeCell::Failed(super::super::FAIL_PROVIDER_FAILURE.to_string()));
            assert!(rows[0].answered(), "the judgment is recorded even when the probe had nothing to say");
        }

        /// The turn never waits on the shadow. Its share of the calling thread
        /// is `fire` — the working directory, the settings roots, the key, each
        /// task cut to the cap, one spawn — timed alone; then the funnel is
        /// timed warm, with the shadow off and on in both orders, while the
        /// judgment answers close to its wall.
        #[test]
        fn the_funnel_returns_in_the_probes_time_while_a_slow_judgment_is_still_out() {
            const WALL: Duration = Duration::from_millis(2_000);
            const JUDGMENT_DELAY: Duration = Duration::from_millis(1_500);
            /// What scheduling noise between two runs may add; far below the delay.
            const NOISE: Duration = Duration::from_millis(250);
            /// `fire` calls timed alone: enough that one slow wake cannot move the median.
            const FIRES: usize = 64;
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = MockCodex::serving_after(JUDGMENT_DELAY, "HTTP/1.1 200 OK", "application/json", judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));

            // One unmeasured pass pays for the first client, runtime and settings read.
            env.set_mode(Some("off"));
            funnel(&[("", unique("latency-warm").as_str())], WALL);

            // `fire` does the same work whatever the mode says — the mode is
            // read detached — so "off" times it without a socket.
            let verdict = runtime::parse_probe_response(PROBE_ANSWER).expect("the scripted probe answer");
            let mut fired = Vec::with_capacity(FIRES);
            let mut batches = Vec::with_capacity(FIRES);
            for index in 0..FIRES {
                let task = unique(&format!("latency-fire-{index}"));
                let slot = ProbeSlot { fingerprint: task_fingerprint("", &task), verdict: Ok(verdict) };
                let started = Instant::now();
                let batch = fire(&[("", task.as_str())], &[Some(slot)], "turn-7@1", WALL);
                fired.push(started.elapsed());
                batches.push(batch.expect("a probed task fires a batch"));
            }
            for batch in batches {
                api::sync_bridge::run_blocking(batch).expect("the batch read the mode and declined");
            }
            fired.sort_unstable();
            assert!(judgment.requests().is_empty(), "off sends nothing");

            // The funnel to its return, and the rows its shadow had written by then.
            let timed = |mode: &str, round: usize| {
                env.set_mode(Some(mode));
                let task = unique(&format!("latency-{mode}-{round}"));
                let before = env.rows().len();
                let started = Instant::now();
                let (_, shadow) = probe_and_shadow(&inventory(), "gpt-5.5-codex", &[("", task.as_str())], "turn-7@1", WALL);
                let returned = started.elapsed();
                let written = env.rows().len() - before;
                if let Some(shadow) = shadow {
                    api::sync_bridge::run_blocking(shadow).expect("the shadow batch ran to its end");
                }
                (returned, written)
            };
            let (off_first, _) = timed("off", 0);
            let (on_second, written_second) = timed("shadow", 0);
            let (on_first, written_first) = timed("shadow", 1);
            let (off_second, _) = timed("off", 1);

            let rows = env.rows();
            eprintln!(
                "[decision-shadow latency] fire (the calling thread's share) median {} us, max {} us over {FIRES}; \
                 funnel returned in {} / {} us with the shadow off, {} / {} us on; \
                 the judgment answered after {} ms (wall {} ms)",
                fired[FIRES / 2].as_micros(),
                fired[FIRES - 1].as_micros(),
                off_first.as_micros(),
                off_second.as_micros(),
                on_first.as_micros(),
                on_second.as_micros(),
                rows.iter().map(|row| row.elapsed_ms.to_string()).collect::<Vec<_>>().join(" / "),
                WALL.as_millis()
            );
            assert_eq!(rows.len(), 2, "a row per shadowed funnel, none while off");
            assert_eq!((written_first, written_second), (0, 0), "the funnel returned before its judgment was written");
            for row in &rows {
                assert!(row.answered(), "{row:?}");
                assert!(row.elapsed_ms >= u64::try_from(JUDGMENT_DELAY.as_millis()).unwrap_or(u64::MAX));
            }
            let (off, on) = (off_first.max(off_second), on_first.max(on_second));
            assert!(on < off + NOISE, "the funnel waited on the shadow: {on:?} with it vs {off:?} without");
            assert!(on < JUDGMENT_DELAY / 3, "{on:?}");
            assert!(fired[FIRES / 2] < NOISE, "fire's median share of the turn: {:?}", fired[FIRES / 2]);
        }

        #[test]
        fn without_a_key_nothing_is_sent_and_the_row_says_no_key() {
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(probe.addr, judgment.addr, None);
            env.set_mode(Some("shadow"));
            let before = declines(api::SystemOneFailure::NoKey.token());

            funnel(&[("", unique("no-key").as_str())], UNHURRIED);

            assert!(judgment.requests().is_empty(), "no key, no request");
            let rows = env.rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].outcome, api::SystemOneFailure::NoKey.token());
            assert!(!rows[0].called() && rows[0].jev.is_none());
            assert!(declines(api::SystemOneFailure::NoKey.token()) > before);
        }

        #[test]
        fn an_answer_that_breaks_the_rubric_is_a_schema_row_that_still_counts_its_tokens() {
            let mut broken: serde_json::Value = serde_json::from_str(&judgment_answer()).expect("json");
            broken["answers"]["risk"]["choice"] = serde_json::json!("severe");
            let probe = MockCodex::sse(responses_sse(PROBE_ANSWER));
            let judgment = judging(broken.to_string());
            let env = ShadowEnv::new(probe.addr, judgment.addr, Some("test-key"));
            env.set_mode(Some("shadow"));

            let task = unique("schema");
            funnel(&[("", task.as_str())], UNHURRIED);
            funnel(&[("", task.as_str())], UNHURRIED);

            let rows = env.rows();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].outcome, api::SystemOneFailure::Schema.token());
            assert_eq!(rows[0].input_tokens, Some(431), "a discarded answer still billed");
            assert!(rows[0].jev.is_none(), "an answer that breaks a rule is discarded whole");
            assert_eq!(judgment.requests().len(), 2, "only an answered judgment is remembered");
        }

        /// A key check puts the shadow's own question — about a task no person
        /// wrote — through the key road the shadow reads, and names the model
        /// that answered: one call, nothing written.
        #[test]
        fn a_key_check_asks_the_shadows_question_about_no_ones_task_and_names_the_model() {
            let judgment = judging(judgment_answer());
            let env = ShadowEnv::new(judgment.addr, judgment.addr, Some("test-key"));

            let check = api::sync_bridge::run_blocking(check_system_one());

            assert_eq!(check.outcome, Ok("jev-latest".to_string()));
            assert_eq!(check.retries, 0);
            let requests = judgment.requests();
            assert_eq!(requests.len(), 1, "one call");
            let body: serde_json::Value = serde_json::from_str(&requests[0]).expect("a JSON request");
            assert_eq!(body["state"], runtime::rubric_task_text("", KEY_CHECK_TASK));
            assert_eq!(body["model"], api::SYSTEMONE_MODEL);
            let asked: Vec<&str> = body["questions"].as_object().expect("questions").keys().map(String::as_str).collect();
            let rubric: Vec<&str> = runtime::decision_questions().keys().map(String::as_str).collect();
            assert_eq!(asked, rubric, "the shadow's own questions");
            assert!(env.rows().is_empty(), "a check writes no ledger row");
        }

        /// A refused key is named by the token a ledger row would carry, after
        /// one call — a 401 is the same answer the second time.
        #[test]
        fn a_refused_key_is_named_by_the_check_after_one_call() {
            let judgment = MockCodex::serving(
                "HTTP/1.1 401 Unauthorized",
                "application/json",
                "{\"error\":\"invalid api key\"}".to_string(),
            );
            let _env = ShadowEnv::new(judgment.addr, judgment.addr, Some("a-refused-key"));

            let check = api::sync_bridge::run_blocking(check_system_one());

            assert_eq!(check.outcome, Err(CheckFailure::Wire(api::SystemOneFailure::Unauthorized)));
            assert_eq!(judgment.requests().len(), 1, "no retry for a refused key");
        }

        /// With no key anywhere the check says so and sends nothing.
        #[test]
        fn a_key_check_without_a_key_sends_nothing() {
            let judgment = judging(judgment_answer());
            let _env = ShadowEnv::new(judgment.addr, judgment.addr, None);

            let check = api::sync_bridge::run_blocking(check_system_one());

            assert_eq!(check.outcome, Err(CheckFailure::Refused(Refused::NoKey)));
            assert!(judgment.requests().is_empty(), "nothing was sent");
        }
    }
}
