use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use local_judgment::{Manager, Snapshot, SourceInput};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Options {
    phase: String,
    root: PathBuf,
    report: PathBuf,
    source: Option<String>,
    expected_sha: Option<String>,
    expected_revision: Option<String>,
    driver: bool,
}

impl Options {
    fn read() -> Result<Self> {
        let mut options = Self {
            phase: String::new(),
            root: PathBuf::new(),
            report: PathBuf::new(),
            source: None,
            expected_sha: None,
            expected_revision: None,
            driver: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(key) = args.next() {
            if key == "--await-driver" {
                options.driver = true;
                continue;
            }
            let value = args.next().ok_or("missing_option_value")?;
            match key.as_str() {
                "--phase" => options.phase = value,
                "--root" => options.root = value.into(),
                "--report" => options.report = value.into(),
                "--source" => options.source = Some(value),
                "--expect-sha256" => options.expected_sha = Some(value),
                "--expect-revision" => options.expected_revision = Some(value),
                _ => return Err("unknown_option".into()),
            }
        }
        if !matches!(options.phase.as_str(), "candidate" | "final")
            || !options.root.is_absolute()
            || !options.report.is_absolute()
        {
            return Err("explicit_phase_and_paths_required".into());
        }
        let target = fs::canonicalize(std::env::current_dir()?.join("target"))?;
        let parent = fs::canonicalize(options.root.parent().ok_or("root_parent_required")?)?;
        if !parent.starts_with(target)
            || options
                .root
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("owned_target_root_required".into());
        }
        if options.report.starts_with(std::env::current_dir()?) {
            return Err("evidence_must_outlive_checkout".into());
        }
        fs::create_dir_all(&options.report)?;
        Ok(options)
    }
}

fn phase(name: &str, fields: Value) -> Result<()> {
    println!("{}", json!({"phase":name,"details":fields}));
    io::stdout().flush()?;
    Ok(())
}

fn driver(options: &Options, expected: &str) -> Result<()> {
    if options.driver {
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 || line.trim() != expected {
            return Err("driver_command_required".into());
        }
    }
    Ok(())
}

