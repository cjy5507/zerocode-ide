//! Authenticated coordinator attestations for one executed gate.
//!
//! The coordinator attests a declared input cache through a separately trusted launcher.
//! Files locate ledger evidence. Same-user candidate processes are not an isolated security boundary.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod job_scope;
use job_scope::JobProvenance;
pub use job_scope::{
    BootstrapIdentity, FileIdentity, JobContext, JobIntent, JobMode, JobScope, LibraryIdentity,
    authorize_job, job_lifetime_ms, validate_job, validate_job_inputs, validate_job_receipt,
    validate_libraries,
};

pub const REGISTER: &str = "gate-register";
pub const PIN: &str = "gate-policy-pin";
pub const RECORD: &str = "gate-record";
pub const REUSE: &str = "gate-reuse";
const KEY: &str = "gateEvidenceV1";
const MAX_METADATA: usize = 16 * 1024;
const LEASE_MS: i64 = 2 * 60 * 60 * 1000;
const ALLOWED_GATES: &[&str] = &["gate-root", "gate-zo"];
const MANDATORY_CHECKS: &[&str] = &["native", "windows", "flake-judgment", "publish"];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovedPolicy {
    version: u32,
    policy: String,
    producer: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyPin {
    gate: String,
    definition: ApprovedPolicy,
    generation: u32,
    actor: String,
    pinned_ms: i64,
}

fn current_pin<'a>(run: &'a Run, gate: &str, at: i64) -> Result<(&'a Message, PolicyPin), String> {
    let current = run
        .coordinator_live()
        .ok_or("gate evidence needs a live coordinator")?;
    let (row, value) = run
        .messages
        .iter()
        .rev()
        .find_map(|row| {
            if row.kind != MessageKind::LandCheck
                || row.from != LEDGER_ITSELF
                || row.task.is_some()
                || row.dispatch.is_some()
            {
                return None;
            }
            let body: serde_json::Value = serde_json::from_str(row.payload.as_str()).ok()?;
            (body[KEY]["phase"] == "policy_pinned" && body[KEY]["value"]["gate"] == gate)
                .then(|| (row, body[KEY]["value"].clone()))
        })
        .ok_or("unpinned: gate has no current coordinator policy pin")?;
    let pin: PolicyPin =
        serde_json::from_value(value).map_err(|_| "unsupported gate policy pin")?;
    if pin.generation != current.generation || current.actor.as_deref() != Some(pin.actor.as_str())
    {
        return Err("stale_pin: coordinator changed after gate policy pin".into());
    }
    if pin.pinned_ms != row.created_ms || at < row.created_ms {
        return Err("gate policy pin has an invalid event order".into());
    }
    if pin.definition.version != 1
        || !hex(&pin.definition.policy, &[64])
        || !hex(&pin.definition.producer, &[64])
    {
        return Err("gate policy pin has an unknown identity".into());
    }
    Ok((row, pin))
}

fn policy_matches(run: &Run, inputs: &Inputs, id: Option<&str>, at: i64) -> Result<String, String> {
    let (row, pin) = current_pin(run, &inputs.gate, at)?;
    if id.is_some_and(|id| id != row.id.as_str()) {
        return Err("stale_pin: gate policy pin changed after registration".into());
    }
    if inputs.policy.as_deref() != Some(pin.definition.policy.as_str())
        || inputs.producer.as_deref() != Some(pin.definition.producer.as_str())
    {
        return Err(
            "digest_mismatch: gate policy or producer differs from the current coordinator pin"
                .into(),
        );
    }
    Ok(row.id.clone())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Deterministic,
    Measurement,
    Nondeterministic,
    Unknown,
}

/// Only digests cross this boundary: no raw environment, paths or command arguments.
/// Missing identities remain unknown, even when the gate exits successfully.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inputs {
    pub version: u32,
    pub gate: String,
    pub head: String,
    pub tree: Option<String>,
    pub source: Option<String>,
    pub deps: Option<String>,
    pub lock: Option<String>,
    pub script: Option<String>,
    pub recipe: Option<String>,
    pub toolchain: Option<String>,
    pub platform: Option<String>,
    pub profile: Option<String>,
    pub env: Option<String>,
    pub policy: Option<String>,
    pub producer: Option<String>,
    pub handed_in_head: Option<String>,
    #[serde(default)]
    pub source_ancestor: bool,
    pub scope: Scope,
}

