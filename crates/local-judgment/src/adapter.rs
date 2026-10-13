use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::catalog::{EngineKind, Model, RuntimeProfile};
use crate::{Error, Result, storage};

pub fn local_profile<'profile>(
    model: &Model,
    profile: Option<&'profile RuntimeProfile>,
    allow_unverified: bool,
) -> Result<&'profile RuntimeProfile> {
    match model.engine {
        EngineKind::Laya => {}
        EngineKind::TransformersSystemOne => {
            if model.native_format.as_deref() != Some("d3-code-readout-v1") {
                return Err(Error::ModelFormatUnsupported);
            }
        }
        EngineKind::SystemOneServer => return Err(Error::EngineUnsupported),
    }
    if model.files.is_empty() {
        return Err(Error::ModelManifestMissing);
    }
    let profile = profile
        .filter(|profile| profile.engine == model.engine)
        .ok_or(Error::Unsupported)?;
    if !profile.validated && !allow_unverified {
        return Err(Error::EngineUnverified);
    }
    Ok(profile)
}

pub fn verify_model(directory: &Path, model: &Model, cancel: &AtomicBool) -> Result<()> {
    for artifact in &model.files {
        let path = storage::child(directory, &artifact.path)?;
        storage::verify(&path, artifact, cancel)?;
        if artifact.path.ends_with("config.json") || artifact.path.ends_with(".index.json") {
            let bytes = storage::bounded_read(&path, 1024 * 1024)?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Manifest)?;
            reject_code(&value, model.engine)?;
            if model.engine == EngineKind::TransformersSystemOne
                && artifact.path == "decision_config.json"
                && value.get("format_id").and_then(Value::as_str) != model.native_format.as_deref()
            {
                return Err(Error::ModelFormatUnsupported);
            }
            if artifact.path.ends_with(".index.json") {
                let entries = value
                    .get("weight_map")
                    .and_then(Value::as_object)
                    .ok_or(Error::Manifest)?;
                for entry in entries.values() {
                    let shard = entry.as_str().ok_or(Error::Manifest)?;
                    let shard = crate::catalog::relative(shard)?;
                    let parent = Path::new(&artifact.path).parent().ok_or(Error::Manifest)?;
                    let shard = parent.join(shard);
                    if !model.files.iter().any(|file| {
                        Path::new(&file.path) == shard && file.path.ends_with(".safetensors")
                    }) {
                        return Err(Error::Manifest);
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn reject_code(value: &Value, engine: EngineKind) -> Result<()> {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if name == "trust_remote_code"
                    || (matches!(
                        name.as_str(),
                        "auto_map" | "custom_pipelines" | "chat_template"
                    ) && engine != EngineKind::TransformersSystemOne)
                {
                    return Err(Error::ExecutableArtifact);
                }
                reject_code(value, engine)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_code(value, engine)?;
            }
        }
        _ => {}
    }
    Ok(())
}
