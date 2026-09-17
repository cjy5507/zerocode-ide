//! Stored workflow-spec library (C2).
//!
//! This module owns the persistence format and all writes under
//! `<workflow_store_dir>/library`. The workflow dispatcher may load specs from
//! here, but does not write the library itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::cache::{workflow_store_dir, write_atomic};
use super::spec::WorkflowSpec;
use super::workflow_preview;
use crate::ToolError;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LibraryAction {
    Save,
    List,
    Show,
    Delete,
}

#[derive(Debug, Deserialize)]
struct WorkflowLibraryInput {
    action: LibraryAction,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    spec: Option<Value>,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LibraryEnvelope {
    pub(super) name: String,
    pub(super) saved_at_unix: u64,
    pub(super) spec: Value,
}

pub(crate) fn run(input: &Value) -> Result<String, ToolError> {
    let input: WorkflowLibraryInput = serde_json::from_value(input.clone())
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let dir = library_dir()?;
    let output = match input.action {
        LibraryAction::Save => save(
            &dir,
            required_name(input.name.as_deref())?,
            input.spec
                .ok_or_else(|| ToolError::InvalidInput("`spec` is required for save".into()))?,
            input.overwrite,
        )?,
        LibraryAction::List => list(&dir)?,
        LibraryAction::Show => show(&dir, required_name(input.name.as_deref())?)?,
        LibraryAction::Delete => delete(&dir, required_name(input.name.as_deref())?)?,
    };
    crate::to_pretty_json(output)
}

pub(super) fn load_spec(name: &str) -> Result<Value, ToolError> {
    Ok(load_envelope(&library_dir()?, name)?.spec)
}

pub(super) fn load_envelope_for_projection(name: &str) -> Result<LibraryEnvelope, ToolError> {
    load_envelope(&library_dir()?, name)
}

fn library_dir() -> Result<PathBuf, ToolError> {
    workflow_store_dir()
        .map(|dir| dir.join("library"))
        .ok_or_else(|| ToolError::Execution("workflow store directory is not available".into()))
}

fn required_name(name: Option<&str>) -> Result<&str, ToolError> {
    name.ok_or_else(|| ToolError::InvalidInput("`name` is required".into()))
}

fn validate_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty() || name.len() > 64 {
        return Err(ToolError::InvalidInput(
            "workflow library name must be 1-64 characters".into(),
        ));
    }
    if name.contains('/') || name.contains('\\') || name.contains('.') {
        return Err(ToolError::InvalidInput(format!(
            "invalid workflow library name `{name}`: dots and path separators are not allowed"
        )));
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(ToolError::InvalidInput("workflow library name must not be empty".into()));
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(invalid_name(name));
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_') {
        return Err(invalid_name(name));
    }
    Ok(())
}

fn invalid_name(name: &str) -> ToolError {
    ToolError::InvalidInput(format!(
        "invalid workflow library name `{name}`: expected [a-z0-9][a-z0-9_-]*"
    ))
}

fn entry_path(dir: &Path, name: &str) -> Result<PathBuf, ToolError> {
    validate_name(name)?;
    Ok(dir.join(format!("{name}.json")))
}

fn save(dir: &Path, name: &str, spec: Value, overwrite: bool) -> Result<Value, ToolError> {
    let path = entry_path(dir, name)?;
    let normalized = WorkflowSpec::from_value(&spec)?.validate()?;
    let envelope = LibraryEnvelope {
        name: name.to_string(),
        saved_at_unix: unix_now(),
        spec,
    };
    let text = serde_json::to_string_pretty(&envelope)?;
    if overwrite {
        write_atomic(&path, &text)?;
    } else {
        // Atomic create-only: `create_new` makes "exists" checking and file
        // creation one filesystem operation, so two concurrent `overwrite:
        // false` saves cannot both pass a pre-check and silently clobber each
        // other (the pre-`exists()` + rename pattern had exactly that TOCTOU).
        fs::create_dir_all(dir)?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write as _;
                file.write_all(text.as_bytes())?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(ToolError::InvalidInput(format!(
                    "workflow library entry already exists at {}; pass `overwrite: true` to replace it",
                    path.display()
                )));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(json!({
        "name": envelope.name,
        "path": path.display().to_string(),
        "saved_at_unix": envelope.saved_at_unix,
        "preview": workflow_preview(&normalized)
    }))
}

fn list(dir: &Path) -> Result<Value, ToolError> {
    let mut entries = Vec::new();
    match fs::read_dir(dir) {
        Ok(read_dir) => {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                entries.push(list_entry(&path));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    entries.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    Ok(json!({ "entries": entries, "count": entries.len() }))
}

fn list_entry(path: &Path) -> Value {
    let fallback_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_string();
    let Ok(text) = fs::read_to_string(path) else {
        return json!({ "name": fallback_name, "invalid": true });
    };
    let Ok(envelope) = serde_json::from_str::<LibraryEnvelope>(&text) else {
        return json!({ "name": fallback_name, "invalid": true });
    };
    match WorkflowSpec::from_value(&envelope.spec).and_then(WorkflowSpec::validate) {
        Ok(normalized) => json!({
            "name": envelope.name,
            "saved_at_unix": envelope.saved_at_unix,
            "spec_name": normalized.name,
            "description": normalized.description,
            "mode": super::workflow_mode_label(normalized.mode),
            "phase_count": normalized.phases.len()
        }),
        Err(_) => json!({
            "name": envelope.name,
            "saved_at_unix": envelope.saved_at_unix,
            "invalid": true
        }),
    }
}

fn show(dir: &Path, name: &str) -> Result<Value, ToolError> {
    let envelope = load_envelope(dir, name)?;
    // Lenient like `list`: a stored spec that no longer validates (engine
    // evolution) still shows its envelope — otherwise the caller could never
    // inspect what is stored in order to fix or delete it.
    let preview = match WorkflowSpec::from_value(&envelope.spec).and_then(WorkflowSpec::validate) {
        Ok(normalized) => workflow_preview(&normalized),
        Err(error) => json!({ "valid": false, "error": error.to_string() }),
    };
    Ok(json!({
        "name": envelope.name,
        "saved_at_unix": envelope.saved_at_unix,
        "spec": envelope.spec,
        "preview": preview
    }))
}

fn delete(dir: &Path, name: &str) -> Result<Value, ToolError> {
    let path = entry_path(dir, name)?;
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "workflow library entry not found: {name}"
        )));
    }
    fs::remove_file(&path)?;
    Ok(json!({ "deleted": true, "name": name, "path": path.display().to_string() }))
}