fn hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Inputs {
    fn digests(&self) -> [(&str, Option<&str>); 12] {
        [
            ("source", self.source.as_deref()),
            ("deps", self.deps.as_deref()),
            ("lock", self.lock.as_deref()),
            ("script", self.script.as_deref()),
            ("recipe", self.recipe.as_deref()),
            ("toolchain", self.toolchain.as_deref()),
            ("platform", self.platform.as_deref()),
            ("profile", self.profile.as_deref()),
            ("env", self.env.as_deref()),
            ("tree", self.tree.as_deref()),
            ("policy", self.policy.as_deref()),
            ("producer", self.producer.as_deref()),
        ]
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !hex(&self.head, &[40, 64]) {
            return Err("gate inputs require version 1 and a full lowercase commit id".into());
        }
        if self
            .handed_in_head
            .as_deref()
            .is_some_and(|head| !hex(head, &[40, 64]))
        {
            return Err("invalid handed-in source identity".into());
        }
        if self.gate.is_empty()
            || self.gate.len() > 64
            || !self
                .gate
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("one gate needs one bounded identifier".into());
        }
        for (key, value) in self.digests() {
            let lengths: &[usize] = if key == "tree" { &[40, 64] } else { &[64] };
            if value.is_some_and(|value| !hex(value, lengths)) {
                return Err(format!("invalid {key} identity"));
            }
        }
        Ok(())
    }

    fn reusable(&self) -> Result<(), String> {
        self.validate()?;
        if !ALLOWED_GATES.contains(&self.gate.as_str()) {
            return Err("gate scope is not allowed for reuse".into());
        }
        if self.scope != Scope::Deterministic {
            return Err("gate is unknown, measured or nondeterministic".into());
        }
        for (key, value) in self.digests() {
            if value.is_none() {
                return Err(format!("unknown {key} identity"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Cancelled,
    TimedOut,
    Unknown,
}

/// Exact UTF-8 bytes of this object are hashed at recording, including every field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub version: u32,
    pub registration: String,
    pub policy_pin: String,
    pub lease: String,
    pub execution: String,
    pub attempt: String,
    pub inputs: Inputs,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub duration_ms: u64,
    pub rc: Option<i64>,
    pub outcome: Outcome,
    pub tests: u64,
    pub log_sha256: String,
    pub log_bytes: u64,
    pub inputs_unchanged: bool,
    /// A reuse decision cannot be submitted as an execution.
    pub executed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    #[serde(default)]
    job: Option<JobProvenance>,
    inputs: Inputs,
    #[serde(default)]
    policy_pin: String,
    attempt: String,
    handed_in_head: Option<String>,
    lease: String,
    generation: u32,
    actor: String,
    registered_ms: i64,
    expires_ms: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Completion {
    #[serde(default)]
    job: Option<JobProvenance>,
    metadata: Metadata,
    metadata_sha256: String,
    generation: u32,
    actor: String,
}

/// The consumer recomputes these observations and asks the live ledger, never an exported row.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReuseRequest {
    pub version: u32,
    pub registration: String,
    pub lease: String,
    pub inputs: Inputs,
    pub metadata_sha256: String,
    pub log_sha256: String,
    pub log_bytes: u64,
    pub tests: u64,
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn check_context(
    verb: &str,
    words: &Words,
    job: Option<&JobContext>,
) -> Result<(), String> {
    let scoped = matches!(verb, REGISTER | REUSE)
        || verb == RECORD
            && (words.value("--cancel").is_none() || words.value("--start").is_some());
    if scoped && job.is_none() || job.is_some() && !matches!(verb, REGISTER | RECORD | REUSE) {
        return Err("out_of_scope: this operation needs a server-owned gate job context".into());
    }
    Ok(())
}

fn validate_origin(
    run: &Run,
    id: &str,
    task: &str,
    attempt: &str,
    job: Option<&JobContext>,
    same_job: bool,
) -> Result<(), String> {
    let context = job.ok_or("out_of_scope: gate operation needs a scoped job")?;
    let (_, registered) = registration(run, id, task, attempt)?;
    let origin = registered
        .job
        .as_ref()
        .ok_or("out_of_scope: gate registration has no job provenance")?;
    if context.provenance.bootstrap_sha256 != origin.bootstrap_sha256
        || same_job && context.provenance.id != origin.id
    {
        return Err("out_of_scope: gate operation has different job provenance".into());
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    if text.len() > MAX_METADATA {
        return Err("gate metadata exceeds its size limit".into());
    }
    serde_json::from_str(text).map_err(|error| {
        if error.to_string().contains("unknown field") {
            "unknown_field: unsupported gate request field".into()
        } else {
            "invalid_request: invalid or unsupported gate metadata".into()
        }
    })
}

fn coordinator(run: &Run, team: &Team, pane: &str, actor: &str) -> Result<u32, String> {
    let held = run
        .coordinator_live()
        .ok_or("gate evidence needs a live coordinator")?;
    // actor is supplied by the window's session boundary, never a CLI flag or a JSON field.
    if run.worker_in_pane(&team.id, pane).is_some()
        || held.seat != caller_of(team, pane)
        || !actor
            .strip_prefix(ACTOR_V1)
            .is_some_and(|digest| hex(digest, &[64]))
        || held.actor.as_deref() != Some(actor)
    {
        return Err("gate evidence requires the live coordinator's authenticated session".into());
    }
    Ok(held.generation)
}

fn handed_in(run: &Run, task: &str, attempt: &str, at: i64) -> Result<Option<String>, String> {
    let task_row = run.task(task).ok_or("gate task does not exist")?;
    if matches!(
        task_row.status,
        TaskStatus::Failed | TaskStatus::Closed | TaskStatus::Blocked
    ) || task_row.created_ms > at
    {
        return Err("gate task failed, was cancelled or has an unknown event order".into());
    }
    let newest = run.newest_attempt(task);
    if attempt == NO_ATTEMPT && newest.is_none() {
        return Ok(None);
    }
    let newest = newest.ok_or("gate task has no attempt; name none explicitly")?;
    if newest.id != attempt {
        return Err("gate attempt changed".into());
    }
    if newest.is_open()
        || newest.succeeded != Some(true)
        || newest.ended_ms.is_none_or(|ended| ended < 0 || ended > at)
    {
        return Err("gate attempt failed, was cancelled or has not ended".into());
    }
    let head = newest
        .source
        .as_deref()
        .filter(|head| hex(head, &[40, 64]))
        .ok_or("handed-in source HEAD is unknown")?;
    Ok(Some(head.into()))
}

fn payload(row: &Message, task: &str, attempt: &str, phase: &str) -> Option<serde_json::Value> {
    if row.kind != MessageKind::LandCheck
        || row.from != LEDGER_ITSELF
        || row.task.as_deref() != Some(task)
        || row.dispatch.as_deref()
            != (phase == "completed" && attempt != NO_ATTEMPT).then_some(attempt)
    {
        return None;
    }
    let body: serde_json::Value = serde_json::from_str(row.payload.as_str()).ok()?;
    (body[KEY]["phase"] == phase && body[KEY]["attempt"] == attempt)
        .then(|| body[KEY]["value"].clone())
}

/// Whether a ledger message records gate bookkeeping rather than a merge check.
pub fn is_gate_evidence(row: &Message) -> bool {
    row.kind == MessageKind::LandCheck
        && row.from == LEDGER_ITSELF
        && serde_json::from_str::<serde_json::Value>(row.payload.as_str())
            .ok()
            .is_some_and(|value| value.get(KEY).is_some())
}

fn registration<'a>(
    run: &'a Run,
    id: &str,
    task: &str,
    attempt: &str,
) -> Result<(&'a Message, Registration), String> {
    let row = run
        .messages
        .iter()
        .find(|row| row.id == id)
        .ok_or("registration not found")?;
    let value = payload(row, task, attempt, "registered")
        .ok_or("registration is not ledger evidence for this attempt")?;
    let registered: Registration =
        serde_json::from_value(value).map_err(|_| "unsupported registration")?;
    if registered.registered_ms != row.created_ms || registered.attempt != attempt {
        return Err("registration event does not match its record".into());
    }
    Ok((row, registered))
}

fn event<'a>(
    run: &'a Run,
    registration: &str,
    task: &str,
    attempt: &str,
    phase: &str,
) -> Option<&'a Message> {
    run.messages.iter().find(|row| {
        payload(row, task, attempt, phase)
            .is_some_and(|value| value["registration"] == registration)
    })
}

fn execution<'a>(
    run: &'a Run,
    registered: &Registration,
    metadata: &Metadata,
    task: &str,
    attempt: &str,
) -> Result<&'a Message, String> {
    let row = event(run, &metadata.registration, task, attempt, "started")
        .ok_or("gate has no authenticated execution start")?;
    let value = payload(row, task, attempt, "started").ok_or("unsupported execution start")?;
    if registered.job.is_none() || value["job"] != serde_json::json!(registered.job) {
        return Err("out_of_scope: execution start has different job provenance".into());
    }
    if row.id != metadata.execution
        || value["lease"] != registered.lease
        || metadata.lease != registered.lease
    {
        return Err("gate execution lease changed".into());
    }
    Ok(row)
}

