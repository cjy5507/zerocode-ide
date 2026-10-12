use std::path::PathBuf;
use std::time::Duration;

use local_judgment::{Manager, Snapshot};

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
    let root = PathBuf::from(
        args.next()
            .ok_or("managed-smoke requires an isolated root")?,
    );
    let action = args.next().ok_or("managed-smoke requires an action")?;
    let source = args.next().map(PathBuf::from);
    let settings = root.join("settings.json");
    let manager = Manager::new(&root, &settings, |_| {})?;
    let before = manager.snapshot();
    let model = before.models.first().ok_or("empty_catalog")?;
    match action.to_str().ok_or("invalid_action")? {
        "runtime" => {
            if manager.prepare_runtime()?.runtime.state != "ready" {
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
        "run" => {
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
