use std::path::PathBuf;
use std::time::Duration;

use local_judgment::{Manager, Snapshot, SourceInput};

async fn settled(manager: &Manager) -> Result<Snapshot, Box<dyn std::error::Error>> {
    loop {
        let snapshot = manager.snapshot();
        if let Some(operation) = &snapshot.operation
            && !matches!(operation.state, "running" | "cancelling")
        {
            if operation.state != "done" {
                return Err(operation.error.unwrap_or("operation_failed").into());
            }
            return Ok(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let first = args
        .next()
        .ok_or("managed-smoke requires an isolated root")?;
    let validation = first == "--validate-candidate";
    let root = PathBuf::from(if validation {
        args.next()
            .ok_or("managed-smoke requires an isolated root")?
    } else {
        first
    });
    let action = args.next().ok_or("managed-smoke requires an action")?;
    let action = action.to_str().ok_or("invalid_action")?;
    let source = args
        .next()
        .filter(|source| source != "-")
        .map(PathBuf::from);
    let model_id = args.next().map(|id| id.to_string_lossy().into_owned());
    let settings = root.join("settings.json");
    let manager = if validation {
        #[cfg(feature = "runtime-validation")]
        {
            Manager::for_validation(&root, &settings, |_| {})?
        }
        #[cfg(not(feature = "runtime-validation"))]
        {
            return Err("runtime_validation_feature_required".into());
        }
    } else {
        Manager::new(&root, &settings, |_| {})?
    };
    let before = manager.snapshot();
    let mut selected_id = model_id.clone();
    if matches!(action, "validate-hf" | "validate-local") {
        if !validation {
            return Err("candidate_validation_mode_required".into());
        }
        let source = source.as_ref().ok_or("validation_source_required")?;
        let input = if action == "validate-hf" {
            let expected = before
                .models
                .iter()
                .find(|model| Some(&model.id) == model_id.as_ref())
                .ok_or("pinned_validation_model_required")?;
            SourceInput::HuggingFace {
                repository: source.to_string_lossy().into_owned(),
                revision: Some(expected.revision.clone()),
                subfolder: None,
            }
        } else {
            SourceInput::LocalDirectory {
                directory: source.to_string_lossy().into_owned(),
            }
        };
        let inspection = manager.inspect_source(input).await?;
        if action == "validate-hf"
            && !before.models.iter().any(|model| {
                Some(&model.id) == model_id.as_ref()
                    && model.manifest_fingerprint == inspection.model.manifest_fingerprint
            })
        {
            return Err("source_manifest_changed".into());
        }
        manager.install_source(
            &inspection.inspection_id,
            &inspection.instance_id,
            &inspection.catalog_fingerprint,
            None,
        )?;
        settled(&manager).await?;
        selected_id = Some(inspection.model.id);
    }
    let current = manager.snapshot();
    let model = match selected_id.as_deref() {
        Some(id) => current
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or("unknown_model")?,
        None => current.models.first().ok_or("empty_catalog")?,
    };
    if validation {
        if !matches!(action, "validate" | "validate-hf" | "validate-local") {
            return Err("candidate_requires_complete_validation".into());
        }
        manager.prepare_model_runtime(&model.id, &model.revision)?;
        settled(&manager).await?;
        if action == "validate" {
            manager.install(&model.id, &model.revision, source.clone())?;
            settled(&manager).await?;
        }
    }
    match action {
        "runtime" => {
            if model.runtime.state != "ready" {
                manager.prepare_model_runtime(&model.id, &model.revision)?;
                settled(&manager).await?;
            }
        }
        "install" => {
            let installed = manager.install(&model.id, &model.revision, source)?;
            if !installed
                .models
                .iter()
                .any(|installed| installed.id == model.id && installed.installed)
            {
                settled(&manager).await?;
            }
        }
        "run" | "validate" | "validate-hf" | "validate-local" => {
            manager.start(&model.id, &model.revision)?;
            let started = settled(&manager).await?;
            let id = started.server.id.ok_or("server_not_ready")?;
            let duplicate = manager.start(&model.id, &model.revision)?;
            if duplicate.server.id.as_deref() != Some(&id) {
                return Err("duplicate_start_changed_server".into());
            }
            manager.check(&id)?;
            settled(&manager).await?;
            let applied = manager.apply(
                &id,
                before
                    .provider_identity
                    .as_deref()
                    .ok_or("provider_unknown")?,
                |provider, expected, binding| {
                    let current = judgment_provider::Provider::read(&settings)
                        .map_err(|_| local_judgment::Error::SettingsChanged)?;
                    if current.identity() != expected {
                        return Err(local_judgment::Error::SettingsChanged);
                    }
                    let bytes = serde_json::to_vec(
                        &serde_json::json!({"smart":{"judgmentProvider":provider,(local_judgment::MANAGED_MODEL_KEY):binding}}),
                    )
                    .map_err(|_| local_judgment::Error::Storage)?;
                    std::fs::write(&settings, bytes).map_err(|_| local_judgment::Error::Storage)
                },
            )?;
            if applied.applied_model_id.as_deref() != Some(&model.id) {
                return Err("apply_failed".into());
            }
            manager.stop(&id)?;
            if manager.snapshot().configured_model_id.as_deref() != Some(&model.id) {
                return Err("configuration_lost".into());
            }
        }
        "parent-eof" => {
            manager.start(&model.id, &model.revision)?;
            let started = settled(&manager).await?;
            if started.server.state != "ready" {
                return Err("server_not_ready".into());
            }
            println!("{{\"parentExit\":true}}");
            std::process::exit(0);
        }
        "status" => {}
        _ => return Err("unknown_action".into()),
    }
    let mut result = serde_json::to_value(manager.snapshot())?;
    if let Some(provider) = result
        .get_mut("server")
        .and_then(|server| server.get_mut("provider"))
    {
        *provider = serde_json::Value::Null;
    }
    println!("{}", serde_json::to_string(&result)?);
    manager.shutdown();
    Ok(())
}