fn live_lease(
    run: &Run,
    id: &str,
    registered: &Registration,
    task: &str,
    attempt: &str,
    at: i64,
) -> Result<(), String> {
    if at < registered.registered_ms || at > registered.expires_ms {
        return Err("gate execution lease expired or has an invalid event order".into());
    }
    if event(run, id, task, attempt, "cancelled").is_some() {
        return Err("gate execution lease was cancelled".into());
    }
    if handed_in(run, task, attempt, registered.registered_ms)? != registered.handed_in_head {
        return Err("gate handed-in source or attempt changed".into());
    }
    Ok(())
}

fn completion<'a>(
    run: &'a Run,
    id: &str,
    task: &str,
    attempt: &str,
) -> Result<(&'a Message, Completion), String> {
    let mut found = run.messages.iter().filter_map(|row| {
        let value = payload(row, task, attempt, "completed")?;
        let completed: Completion = serde_json::from_value(value).ok()?;
        (completed.metadata.registration == id).then_some((row, completed))
    });
    let first = found.next().ok_or("completed gate evidence not found")?;
    if found.next().is_some() {
        return Err("ambiguous completed gate evidence".into());
    }
    Ok(first)
}

fn post(
    ledger: &mut Ledger,
    run: &str,
    task: &str,
    attempt: &str,
    phase: &str,
    value: serde_json::Value,
    at: i64,
) -> Result<String, String> {
    let mut envelope = serde_json::Map::new();
    envelope.insert(
        KEY.into(),
        serde_json::json!({"phase":phase,"attempt":attempt,"value":value}),
    );
    ledger.post(
        run,
        Draft {
            from: LEDGER_ITSELF.into(),
            to: format!("run:{run}"),
            kind: MessageKind::LandCheck,
            body: format!("gate evidence {task}: {phase}").into(),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: serde_json::Value::Object(envelope).to_string().into(),
            thread: None,
            task: Some(task.into()),
            // Only an actual completion is a check execution in attempt event tallies.
            dispatch: (phase == "completed" && attempt != NO_ATTEMPT).then(|| attempt.into()),
        },
        at,
    )
}

