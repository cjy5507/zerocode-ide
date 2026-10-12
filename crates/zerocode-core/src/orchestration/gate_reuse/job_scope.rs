use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobMode {
    Run,
    Verify,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobIntent {
    pub version: u32,
    pub run: String,
    pub task: String,
    pub attempt: String,
    pub gate: String,
    pub pin: String,
    pub head: String,
    pub mode: JobMode,
    pub registration: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct JobScope {
    pub intent: JobIntent,
    pub generation: u32,
    pub actor: String,
    pub policy: String,
    pub producer: String,
    pub recipe: String,
    pub env: String,
    pub profile: String,
    pub expires_ms: i64,
    pub handed_in_head: Option<String>,
}

fn identity(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256(&bytes))
        .map_err(|_| "invalid_request: gate job identity".into())
}

pub fn authorize_job(
    ledger: &Ledger,
    team: &Team,
    pane: &str,
    actor: &str,
    intent: JobIntent,
    manifest: &str,
    now_ms: i64,
) -> Result<JobScope, String> {
    if manifest.len() > 1024 * 1024
        || intent.version != 1
        || !hex(&intent.head, &[40, 64])
        || intent.task.is_empty()
        || intent.attempt.is_empty()
        || !ALLOWED_GATES.contains(&intent.gate.as_str())
    {
        return Err("invalid_request: gate job declaration".into());
    }
    let run = ledger
        .run(&intent.run)
        .ok_or("invalid_request: gate job run")?;
    let generation = coordinator(run, team, pane, actor)?;
    let (pin_row, pin) = current_pin(run, &intent.gate, now_ms)?;
    if pin_row.id != intent.pin {
        return Err("stale_pin: gate job pin changed".into());
    }
    let policy = sha256(manifest.as_bytes());
    if pin.definition.policy != policy {
        return Err("digest_mismatch: gate job manifest".into());
    }
    let manifest: serde_json::Value =
        serde_json::from_str(manifest).map_err(|_| "invalid_request: gate job manifest")?;
    if manifest["version"] != 2
        || manifest["bundle"]
            .as_object()
            .is_none_or(|bundle| bundle.len() != 4)
    {
        return Err("invalid_request: gate job manifest version or bundle".into());
    }
    let mut bundle = BTreeMap::new();
    for role in ["launcher", "producer", "scanner", "rules"] {
        let value = manifest["bundle"][role]["sha256"]
            .as_str()
            .filter(|value| hex(value, &[64]))
            .ok_or("digest_mismatch: gate job bundle identity")?;
        bundle.insert(role, value);
    }
    let producer = identity(&bundle)?;
    if producer != pin.definition.producer {
        return Err("digest_mismatch: gate job producer".into());
    }
    let selected = &manifest["gates"][&intent.gate];
    let timeout = selected["timeout"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= 7200.0)
        .ok_or("invalid_request: gate job timeout")?;
    let ttl = selected["maxJobTtlSeconds"]
        .as_u64()
        .filter(|value| *value > 0 && *value <= 7200)
        .ok_or("invalid_request: gate job lifetime")?;
    if timeout > ttl as f64
        || selected["target"]["head"] != intent.head
        || selected["target"]["scope"] != "deterministic"
        || selected["target"]["repo"]
            .as_str()
            .is_none_or(str::is_empty)
        || selected["command"]
            .as_array()
            .is_none_or(|values| values.is_empty() || values.iter().any(|value| !value.is_string()))
        || selected["mandatoryChecks"] != serde_json::json!(MANDATORY_CHECKS)
    {
        return Err("digest_mismatch: gate job target, command, scope or lifetime".into());
    }
    let environment = selected["environment"]
        .as_object()
        .ok_or("invalid_request: gate job environment")?;
    let mut pairs = environment
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key, value))
                .ok_or("invalid_request: gate job environment value")
        })
        .collect::<Result<Vec<_>, _>>()?;
    pairs.sort_by(|left, right| left.0.cmp(right.0));
    let profile = selected["profile"]
        .as_str()
        .ok_or("invalid_request: gate job profile")?;
    let handed_in_head = handed_in(run, &intent.task, &intent.attempt, now_ms)?;
    match (&intent.mode, &intent.registration) {
        (JobMode::Run, None) => {}
        (JobMode::Verify, Some(id)) => {
            let (_, registered) = registration(run, id, &intent.task, &intent.attempt)?;
            if registered.policy_pin != intent.pin || registered.inputs.head != intent.head {
                return Err("stale_pin: gate job receipt changed".into());
            }
        }
        _ => return Err("out_of_scope: gate job mode or receipt".into()),
    }
    let seconds = if intent.mode == JobMode::Verify {
        ttl.min(60)
    } else {
        ttl
    };
    Ok(JobScope {
        intent,
        generation,
        actor: actor.into(),
        policy,
        producer,
        recipe: identity(&selected["command"])?,
        env: identity(&pairs)?,
        profile: identity(&[profile, "declared"])?,
        expires_ms: now_ms
            .checked_add((seconds * 1000) as i64)
            .ok_or("invalid_request: gate job lifetime")?,
        handed_in_head,
    })
}

pub fn validate_job(
    ledger: &Ledger,
    team: &Team,
    pane: &str,
    actor: &str,
    scope: &JobScope,
    now_ms: i64,
) -> Result<(), String> {
    if now_ms >= scope.expires_ms {
        return Err("expired: gate job lifetime ended".into());
    }
    let run = ledger
        .run(&scope.intent.run)
        .ok_or("invalid_request: gate job run")?;
    if coordinator(run, team, pane, actor)? != scope.generation || actor != scope.actor {
        return Err("stale_pin: gate job coordinator changed".into());
    }
    let (row, pin) = current_pin(run, &scope.intent.gate, now_ms)?;
    if row.id != scope.intent.pin
        || pin.definition.policy != scope.policy
        || pin.definition.producer != scope.producer
    {
        return Err("stale_pin: gate job policy changed".into());
    }
    if handed_in(run, &scope.intent.task, &scope.intent.attempt, now_ms)? != scope.handed_in_head {
        return Err("out_of_scope: gate job attempt or source changed".into());
    }
    Ok(())
}

pub fn validate_job_inputs(scope: &JobScope, inputs: &Inputs) -> Result<(), String> {
    if inputs.gate != scope.intent.gate
        || inputs.head != scope.intent.head
        || inputs.policy.as_deref() != Some(scope.policy.as_str())
        || inputs.producer.as_deref() != Some(scope.producer.as_str())
        || inputs.recipe.as_deref() != Some(scope.recipe.as_str())
        || inputs.env.as_deref() != Some(scope.env.as_str())
        || inputs.profile.as_deref() != Some(scope.profile.as_str())
        || inputs.scope != Scope::Deterministic
    {
        return Err("out_of_scope: gate job inputs differ from its manifest".into());
    }
    Ok(())
}
