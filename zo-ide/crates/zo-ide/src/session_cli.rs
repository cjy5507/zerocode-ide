//! Session operations without a provider runtime. Writes use the transcript's writer lease.

use std::path::{Path, PathBuf};
use core_types::Session;
use serde_json::json;

pub const USAGE: &str = "zo session list [--query <text>] [--offset <rows>] [--limit <rows>] [--json] [--cwd <dir>]\nzo session fork <id|name|path> [--name <name>] [--cwd <dir>]\nzo session rename <id|name|path> <name> [--cwd <dir>]\nzo session export <id|name|path> <new-file.jsonl> [--cwd <dir>]";

pub fn run(args: &[String], cwd: &Path) -> Result<String, String> {
    if matches!(args, [flag] if flag == "--help" || flag == "-h") { return Ok(USAGE.into()); }
    let mut words = Vec::new();
    let mut cwd = cwd.to_path_buf();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--cwd" {
            cwd = PathBuf::from(args.get(i + 1).ok_or("--cwd requires a directory")?);
            i += 2;
        } else {
            words.push(args[i].clone());
            i += 1;
        }
    }
    let cwd = cwd.canonicalize().map_err(|e| e.to_string())?;
    std::env::set_current_dir(&cwd).map_err(|e| e.to_string())?;
    let Some((verb, rest)) = words.split_first() else { return Err(USAGE.into()); };
    match (verb.as_str(), rest) {
        ("list", rest) => list(rest),
        ("fork", [reference]) => fork_reference(reference, None, &cwd).map(|path| path.display().to_string()),
        ("fork", [reference, flag, name]) if flag == "--name" => {
            fork_reference(reference, Some(name), &cwd).map(|path| path.display().to_string())
        }
        ("rename", [reference, name]) => {
            validate_name(name)?;
            let handle = crate::session_registry::resolve_session_reference(reference).map_err(|e| e.to_string())?;
            let mut session = Session::load_from_path(&handle.path).map_err(|e| e.to_string())?;
            session.name = Some(name.trim().into());
            session.save_to_path(&handle.path).map_err(|e| e.to_string())?;
            Ok(format!("Renamed {} to {}", session.session_id, name.trim()))
        }
        ("export", [reference, destination]) => {
            let handle = crate::session_registry::resolve_session_reference(reference).map_err(|e| e.to_string())?;
            let session = Session::load_from_path(&handle.path).map_err(|e| e.to_string())?;
            export(&session, Path::new(destination))?;
            Ok(destination.clone())
        }
        _ => Err(USAGE.into()),
    }
}

fn list(args: &[String]) -> Result<String, String> {
    let mut query = String::new();
    let mut offset = 0;
    let mut limit = 100;
    let mut as_json = false;
    let mut i = 0;
    while i < args.len() {
        let value = || args.get(i + 1).ok_or_else(|| format!("{} requires a value", args[i]));
        match args[i].as_str() {
            "--json" => { as_json = true; i += 1; }
            "--query" => { query.clone_from(value()?); i += 2; }
            "--offset" => { offset = value()?.parse().map_err(|_| "invalid --offset")?; i += 2; }
            "--limit" => { limit = value()?.parse().map_err(|_| "invalid --limit")?; i += 2; }
            _ => return Err(USAGE.into()),
        }
    }
    let rows = crate::resume::search_sessions(&query, offset, limit).map_err(|e| e.to_string())?;
    if as_json {
        Ok(serde_json::Value::Array(rows.iter().map(|row| json!({
            "id":row.id, "name":row.name, "path":row.path, "cwd":row.cwd,
            "first_prompt":row.first_prompt, "modified_epoch_millis":row.modified_epoch_millis
        })).collect()).to_string())
    } else {
        Ok(rows.iter().map(|row| format!("{}\t{}\t{}", row.id, row.name.as_deref().unwrap_or(""), row.first_prompt.as_deref().unwrap_or(""))).collect::<Vec<_>>().join("\n"))
    }
}

pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    if crate::SESSION_REFERENCE_ALIASES.iter().any(|alias| name.trim().eq_ignore_ascii_case(alias)) { return Err("session name is reserved for a session reference alias".into()); }
    if name.trim().is_empty() { return Err("session name cannot be empty".into()); }
    Ok(())
}

pub(crate) fn fork_reference(reference: &str, name: Option<&str>, cwd: &Path) -> Result<PathBuf, String> {
    let handle = crate::session_registry::resolve_session_reference(reference).map_err(|e| e.to_string())?;
    let source = Session::load_from_path(&handle.path).map_err(|e| e.to_string())?;
    fork_session(&source, &handle.path, name, cwd)
}

pub(crate) fn fork_session(source: &Session, source_path: &Path, name: Option<&str>, cwd: &Path) -> Result<PathBuf, String> {
    if let Some(name) = name { validate_name(name)?; }
    let mut child = source.fork(name.map(str::to_string));
    if let Some(name) = name { child.name = Some(name.trim().into()); }
    let handle = crate::session_registry::create_managed_session_handle_at(&child.session_id, cwd).map_err(|e| e.to_string())?;
    let child = child.with_persistence_path(handle.path.clone());
    child.save_to_path(&handle.path).map_err(|e| e.to_string())?;
    for extension in ["system-prompt.json", "prefs.json"] {
        let from = source_path.with_extension(extension);
        if from.is_file() {
            let data = std::fs::read(from).map_err(|e| e.to_string())?;
            crate::write_atomic(&handle.path.with_extension(extension), &data).map_err(|e| e.to_string())?;
        }
    }
    crate::resume::write_session_cwd_if_missing(&handle.path, cwd).map_err(|e| e.to_string())?;
    Ok(handle.path)
}

pub(crate) fn export(session: &Session, destination: &Path) -> Result<(), String> {
    // Never overwrite another session or a symlink and bypass its writer lease.
    use std::io::Write;
    let bytes = session.render_jsonl_snapshot().map_err(|e| e.to_string())?;
    let parent = destination.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(bytes.as_bytes()).and_then(|()| file.as_file().sync_all()).map_err(|e| e.to_string())?;
    file.persist_noclobber(destination).map_err(|e| e.to_string())?;
    Ok(())
}