fn metadata_events(
    metadata: &Metadata,
    started_at: i64,
    expires_at: i64,
    recorded_at: i64,
) -> Result<(), String> {
    if metadata.version != 1
        || !metadata.executed
        || metadata.started_ms < started_at
        || metadata.ended_ms < metadata.started_ms
        || recorded_at < metadata.ended_ms
        || recorded_at > expires_at
        || !hex(&metadata.log_sha256, &[64])
    {
        return Err("gate execution or event order is unknown or invalid".into());
    }
    Ok(())
}

fn judge(
    run: &Run,
    task: &str,
    attempt: &str,
    request: &ReuseRequest,
    at: i64,
) -> (Option<String>, Result<(), String>) {
    let mut original = None;
    let result = (|| {
        if request.version != 1 {
            return Err("unsupported reuse version".into());
        }
        let (registered_row, registered) = registration(run, &request.registration, task, attempt)?;
        let job = registered
            .job
            .as_ref()
            .filter(|job| !job.id.is_empty() && hex(&job.bootstrap_sha256, &[64]))
            .ok_or("out_of_scope: gate registration has no verified job provenance")?;
        let current = run
            .coordinator_live()
            .ok_or("gate evidence needs a live coordinator")?;
        if current.generation != registered.generation
            || current.actor.as_deref() != Some(registered.actor.as_str())
        {
            return Err("coordinator changed after gate registration".into());
        }
        let (completed_row, completed) = completion(run, &request.registration, task, attempt)?;
        if completed.job.as_ref() != Some(job) {
            return Err("out_of_scope: gate completion has different job provenance".into());
        }
        original = Some(completed_row.id.clone());
        if handed_in(run, task, attempt, registered_row.created_ms)? != registered.handed_in_head {
            return Err("gate handed-in source or attempt changed".into());
        }
        if event(run, &request.registration, task, attempt, "cancelled").is_some() {
            return Err("gate execution lease was cancelled".into());
        }
        if request.lease != registered.lease {
            return Err("gate execution lease changed".into());
        }
        if completed.generation != registered.generation || completed.actor != registered.actor {
            return Err("completion has another coordinator identity".into());
        }
        let metadata = &completed.metadata;
        if metadata.policy_pin != registered.policy_pin {
            return Err("stale_pin: completion names another policy pin".into());
        }
        let started = execution(run, &registered, metadata, task, attempt)?;
        if started.created_ms < registered_row.created_ms {
            return Err("gate start precedes registration".into());
        }
        metadata_events(
            metadata,
            started.created_ms,
            registered.expires_ms,
            completed_row.created_ms,
        )?;
        if at < completed_row.created_ms || completed_row.created_ms < registered_row.created_ms {
            return Err("reuse precedes the recorded execution".into());
        }
        policy_matches(run, &registered.inputs, Some(&registered.policy_pin), at)?;
        registered.inputs.reusable()?;
        request.inputs.reusable()?;
        if metadata.attempt != attempt
            || metadata.inputs != registered.inputs
            || request.inputs != registered.inputs
        {
            return Err("gate inputs changed, including HEAD or policy".into());
        }
        if !metadata.inputs_unchanged {
            return Err("inputs changed during the original execution".into());
        }
        if metadata.outcome != Outcome::Passed || metadata.rc != Some(0) {
            return Err("original gate failed, was cancelled or has an unknown outcome".into());
        }
        if metadata.tests == 0 || request.tests != metadata.tests {
            return Err("no matching nonzero executed test count".into());
        }
        if request.metadata_sha256 != completed.metadata_sha256 {
            return Err("metadata content digest changed".into());
        }
        if request.log_sha256 != metadata.log_sha256
            || request.log_bytes != metadata.log_bytes
            || metadata.log_bytes == 0
        {
            return Err("log content digest or size changed".into());
        }
        Ok(())
    })();
    (original, result)
}

