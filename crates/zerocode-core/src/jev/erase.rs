//! A deleted decision takes its records with it (t-87506).
//!
//! Deleting a built-in decision removes that seat's ledger from every folder a
//! provider's evidence is kept in, and no program writes it again while the
//! seat stays deleted. Deleting a person's own decision removes only its rows,
//! since those decisions share one ledger.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::threshold::{SEAT_KEY, THRESHOLDS_FILE};
use super::{
    CUSTOM_DECISIONS, JEV_USES, JevUse, ROUTING, SMART_SETTINGS_KEY, STALL, builtin, count,
};
use crate::settings_file_lock::SettingsFileLock;

const SETTINGS_FILE: &str = "settings.json";
const PROJECTS_DIR: &str = "projects";
const PROJECT_STATE_DIR: &str = "state";
const PROJECT_LEDGER_DIR: &str = "smart-router";
const PROVIDERS_DIR: &str = "providers";
const UNCONFIGURED_DIR: &str = "provider-unconfigured";
const CUSTOM_ROW_ID: &str = "decision";

/// What a kept seat does once its mark is lifted.
#[derive(Clone, Copy)]
enum AfterLift {
    /// It stands where it stood before the deletion, as a restore would leave
    /// it: its saved word, or no word and the seat's own default.
    AsBefore,
    /// It stays at the mode the settings name.
    StaysAsWritten,
}

/// The seats the person kept after they had deleted them (11-01), lifted once;
/// a later deletion stands. Stall asks again, since its remote beats the rule
/// (54% to 25% over 538 pairs). Routing stays on the rule: the remote answered
/// 0 of 427 requests and laya read below the rule (34.2% to 35.2% over 219
/// pairs, reports/v1.1.113-zo/t-87506), so it stays off until something beats it.
const KEPT_SEATS: [(&JevUse, AfterLift); 2] = [
    (&STALL, AfterLift::AsBefore),
    (&ROUTING, AfterLift::StaysAsWritten),
];
/// Set under `smart` once [`KEPT_SEATS`] have had their one lift.
const LIFTED_KEY: &str = "keptSeatsLifted";

/// Remove the records of built-in decision `id` from the zo home holding
/// `settings`: its ledger in every evidence folder, and its line in the act
/// lines beside each. Returns how many ledgers went.
///
/// # Errors
/// Whatever keeps a file from being removed. The folders are all tried first.
pub fn forget_seat(settings: &Path, id: &str) -> io::Result<usize> {
    let Some(seat) = builtin::seat(id) else {
        return Ok(0);
    };
    if KEPT_SEATS.iter().any(|(kept, _)| kept.id == id) {
        record_lift(settings)?;
    }
    let Some(home) = settings.parent() else {
        return Ok(0);
    };
    let mut removed = 0;
    let mut failed = None;
    for folder in evidence_folders(home) {
        match fs::remove_file(folder.join(seat.ledger)) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => failed = Some(error),
        }
        if let Err(error) = drop_threshold_line(&folder.join(THRESHOLDS_FILE), id) {
            failed = Some(error);
        }
    }
    failed.map_or(Ok(removed), Err)
}

/// Remove the rows of the person's decision `id` from every copy of the shared
/// ledger under `home`, each replaced whole. A ledger left with no line goes.
/// Returns how many rows went.
///
/// # Errors
/// Whatever keeps a ledger from being read or replaced. The folders are all
/// tried first.
pub fn forget_custom(home: &Path, id: &str) -> io::Result<usize> {
    let mut removed = 0;
    let mut failed = None;
    for folder in evidence_folders(home) {
        match drop_custom_rows(&folder.join(CUSTOM_DECISIONS.ledger), id) {
            Ok(rows) => removed += rows,
            Err(error) => failed = Some(error),
        }
    }
    failed.map_or(Ok(removed), Err)
}