async fn settled(manager: &Manager, started: Snapshot) -> Result<Snapshot> {
    let Some(operation) = started.operation else {
        return Ok(manager.snapshot());
    };
    if !matches!(operation.state, "running" | "cancelling") {
        return Ok(manager.snapshot());
    }
    let until = Instant::now() + Duration::from_secs(180);
    while Instant::now() < until {
        let snapshot = manager.snapshot();
        if let Some(current) = &snapshot.operation {
            if current.id != operation.id {
                return Err("operation_changed".into());
            }
            if !matches!(current.state, "running" | "cancelling") {
                if current.state != "done" {
                    return Err(current.error.unwrap_or("operation_failed").into());
                }
                return Ok(snapshot);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err("operation_timeout".into())
}

async fn cancelled(
    manager: &Manager,
    started: Snapshot,
    options: &Options,
    stage: &str,
) -> Result<Value> {
    let id = started
        .operation
        .as_ref()
        .ok_or("missing_operation")?
        .id
        .clone();
    phase(stage, json!({"operation":id}))?;
    if options.driver {
        driver(options, "cancel")?;
    } else {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let began = Instant::now();
    manager.cancel(&id)?;
    loop {
        let snapshot = manager.snapshot();
        let done = snapshot.operation.as_ref().is_some_and(|operation| {
            operation.id == id && !matches!(operation.state, "running" | "cancelling")
        });
        if done {
            let operation = snapshot.operation.as_ref().ok_or("missing_operation")?;
            if operation.error != Some("cancelled") || snapshot.server.state == "ready" {
                return Err("cancellation_not_confirmed".into());
            }
            return Ok(
                json!({"elapsedMs":began.elapsed().as_secs_f64()*1000.0,"snapshot":snapshot}),
            );
        }
        if began.elapsed() > Duration::from_secs(5) {
            return Err("cancellation_timeout".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn question(kind: &str) -> Value {
    match kind {
        "choice" => {
            json!({"type":"choice","instructions":"Which page did the user request?","criteria":{"help":"Help page","home":"Home page"}})
        }
        "score" => {
            json!({"type":"score","instructions":"How urgent is the request?","criteria":["Routine","Urgent"]})
        }
        _ => json!({"type":"noul","instructions":"Is the page ready?"}),
    }
}

async fn exchange(
    client: &reqwest::Client,
    provider: &judgment_provider::Provider,
    body: Vec<u8>,
) -> Result<Value> {
    let began = Instant::now();
    let mut response = client
        .post(provider.request_url())
        .header("content-type", "application/json")
        .header(judgment_provider::DEADLINE_HEADER, "30000")
        .body(body)
        .send()
        .await?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > judgment_provider::MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err("response_cap".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(
        json!({"status":status.as_u16(),"elapsedMs":began.elapsed().as_secs_f64()*1000.0,"body":serde_json::from_slice::<Value>(&bytes)?}),
    )
}

fn runtime_python(root: &Path, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if entry.file_name() == "python3.12" && path.is_file() {
            return Some(path);
        }
        if path.is_dir()
            && !entry.file_type().ok()?.is_symlink()
            && let Some(found) = runtime_python(&path, depth - 1)
        {
            return Some(found);
        }
    }
    None
}

fn owned_runtime_python(root: &Path) -> Option<PathBuf> {
    runtime_python(&root.join("managed-judgment/runtime"), 5)
}

async fn prove(options: &Options, manager: &Arc<Manager>, report: &mut Value) -> Result<()> {
    let before = manager.snapshot();
    report["before"] = serde_json::to_value(&before)?;
    let settings = options.root.join("settings.json");
    let model_id = if options.phase == "candidate" {
        let source = options.source.as_ref().ok_or("source_required")?;
        let input = if source.starts_with("https://") {
            SourceInput::HuggingFaceFile {
                url: source.clone(),
            }
        } else {
            SourceInput::LocalFile {
                file: source.clone(),
            }
        };
        let inspection = manager.inspect_source(input).await?;
        if options
            .expected_revision
            .as_ref()
            .is_some_and(|expected| inspection.model.revision != *expected)
        {
            return Err("revision_changed".into());
        }
        let fingerprinted: Value = serde_json::to_value(&inspection)?;
        let artifact = fingerprinted["model"]["manifestFingerprint"].clone();
        report["inspection"] = fingerprinted;
        let installed = manager.install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            None,
        )?;
        settled(manager, installed).await?;
        let id = inspection.model.id;
        let model = manager
            .snapshot()
            .models
            .into_iter()
            .find(|model| model.id == id && model.installed)
            .ok_or("model_not_installed")?;
        if options.expected_sha.is_some() {
            let folder = PathBuf::from(
                model
                    .installed_directory
                    .as_ref()
                    .ok_or("model_directory_missing")?,
            );
            let candidates: Vec<_> = fs::read_dir(folder)?
                .flatten()
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
                })
                .collect();
            if candidates.len() != 1 {
                return Err("single_gguf_required".into());
            }
            let mut file = std::fs::File::open(candidates[0].path())?;
            use sha2::{Digest, Sha256};
            use std::io::Read;
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            if options.expected_sha.as_deref() != Some(format!("{:x}", hash.finalize()).as_str()) {
                return Err("model_sha256_changed".into());
            }
        }
        fs::write(
            options.root.join("native-proof-owner.json"),
            serde_json::to_vec(&json!({"version":1,"modelId":id,"manifestFingerprint":artifact}))?,
        )?;
        id
    } else {
        let marker: Value =
            serde_json::from_slice(&fs::read(options.root.join("native-proof-owner.json"))?)?;
        marker["modelId"]
            .as_str()
            .ok_or("owned_marker_required")?
            .to_owned()
    };
    let model = manager
        .snapshot()
        .models
        .into_iter()
        .find(|model| model.id == model_id && model.installed)
        .ok_or("model_not_installed")?;
    settled(
        manager,
        manager.prepare_model_runtime(&model.id, &model.revision)?,
    )
    .await?;
    report["model"] = serde_json::to_value(&model)?;
    phase(
        "runtime_ready",
        json!({"python":owned_runtime_python(&options.root),"modelId":model_id}),
    )?;
    driver(options, "ready")?;

    let loading = manager.start(&model.id, &model.revision)?;
    report["loadCancellation"] = cancelled(manager, loading, options, "load_cancel_window").await?;
    phase("load_cancel_complete", json!({}))?;
    driver(options, "cancelled")?;
    let started = settled(manager, manager.start(&model.id, &model.revision)?).await?;
    let id = started.server.id.clone().ok_or("server_not_ready")?;
    let duplicate = manager.start(&model.id, &model.revision)?;
    if duplicate.server.id.as_deref() != Some(&id) {
        return Err("duplicate_start".into());
    }
    let provider = started.server.provider.clone().ok_or("provider_missing")?;
    report["started"] = serde_json::to_value(started)?;
    settled(manager, manager.check(&id)?).await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(32))
        .build()?;
    let mut positives = 0_u64;
    for kind in ["choice", "noul", "score"] {
        for ordinal in 0..3 {
            let body = json!({"model":provider.identity(),"state":{"requested_page":"Help page","ready":true,"urgency":"routine","nonce":uuid::Uuid::new_v4().to_string()},"questions":{"answer":question(kind)}});
            let prepared = provider.prepare(serde_json::to_vec(&body)?)?;
            let sample = exchange(&client, &provider, prepared.body.clone()).await?;
            report["samples"]
                .as_array_mut()
                .ok_or("samples_missing")?
                .push(json!({"kind":kind,"ordinal":ordinal,"response":sample}));
            let response = &report["samples"]
                .as_array()
                .ok_or("samples_missing")?
                .last()
                .ok_or("sample_missing")?["response"];
            if response["status"] != 200
                || response["body"]["usage"]["completion_tokens"]
                    .as_u64()
                    .is_none_or(|count| count == 0)
            {
                return Err("positive_real_response_required".into());
            }
            provider.decode(&prepared, serde_json::to_vec(&response["body"])?)?;
            positives += 1;
            report["positiveNonCacheResponses"] = json!(positives);
            report["realModelResponses"] = json!(positives);
        }
    }
    for kind in ["choice", "score", "noul"] {
        let mut invalid_question = question(kind);
        invalid_question["instructions"] = Value::Null;
        let valid = json!({"model":provider.identity(),"state":"A controlled invalid request.","questions":{"answer":question(kind)}});
        let prepared = provider.prepare(serde_json::to_vec(&valid)?)?;
        let mut incoming: Value = serde_json::from_slice(&prepared.body)?;
        incoming["messages"][1]["content"] = json!(serde_json::to_string(
            &json!({"state":"A controlled invalid request.","questions":{"answer":invalid_question}})
        )?);
        let denied = exchange(&client, &provider, serde_json::to_vec(&incoming)?).await?;
        if denied["status"].as_u64().is_none_or(|status| status < 400) {
            return Err("invalid_request_was_accepted".into());
        }
        report["refusals"].as_array_mut().ok_or("refusals_missing")?.push(json!({"kind":kind,"response":denied,"boundary":"invalid request rejected before native inference"}));
    }
    let long = json!({"model":provider.identity(),"state":"bounded ".repeat(15000),"questions":{"answer":question("noul")}});
    let prepared = provider.prepare(serde_json::to_vec(&long)?)?;
    let refused = exchange(&client, &provider, prepared.body).await?;
    if refused["status"].as_u64().is_none_or(|status| status < 400) {
        return Err("oversized_context_was_accepted".into());
    }
    report["contextRefusal"] = refused;
    let pending = manager.check(&id)?;
    report["requestCancellation"] =
        cancelled(manager, pending, options, "check_cancel_window").await?;
    phase("check_cancel_complete", json!({}))?;
    driver(options, "cancelled")?;
    let restarted = settled(manager, manager.start(&model.id, &model.revision)?).await?;
    let id = restarted.server.id.as_deref().ok_or("server_not_ready")?;
    settled(manager, manager.check(id)?).await?;
    manager.apply(id, before.provider_identity.as_deref().ok_or("provider_identity_missing")?, |provider, expected, binding| {
        if judgment_provider::Provider::read(&settings).map_err(|_| local_judgment::Error::SettingsChanged)?.identity() != expected { return Err(local_judgment::Error::SettingsChanged); }
        let bytes = serde_json::to_vec(&json!({"smart":{"judgmentProvider":provider,(local_judgment::MANAGED_MODEL_KEY):binding}})).map_err(|_| local_judgment::Error::Storage)?;
        fs::write(&settings, bytes).map_err(|_| local_judgment::Error::Storage)
    })?;
    let began = Instant::now();
    manager.stop(id)?;
    if manager.snapshot().server.state != "stopped" || began.elapsed() > Duration::from_secs(5) {
        return Err("stop_failed".into());
    }
    report["stopElapsedMs"] = json!(began.elapsed().as_secs_f64() * 1000.0);
    phase("stop_complete", json!({}))?;
    driver(options, "stopped")?;
    let last = settled(manager, manager.start(&model.id, &model.revision)?).await?;
    report["parentExitArmed"] = json!(true);
    report["last"] = serde_json::to_value(last)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::read()?;
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let capture = events.clone();
    let sink = move |snapshot: Snapshot| {
        if let Ok(mut events) = capture.lock()
            && events.len() < 1024
        {
            events.push(json!({"operation":snapshot.operation,"server":snapshot.server}));
        }
    };
    let manager = if options.phase == "candidate" {
        #[cfg(feature = "runtime-validation")]
        {
            Manager::for_validation(&options.root, &options.root.join("settings.json"), sink)?
        }
        #[cfg(not(feature = "runtime-validation"))]
        {
            return Err("runtime_validation_feature_required".into());
        }
    } else {
        Manager::new(&options.root, &options.root.join("settings.json"), sink)?
    };
    let mut report = json!({"phase":options.phase,"samples":[],"refusals":[],"realModelResponses":0,"parentCleanupVerifiedByDriver":false});
    let outcome = prove(&options, &manager, &mut report).await;
    report["ok"] = json!(outcome.is_ok());
    report["failure"] = outcome
        .as_ref()
        .err()
        .map(|error| json!(error.to_string()))
        .unwrap_or(Value::Null);
    report["events"] = json!(&*events.lock().map_err(|_| "event_lock_failed")?);
    fs::write(
        options.report.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if outcome.is_ok() {
        phase(
            "parent_exit_armed",
            json!({"report":options.report.join("report.json")}),
        )?;
        std::process::exit(0);
    }
    manager.shutdown();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_observer_receives_python_from_the_owned_managed_store() {
        let root = tempfile::tempdir().unwrap();
        let python = root
            .path()
            .join("managed-judgment/runtime/fixture/python/bin/python3.12");
        fs::create_dir_all(python.parent().unwrap()).unwrap();
        fs::write(&python, b"synthetic interpreter, never executed").unwrap();
        assert_eq!(
            owned_runtime_python(root.path()),
            Some(python),
            "owned_runtime_python_was_not_found"
        );
    }
}