/// Retry receipts preserve answers, not permissions. Recheck before returning a cached hit or
/// letting a producer execute from a replayed registration after a takeover or another attempt.
#[allow(clippy::too_many_arguments)]
pub(super) fn validate_replay(
    ledger: &Ledger,
    team: &Team,
    run_id: &str,
    verb: &str,
    words: &Words,
    pane: &str,
    actor: &str,
    now_ms: i64,
    answer: &str,
    job: Option<&JobContext>,
) -> Result<(), String> {
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    let generation = coordinator(run, team, pane, actor)?;
    if verb == PIN {
        let gate = words
            .value("--gate")
            .ok_or("gate policy pin needs --gate")?;
        let answer: serde_json::Value = parse(answer)?;
        let (row, _) = current_pin(run, gate, now_ms)?;
        if answer["pin"] != row.id {
            return Err("stale_pin: gate policy pin retry no longer names the current pin".into());
        }
        return Ok(());
    }
    let task = words.value("--task").ok_or("gate evidence needs --task")?;
    let attempt = words
        .value("--attempt")
        .ok_or("gate evidence needs --attempt")?;
    let body = words.value("--body").unwrap_or_default();
    if verb == RECORD && words.value("--start").is_some() {
        // A lost start acknowledgement must never authorize a second process.
        return Err("a gate execution start cannot be replayed; inspect its recorded start without executing again".into());
    }
    if verb == RECORD && words.value("--cancel").is_some() {
        return Ok(());
    }
    if verb == REUSE {
        let request: ReuseRequest = parse(body)?;
        validate_origin(run, &request.registration, task, attempt, job, false)?;
        return judge(run, task, attempt, &request, now_ms).1
            .map_err(|_| "gate retry no longer establishes a hit; use a fresh retry name to record the current miss".into());
    }
    let id = if verb == REGISTER {
        let replay: serde_json::Value = parse(answer)?;
        replay["registration"]
            .as_str()
            .ok_or("unsupported gate retry answer")?
            .to_string()
    } else {
        let metadata: Metadata = parse(body)?;
        metadata.registration
    };
    let (row, registered) = registration(run, &id, task, attempt)?;
    validate_origin(run, &id, task, attempt, job, true)?;
    live_lease(run, &id, &registered, task, attempt, now_ms)?;
    if registered.generation != generation || registered.actor != actor || now_ms < row.created_ms {
        return Err("gate retry's coordinator or registration event changed".into());
    }
    policy_matches(
        run,
        &registered.inputs,
        Some(&registered.policy_pin),
        now_ms,
    )?;
    Ok(())
}

