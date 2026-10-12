use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryIdentity {
    pub path: String,
    pub resolved_path: String,
    pub sha256: String,
}

pub fn validate_libraries(libraries: &[LibraryIdentity]) -> Result<(), String> {
    let mut names = std::collections::BTreeSet::new();
    let mut targets = BTreeMap::new();
    if libraries.len() > 16
        || libraries.iter().any(|entry| {
            !std::path::Path::new(&entry.path).is_absolute()
                || !std::path::Path::new(&entry.resolved_path).is_absolute()
                || std::path::Path::new(&entry.path)
                    .components()
                    .any(|part| part == std::path::Component::ParentDir)
                || std::path::Path::new(&entry.resolved_path)
                    .components()
                    .any(|part| part == std::path::Component::ParentDir)
                || !hex(&entry.sha256, &[64])
                || !names.insert(&entry.path)
                || targets
                    .insert(&entry.resolved_path, &entry.sha256)
                    .is_some_and(|previous| previous != &entry.sha256)
        })
    {
        return Err(
            "digest_mismatch: runtime library declarations are invalid or duplicated".into(),
        );
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapIdentity {
    pub launcher: FileIdentity,
    pub python: FileIdentity,
    pub stdlib: FileIdentity,
    pub libraries: Vec<LibraryIdentity>,
}

impl BootstrapIdentity {
    pub fn from_manifest(manifest: &serde_json::Value) -> Result<Self, String> {
        let result: Self = serde_json::from_value(serde_json::json!({
            "launcher":manifest["bundle"]["launcher"],
            "python":manifest["runtime"]["python"],
            "stdlib":manifest["runtime"]["stdlib"],
            "libraries":manifest["runtime"]["libraries"]
        }))
        .map_err(|_| "digest_mismatch: gate bootstrap declaration")?;
        if [&result.launcher, &result.python, &result.stdlib]
            .iter()
            .any(|entry| {
                !std::path::Path::new(&entry.path).is_absolute() || !hex(&entry.sha256, &[64])
            })
        {
            return Err("digest_mismatch: gate bootstrap identity".into());
        }
        validate_libraries(&result.libraries)?;
        Ok(result)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct JobProvenance {
    pub id: String,
    pub bootstrap_sha256: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct JobContext {
    pub(super) provenance: JobProvenance,
    pub(super) expires_ms: i64,
}

impl JobContext {
    pub fn from_session(id: &str, scope: &JobScope) -> Result<Self, String> {
        if id.is_empty() || id.len() > MAX_NAME {
            return Err("out_of_scope: gate job identity".into());
        }
        Ok(Self {
            provenance: JobProvenance {
                id: id.into(),
                bootstrap_sha256: identity(&scope.bootstrap)?,
            },
            expires_ms: scope.expires_ms,
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        Self {
            provenance: JobProvenance {
                id: "gj-unit-fixture".into(),
                bootstrap_sha256: "b".repeat(64),
            },
            expires_ms: i64::MAX,
        }
    }
}

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
    pub bootstrap: BootstrapIdentity,
}

fn identity(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256(&bytes))
        .map_err(|_| "invalid_request: gate job identity".into())
}

pub fn job_lifetime_ms(selected: &serde_json::Value, mode: &JobMode) -> Result<u64, String> {
    let timeout = selected["timeout"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= 7200.0)
        .ok_or("invalid_request: gate execution timeout")?;
    let seconds = |name: &str, cap: u64| {
        selected[name]
            .as_u64()
            .filter(|value| *value > 0 && *value <= cap)
            .ok_or_else(|| format!("invalid_request: gate {name}"))
    };
    let overhead = seconds("overheadTimeoutSeconds", 3600)?;
    let verification = seconds("verificationTimeoutSeconds", 3600)?;
    let maximum = seconds("maxJobTtlSeconds", 7200)?;
    if timeout + overhead as f64 > maximum as f64 || verification > maximum {
        return Err("invalid_request: gate phase budgets exceed maximum job lifetime".into());
    }
    Ok(match mode {
        JobMode::Run => (timeout * 1000.0).ceil() as u64 + overhead * 1000,
        JobMode::Verify => verification * 1000,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn authorize_job(
    ledger: &Ledger,
    team: &Team,
    pane: &str,
    actor: &str,
    intent: JobIntent,
    manifest: &str,
    bootstrap: BootstrapIdentity,
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
    if manifest["version"] != 3
        || manifest["bundle"]
            .as_object()
            .is_none_or(|bundle| bundle.len() != 4)
    {
        return Err("invalid_request: gate job manifest version or bundle".into());
    }
    if BootstrapIdentity::from_manifest(&manifest)? != bootstrap {
        return Err("digest_mismatch: native gate bootstrap differs from the policy pin".into());
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
    let lifetime_ms = job_lifetime_ms(selected, &intent.mode)?;
    if selected["target"]["head"] != intent.head
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
            let bootstrap_digest = identity(&bootstrap)?;
            if registered
                .job
                .as_ref()
                .is_none_or(|job| job.bootstrap_sha256 != bootstrap_digest)
            {
                return Err("out_of_scope: gate receipt has different bootstrap provenance".into());
            }
        }
        _ => return Err("out_of_scope: gate job mode or receipt".into()),
    }
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
            .checked_add(lifetime_ms as i64)
            .ok_or("invalid_request: gate job lifetime")?,
        handed_in_head,
        bootstrap,
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
    if coordinator(run, team, pane, actor).map_err(|_| "stale_pin: gate job coordinator changed")?
        != scope.generation
        || actor != scope.actor
    {
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

pub fn validate_job_receipt(ledger: &Ledger, scope: &JobScope, id: &str) -> Result<(), String> {
    let run = ledger
        .run(&scope.intent.run)
        .ok_or("out_of_scope: gate job run")?;
    registration(run, id, &scope.intent.task, &scope.intent.attempt)?;
    if event(
        run,
        id,
        &scope.intent.task,
        &scope.intent.attempt,
        "cancelled",
    )
    .is_some()
    {
        return Err("out_of_scope: gate job receipt was cancelled".into());
    }
    Ok(())
}