/// Whether a row may be appended to `ledger`: false while the seat that keeps
/// this file is deleted in the settings of the home the file stands in. A file
/// no seat keeps, a file outside a zo home and settings that cannot be read
/// are written as asked.
#[must_use]
pub fn may_write(ledger: &Path) -> bool {
    let Some(name) = ledger.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let Some(seat) = JEV_USES.iter().find(|seat| seat.ledger == name) else {
        return true;
    };
    if seat.id == CUSTOM_DECISIONS.id {
        return true;
    }
    let Some(home) = home_of(ledger) else {
        return true;
    };
    let Ok(bytes) = fs::read(home.join(SETTINGS_FILE)) else {
        return true;
    };
    let Ok(root) = serde_json::from_slice::<Value>(&bytes) else {
        return true;
    };
    builtin::in_root(&root, seat.id).is_none_or(|decision| !decision.deleted)
}

/// Lift the deletion marks of [`KEPT_SEATS`], once per settings file, and put
/// each seat where [`AfterLift`] says. Returns whether a mark went; the first
/// run on a file with no marks only records the lift.
///
/// # Errors
/// Whatever keeps the settings from being read or replaced.
pub fn lift_kept_seats(settings: &Path) -> io::Result<bool> {
    if !settings.exists() {
        return Ok(false);
    }
    let _lock = SettingsFileLock::acquire(settings)?;
    let mut root = read_settings(settings)?;
    let Some(smart) = smart_of(&mut root) else {
        return Ok(false);
    };
    if smart.get(LIFTED_KEY).is_some() {
        return Ok(false);
    }
    smart.insert(LIFTED_KEY.to_string(), Value::Bool(true));
    let lifted = lift_marks(&mut root);
    write_settings(settings, &root)?;
    Ok(lifted)
}

/// Note that [`KEPT_SEATS`] had their lift, so a seat deleted before the first
/// lift ran is not brought back by it.
fn record_lift(settings: &Path) -> io::Result<()> {
    if !settings.exists() {
        return Ok(());
    }
    let _lock = SettingsFileLock::acquire(settings)?;
    let mut root = read_settings(settings)?;
    let Some(smart) = smart_of(&mut root) else {
        return Ok(());
    };
    if smart.get(LIFTED_KEY).is_some() {
        return Ok(());
    }
    smart.insert(LIFTED_KEY.to_string(), Value::Bool(true));
    write_settings(settings, &root)
}

/// Take the marks of [`KEPT_SEATS`] out of `root`, and put each seat where
/// [`AfterLift`] says. A typed edit carries the same `deleted` word for older
/// clients, so what counts is what the seat's reader says
/// ([`builtin::in_root`]): an edited seat keeps its edit.
fn lift_marks(root: &mut Value) -> bool {
    let marked: Vec<(&JevUse, AfterLift, Option<String>)> = KEPT_SEATS
        .iter()
        .filter_map(|(seat, after)| {
            let decision = builtin::in_root(root, seat.id)?;
            decision.deleted.then_some((*seat, *after, decision.mode))
        })
        .collect();
    let Some(smart) = root
        .get_mut(SMART_SETTINGS_KEY)
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let mut lifted = false;
    for (seat, after, saved) in marked {
        let removed = smart
            .get_mut(builtin::SETTING)
            .and_then(Value::as_object_mut)
            .and_then(|decisions| decisions.remove(seat.id));
        lifted |= removed.is_some();
        if matches!(after, AfterLift::AsBefore) {
            match saved {
                Some(word) => smart.insert(seat.setting.to_string(), Value::String(word)),
                None => smart.remove(seat.setting),
            };
        }
    }
    lifted
}

fn read_settings(settings: &Path) -> io::Result<Value> {
    serde_json::from_slice(&fs::read(settings)?).map_err(io::Error::other)
}

fn smart_of(root: &mut Value) -> Option<&mut Map<String, Value>> {
    let root = root.as_object_mut()?;
    root.entry(SMART_SETTINGS_KEY)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
}