/// All three verbs use the actor's durable mutation path and require retry identities.
#[allow(clippy::too_many_arguments)]
pub(super) fn decide(
    ledger: &mut Ledger,
    team: &Team,
    run_id: &str,
    verb: &str,
    words: &Words,
    pane: &str,
    actor: &str,
    now_ms: i64,
    job: Option<&JobContext>,
) -> Result<Decided, String> {
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    let generation = coordinator(run, team, pane, actor)?;
    if now_ms < 0 {
        return Err("invalid gate event time".into());
    }
    if verb == PIN {
        let gate = words
            .value("--gate")
            .ok_or("gate policy pin needs --gate")?;
        if !ALLOWED_GATES.contains(&gate) {
            return Err("gate scope is not allowed for a policy pin".into());
        }
        let definition: ApprovedPolicy = parse(words.value("--body").unwrap_or_default())?;
        if definition.version != 1
            || !hex(&definition.policy, &[64])
            || !hex(&definition.producer, &[64])
        {
            return Err(
                "digest_mismatch: gate policy pin needs exact policy and producer SHA-256 identities".into(),
            );
        }
        let pin = PolicyPin {
            gate: gate.into(),
            definition,
            generation,
            actor: actor.into(),
            pinned_ms: now_ms,
        };
        let id = ledger.post(
            run_id,
            Draft {
                from: LEDGER_ITSELF.into(),
                to: format!("run:{run_id}"),
                kind: MessageKind::LandCheck,
                body: format!("gate policy {gate}: pinned").into(),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: serde_json::json!({(KEY):{"phase":"policy_pinned","value":pin}})
                    .to_string()
                    .into(),
                thread: None,
                task: None,
                dispatch: None,
            },
            now_ms,
        )?;
        return Ok(said(
            serde_json::json!({"pin":id,"gate":gate,"policy":pin.definition.policy,
            "producer":pin.definition.producer,"generation":generation,"executed":false}),
        ));
    }
    let task = words.value("--task").ok_or("gate evidence needs --task")?;
    let attempt = words
        .value("--attempt")
        .ok_or("gate evidence needs --attempt")?;
    let body = words.value("--body").unwrap_or_default();
    match verb {
        REGISTER => {
            let inputs: Inputs = parse(body)?;
            inputs.validate()?;
            let policy_pin = policy_matches(run, &inputs, None, now_ms)?;
            let handed_in_head = handed_in(run, task, attempt, now_ms)?;
            if inputs.handed_in_head != handed_in_head
                || handed_in_head.is_some() && !inputs.source_ancestor
            {
                return Err("candidate has no verified handed-in source ancestry".into());
            }
            let expires_ms = now_ms
                .checked_add(LEASE_MS)
                .ok_or("gate lease time overflow")?
                .min(
                    job.ok_or("out_of_scope: gate registration needs a scoped job")?
                        .expires_ms,
                );
            if expires_ms <= now_ms {
                return Err("expired: gate job lifetime ended".into());
            }
            let lease = ledger.mint("gx-");
            let registered = Registration {
                job: Some(
                    job.ok_or("out_of_scope: gate registration needs a scoped job")?
                        .provenance
                        .clone(),
                ),
                inputs,
                policy_pin,
                attempt: attempt.into(),
                handed_in_head,
                lease: lease.clone(),
                generation,
                actor: actor.into(),
                registered_ms: now_ms,
                expires_ms,
            };
            let id = post(
                ledger,
                run_id,
                task,
                attempt,
                "registered",
                serde_json::json!(registered),
                now_ms,
            )?;
            Ok(said(
                serde_json::json!({"registration":id,"policyPin":registered.policy_pin,"job":registered.job,"lease":lease,"registeredMs":now_ms,"expiresMs":expires_ms,"executed":false}),
            ))
        }
        RECORD if words.value("--start").is_some() || words.value("--cancel").is_some() => {
            if words.value("--start").is_some() && words.value("--cancel").is_some() {
                return Err("a gate record starts or cancels one lease, not both".into());
            }
            let starting = words.value("--start");
            let id = starting
                .or_else(|| words.value("--cancel"))
                .ok_or("gate record needs a registration")?;
            let (_, registered) = registration(run, id, task, attempt)?;
            if words.value("--lease") != Some(registered.lease.as_str()) {
                return Err("gate execution lease changed".into());
            }
            if starting.is_some() {
                validate_origin(run, id, task, attempt, job, true)?;
                if registered.generation != generation || registered.actor != actor {
                    return Err("coordinator changed after gate registration".into());
                }
                policy_matches(
                    run,
                    &registered.inputs,
                    Some(&registered.policy_pin),
                    now_ms,
                )?;
                live_lease(run, id, &registered, task, attempt, now_ms)?;
                let inputs: Inputs = parse(body)?;
                if inputs != registered.inputs {
                    return Err("inputs changed before the execution start".into());
                }
                if event(run, id, task, attempt, "started").is_some() {
                    return Err("gate execution lease has already started".into());
                }
                let execution = post(
                    ledger,
                    run_id,
                    task,
                    attempt,
                    "started",
                    serde_json::json!({"registration":id,"job":registered.job,"lease":registered.lease,"inputsSha256":sha256(body.as_bytes())}),
                    now_ms,
                )?;
                Ok(said(
                    serde_json::json!({"execution":execution,"lease":registered.lease,"startedMs":now_ms,"executed":false}),
                ))
            } else {
                if handed_in(run, task, attempt, registered.registered_ms)?
                    != registered.handed_in_head
                    || now_ms < registered.registered_ms
                {
                    return Err(
                        "gate cancellation names a changed subject or invalid event order".into(),
                    );
                }
                if let Some(row) = event(run, id, task, attempt, "cancelled") {
                    return Ok(said(
                        serde_json::json!({"cancelled":row.id,"executed":false}),
                    ));
                }
                let cancelled = post(
                    ledger,
                    run_id,
                    task,
                    attempt,
                    "cancelled",
                    serde_json::json!({"registration":id,"lease":registered.lease,"generation":generation,"actor":actor}),
                    now_ms,
                )?;
                Ok(said(
                    serde_json::json!({"cancelled":cancelled,"executed":false}),
                ))
            }
        }
        RECORD => {
            let metadata: Metadata = parse(body)?;
            validate_origin(run, &metadata.registration, task, attempt, job, true)?;
            let (registered_row, registered) =
                registration(run, &metadata.registration, task, attempt)?;
            live_lease(
                run,
                &metadata.registration,
                &registered,
                task,
                attempt,
                now_ms,
            )?;
            if registered.generation != generation || registered.actor != actor {
                return Err("coordinator changed after gate registration".into());
            }
            policy_matches(
                run,
                &registered.inputs,
                Some(&registered.policy_pin),
                now_ms,
            )?;
            if metadata.policy_pin != registered.policy_pin {
                return Err("stale_pin: completion names another policy pin".into());
            }
            if metadata.inputs != registered.inputs || metadata.attempt != attempt {
                return Err("gate metadata differs from the registered inputs or attempt".into());
            }
            let started = execution(run, &registered, &metadata, task, attempt)?;
            if started.created_ms < registered_row.created_ms {
                return Err("gate start precedes registration".into());
            }
            metadata_events(&metadata, started.created_ms, registered.expires_ms, now_ms)?;
            let digest = sha256(body.as_bytes());
            if let Ok((row, prior)) = completion(run, &metadata.registration, task, attempt) {
                if prior.metadata_sha256 != digest {
                    return Err("gate completion is immutable".into());
                }
                return Ok(said(
                    serde_json::json!({"original":row.id,"metadataSha256":digest,"recordedMs":row.created_ms}),
                ));
            }
            let completed = Completion {
                job: registered.job.clone(),
                metadata,
                metadata_sha256: digest.clone(),
                generation,
                actor: actor.into(),
            };
            let id = post(
                ledger,
                run_id,
                task,
                attempt,
                "completed",
                serde_json::json!(completed),
                now_ms,
            )?;
            Ok(said(
                serde_json::json!({"original":id,"metadataSha256":digest,"recordedMs":now_ms}),
            ))
        }
        REUSE => {
            let request: ReuseRequest = parse(body)?;
            validate_origin(run, &request.registration, task, attempt, job, false)?;
            let (original, result) = judge(run, task, attempt, &request, now_ms);
            let policy_pin = registration(run, &request.registration, task, attempt)
                .ok()
                .map(|(_, value)| value.policy_pin);
            let current_policy_pin = current_pin(run, &request.inputs.gate, now_ms)
                .ok()
                .map(|(row, _)| row.id.clone());
            let origin_job = registration(run, &request.registration, task, attempt)
                .ok()
                .and_then(|(_, registered)| registered.job);
            let (decision, reason) = match result {
                Ok(()) => (
                    "hit",
                    "authenticated inputs and execution match".to_string(),
                ),
                Err(why) => ("miss", why),
            };
            let value = serde_json::json!({"decision":decision,"reason":reason,"registration":request.registration,"original":original,"executed":false,"newExecutions":0,"gate":request.inputs.gate,"mandatoryChecks":MANDATORY_CHECKS,
                "head":request.inputs.head,"tree":request.inputs.tree,"handedInHead":request.inputs.handed_in_head,
                "recipe":request.inputs.recipe,"policy":request.inputs.policy,"producer":request.inputs.producer,"sourceAncestor":request.inputs.source_ancestor,
                "policyPin":policy_pin,"currentPolicyPin":current_policy_pin,"originalJob":origin_job,"queryJob":job.map(|job| &job.provenance)});
            let id = post(
                ledger,
                run_id,
                task,
                attempt,
                "reuse",
                value.clone(),
                now_ms,
            )?;
            let mut answer = value;
            answer["decisionRecord"] = id.into();
            Ok(said(answer))
        }
        _ => Err("unknown gate evidence verb".into()),
    }
}