fn load_envelope(dir: &Path, name: &str) -> Result<LibraryEnvelope, ToolError> {
    let path = entry_path(dir, name)?;
    let text = fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ToolError::NotFound(format!("workflow library entry not found: {name}"))
        } else {
            error.into()
        }
    })?;
    serde_json::from_str::<LibraryEnvelope>(&text).map_err(Into::into)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
pub(super) fn save_at_for_test(
    root: &Path,
    name: &str,
    spec: Value,
    overwrite: bool,
) -> Result<Value, ToolError> {
    save(&root.join("library"), name, spec, overwrite)
}

#[cfg(test)]
pub(super) fn load_envelope_at_for_test(root: &Path, name: &str) -> Result<LibraryEnvelope, ToolError> {
    load_envelope(&root.join("library"), name)
}

#[cfg(test)]
pub(super) fn load_spec_at_for_test(root: &Path, name: &str) -> Result<Value, ToolError> {
    Ok(load_envelope_at_for_test(root, name)?.spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-workflow-library-{tag}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn valid_spec() -> Value {
        json!({
            "name": "demo",
            "description": "Demo workflow",
            "mode": "phases",
            "phases": [
                { "id": "read", "prompt": "Read {input}" },
                { "id": "check", "over": "read", "prompt": "Check {item}" }
            ]
        })
    }

    #[test]
    fn save_validates_and_rejects_invalid_spec() {
        let root = temp_root("validate");
        let dir = root.join("library");
        let invalid = json!({ "name": "bad", "phases": [] });

        let err = save(&dir, "bad", invalid, false).expect_err("invalid spec rejected");
        assert!(err.to_string().contains("at least one phase"));
        assert!(!dir.join("bad.json").exists(), "invalid spec is never stored");

        save(&dir, "good", valid_spec(), false).expect("valid spec saves");
        assert!(dir.join("good.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_list_show_delete_roundtrip_with_overwrite_gate_and_corrupt_tolerance() {
        let root = temp_root("roundtrip");
        let dir = root.join("library");
        save(&dir, "demo_flow", valid_spec(), false).expect("save");
        let duplicate = save(&dir, "demo_flow", valid_spec(), false)
            .expect_err("overwrite defaults to false");
        assert!(duplicate.to_string().contains("overwrite: true"));
        save(&dir, "demo_flow", valid_spec(), true).expect("overwrite allowed");
        fs::write(dir.join("corrupt.json"), "{ nope").expect("write corrupt file");

        let listed = list(&dir).expect("list");
        assert_eq!(listed["count"], 2);
        assert!(listed["entries"].as_array().unwrap().iter().any(|entry| {
            entry["name"] == "corrupt" && entry["invalid"] == true
        }));

        let shown = show(&dir, "demo_flow").expect("show");
        assert_eq!(shown["name"], "demo_flow");
        assert_eq!(shown["preview"]["valid"], true);
        assert_eq!(shown["preview"]["phase_count"], 2);

        let deleted = delete(&dir, "demo_flow").expect("delete");
        assert_eq!(deleted["deleted"], true);
        assert!(!dir.join("demo_flow.json").exists());
        let missing = delete(&dir, "demo_flow").expect_err("missing delete is not found");
        assert!(matches!(missing, ToolError::NotFound(_)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn name_path_traversal_is_rejected() {
        let root = temp_root("traversal");
        let dir = root.join("library");
        let err = save(&dir, "../evil", valid_spec(), false).expect_err("reject traversal");
        assert!(err.to_string().contains("path separators"));
        assert!(!root.join("evil.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    /// Engine evolution: an envelope whose stored spec no longer validates
    /// must still be inspectable — `show` returns the envelope with an
    /// invalid-preview marker instead of failing, so the entry can be fixed
    /// or deleted.
    #[test]
    fn show_surfaces_invalid_stored_spec_instead_of_erroring() {
        let root = temp_root("show-invalid");
        let dir = root.join("library");
        fs::create_dir_all(&dir).expect("mkdir");
        let envelope = json!({
            "name": "aged",
            "saved_at_unix": 1,
            "spec": { "name": "aged", "phases": [] }
        });
        fs::write(
            dir.join("aged.json"),
            serde_json::to_string_pretty(&envelope).unwrap(),
        )
        .expect("write envelope");

        let shown = show(&dir, "aged").expect("show tolerates invalid stored spec");
        assert_eq!(shown["name"], "aged");
        assert_eq!(shown["preview"]["valid"], false);
        assert!(shown["preview"]["error"]
            .as_str()
            .unwrap()
            .contains("phase"));
        let _ = fs::remove_dir_all(root);
    }
}