fn write_settings(settings: &Path, root: &Value) -> io::Result<()> {
    let mut text = serde_json::to_string_pretty(root).map_err(io::Error::other)?;
    text.push('\n');
    replace_keeping_mode(settings, text.as_bytes())
}

/// Temp beside, then rename, carrying the mode of the file it replaces.
fn replace_keeping_mode(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let permissions = fs::metadata(path)?.permissions();
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let written = fs::write(&temp, bytes)
        .and_then(|()| fs::set_permissions(&temp, permissions))
        .and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

fn drop_custom_rows(ledger: &Path, id: &str) -> io::Result<usize> {
    let text = match fs::read(ledger) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut kept = String::with_capacity(text.len());
    let mut removed = 0;
    for line in text.split_inclusive('\n') {
        let names_it = serde_json::from_str::<Value>(line)
            .ok()
            .is_some_and(|row| row.get(CUSTOM_ROW_ID).and_then(Value::as_str) == Some(id));
        if names_it {
            removed += 1;
        } else {
            kept.push_str(line);
        }
    }
    if removed == 0 {
        return Ok(0);
    }
    if kept.is_empty() {
        fs::remove_file(ledger)?;
    } else {
        replace_keeping_mode(ledger, kept.as_bytes())?;
    }
    Ok(removed)
}

fn drop_threshold_line(table: &Path, id: &str) -> io::Result<()> {
    let bytes = match fs::read(table) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let Ok(mut document) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(());
    };
    let Some(rows) = document.as_array_mut() else {
        return Ok(());
    };
    let before = rows.len();
    rows.retain(|row| row.get(SEAT_KEY).and_then(Value::as_str) != Some(id));
    if rows.len() == before {
        return Ok(());
    }
    let text = serde_json::to_string(&document).map_err(io::Error::other)?;
    replace_keeping_mode(table, text.as_bytes())
}

/// Every folder under `home` a seat's ledger can stand in: the machine's and
/// each project's, and in each of those the provider folders beside them.
fn evidence_folders(home: &Path) -> Vec<PathBuf> {
    let mut bases = vec![home.join(count::REQUESTS_DIR)];
    if let Ok(projects) = fs::read_dir(home.join(PROJECTS_DIR)) {
        bases.extend(projects.flatten().map(|project| {
            project
                .path()
                .join(PROJECT_STATE_DIR)
                .join(PROJECT_LEDGER_DIR)
        }));
    }
    let mut folders = Vec::new();
    for base in bases {
        folders.push(base.join(UNCONFIGURED_DIR));
        if let Ok(providers) = fs::read_dir(base.join(PROVIDERS_DIR)) {
            folders.extend(providers.flatten().map(|provider| provider.path()));
        }
        folders.push(base);
    }
    folders
}

/// The zo home a ledger's path stands in, read off the shape of the path:
/// `<home>/jev/…` or `<home>/projects/<slug>/state/smart-router/…`, either
/// with `providers/<identity>` or `provider-unconfigured` in between.
fn home_of(ledger: &Path) -> Option<PathBuf> {
    let mut base = ledger.parent()?;
    if base
        .file_name()
        .is_some_and(|name| name == UNCONFIGURED_DIR)
    {
        base = base.parent()?;
    } else if base
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == PROVIDERS_DIR)
    {
        base = base.parent()?.parent()?;
    }
    if base
        .file_name()
        .is_some_and(|name| name == count::REQUESTS_DIR)
    {
        return base.parent().map(Path::to_path_buf);
    }
    if base
        .file_name()
        .is_some_and(|name| name == PROJECT_LEDGER_DIR)
    {
        let state = base.parent()?;
        let project = state.parent()?;
        let projects = project.parent()?;
        if state
            .file_name()
            .is_some_and(|name| name == PROJECT_STATE_DIR)
            && projects
                .file_name()
                .is_some_and(|name| name == PROJECTS_DIR)
        {
            return projects.parent().map(Path::to_path_buf);
        }
    }
    None
}

#[cfg(test)]
mod tests;
