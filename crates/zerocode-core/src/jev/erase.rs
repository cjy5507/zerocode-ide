//! A deleted decision takes its records with it (t-87506).
//!
//! Deleting a built-in decision removes that seat's ledger from every folder a
//! provider's evidence is kept in, and no program writes it again while the
//! seat stays deleted. Deleting a person's own decision removes only its rows,
//! since those decisions share one ledger.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::custom::store;
use super::threshold::{SEAT_KEY, THRESHOLDS_FILE};
use super::{
    CUSTOM_DECISIONS, JEV_USES, JevMode, JevUse, ROUTING, SMART_SETTINGS_KEY, STALL, builtin, count,
};
use crate::held_file_lock::HeldFileLock;
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
    /// It stays at the mode the settings name. No word reads as Auto with the
    /// switch on, so a seat with none is written off.
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
/// Set under `smart` once the first start has erased what older deletes left.
const SWEPT_KEY: &str = "deletedRecordsSwept";
/// The file under a zo home naming what a delete could not erase. Only a
/// delete that failed writes it, so a usual start asks one question of the
/// disk and reads no evidence folder: this machine has 2,540 of them, 205 ms a
/// pass with a warm cache (11-01).
pub const PENDING_FILE: &str = "jev-forget-pending.json";
/// How long a writer of the shared ledger waits for another. Each holds it for
/// one append or one rewrite.
const SHARED_LEDGER_WAIT: Duration = Duration::from_secs(2);
const SHARED_LEDGER_POLL: Duration = Duration::from_millis(1);

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
    let (folders, unread) = evidence_folders(home);
    let removed = forget_in(&folders, seat)?;
    unread.map_or(Ok(removed), Err)
}

fn forget_in(folders: &[PathBuf], seat: &JevUse) -> io::Result<usize> {
    let mut removed = 0;
    let mut failed = None;
    for folder in folders {
        match fs::remove_file(folder.join(seat.ledger)) {
            Ok(()) => removed += 1,
            Err(error) if is_absent(&error) => {}
            Err(error) => failed = Some(error),
        }
        if let Err(error) = drop_threshold_line(&folder.join(THRESHOLDS_FILE), seat.id) {
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
    let (folders, unread) = evidence_folders(home);
    let mut removed = 0;
    let mut failed = unread;
    for folder in folders {
        match drop_custom_rows(&folder.join(CUSTOM_DECISIONS.ledger), |row| row == id) {
            Ok(rows) => removed += rows,
            Err(error) => failed = Some(error),
        }
    }
    failed.map_or(Ok(removed), Err)
}

/// The second half of the window's delete of built-in decision `id`. The mark
/// is already written and the list already changed, so a record that will not
/// go does not undo or fail the delete: it is logged and owed to the next start.
pub fn seat_deleted(settings: &Path, id: &str) {
    if let Err(error) = forget_seat(settings, id)
        && let Some(home) = settings.parent()
    {
        owe(home, Owed::Seat(id), &error);
    }
}

/// The second half of the window's delete of the person's decision `id`, on
/// the same terms as [`seat_deleted`].
pub fn decision_deleted(home: &Path, id: &str) {
    if let Err(error) = forget_custom(home, id) {
        owe(home, Owed::Decision(id), &error);
    }
}

/// What the window's start owes the records, off the interface thread. Not
/// after a failed lift: a mark about to go must not cost its seat's records.
///
/// The first start erases what every marked seat left, and the rows of
/// decisions the store no longer keeps. Every later one re-erases only what
/// [`PENDING_FILE`] names, and is one look at that file when there is none.
pub fn tidy_at_start(settings: &Path, lifted: bool) {
    if !lifted {
        return;
    }
    let Some(home) = settings.parent() else {
        return;
    };
    if settle_owed(settings, home) && !start_over(settings, home) {
        return;
    }
    let Ok(root) = read_settings(settings) else {
        return;
    };
    if root
        .get(SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get(SWEPT_KEY))
        .is_some()
    {
        return;
    }
    let (folders, unread) = evidence_folders(home);
    let mut finished = true;
    if let Some(error) = unread {
        log_failure("list every evidence folder", &error);
        finished = false;
    }
    for seat in JEV_USES.iter().filter(|seat| is_marked(&root, seat.id)) {
        if let Err(error) = forget_in(&folders, seat) {
            finished &= owe(home, Owed::Seat(seat.id), &error);
        }
    }
    finished &= sweep_decision_rows(home, &folders);
    if finished && let Err(error) = record_swept(settings) {
        log_failure("record the first tidy", &error);
    }
}

/// A note that no longer parses names nothing anyone can read, so the first
/// tidy runs again and notes afresh what still will not go. The key goes first:
/// if the note could not then be removed, the next start finds it again.
fn start_over(settings: &Path, home: &Path) -> bool {
    let cleared = (|| {
        let _lock = SettingsFileLock::acquire(settings)?;
        let mut root = read_settings(settings)?;
        if let Some(smart) = smart_of(&mut root)
            && smart.remove(SWEPT_KEY).is_some()
        {
            write_settings(settings, &root)?;
        }
        fs::remove_file(home.join(PENDING_FILE))
    })();
    if let Err(error) = &cleared {
        log_failure("start the first tidy over", error);
    }
    cleared.is_ok()
}

/// Take from every ledger the rows of decisions the store no longer keeps.
/// Whether this is finished: rows the store could not judge, or a ledger that
/// would not be rewritten, leave it for the next start. A home with no index
/// has nothing to judge by and nothing that would change, so that is finished,
/// and its rows stay.
fn sweep_decision_rows(home: &Path, folders: &[PathBuf]) -> bool {
    let ledgers: Vec<PathBuf> = folders
        .iter()
        .map(|folder| folder.join(CUSTOM_DECISIONS.ledger))
        .filter(|ledger| ledger.exists())
        .collect();
    if ledgers.is_empty() || !store::root(home).join(store::INDEX).is_file() {
        return true;
    }
    let kept: BTreeSet<String> = match store::list(home) {
        Ok(entries) => entries.into_iter().map(|entry| entry.id).collect(),
        Err(error) => {
            log_failure("read the decision store", &error);
            return false;
        }
    };
    let mut finished = true;
    for ledger in ledgers {
        if let Err(error) = drop_custom_rows(&ledger, |row| !kept.contains(row)) {
            log_failure("rewrite a decision ledger", &error);
            finished = false;
        }
    }
    finished
}

/// Whether the person deleted seat `id`, read off the saved value itself.
/// [`builtin::in_root`] also reads a malformed value as deleted, so that it
/// does not bring a decision back by accident; that is no reason to erase what
/// a decision kept. A typed edit always says `deleted` for older clients, and
/// its real state is `questionDeleted`.
fn is_marked(root: &Value, id: &str) -> bool {
    if id == CUSTOM_DECISIONS.id {
        return false;
    }
    let Some(value) = root
        .get(SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get(builtin::SETTING))
        .and_then(Value::as_object)
        .and_then(|decisions| decisions.get(id))
    else {
        return false;
    };
    let Ok(decision) = serde_json::from_value::<builtin::Decision>(value.clone()) else {
        return false;
    };
    match value.get("questionSchema") {
        Some(schema) => {
            schema.as_u64() == Some(builtin::QUESTION_SCHEMA)
                && value.get("questionDeleted").and_then(Value::as_bool) == Some(true)
        }
        None => decision.deleted,
    }
}

fn record_swept(settings: &Path) -> io::Result<()> {
    let _lock = SettingsFileLock::acquire(settings)?;
    let mut root = read_settings(settings)?;
    let Some(smart) = smart_of(&mut root) else {
        return Ok(());
    };
    smart.insert(SWEPT_KEY.to_string(), Value::Bool(true));
    write_settings(settings, &root)
}

/// What [`PENDING_FILE`] holds.
#[derive(Default, Serialize, Deserialize)]
struct Pending {
    #[serde(default)]
    seats: BTreeSet<String>,
    #[serde(default)]
    decisions: BTreeSet<String>,
}

enum Owed<'id> {
    Seat(&'id str),
    Decision(&'id str),
}

/// A path that is not there, or whose parent is a file.
fn is_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn log_failure(what: &str, error: &io::Error) {
    eprintln!("jev: could not {what}: {error}");
}

/// Log the failure and note what it left. Whether the note holds it.
fn owe(home: &Path, owed: Owed<'_>, error: &io::Error) -> bool {
    log_failure("erase a deleted decision's records", error);
    let noted = change_pending(home, |pending| {
        match owed {
            Owed::Seat(id) => pending.seats.insert(id.to_string()),
            Owed::Decision(id) => pending.decisions.insert(id.to_string()),
        };
    });
    if let Err(error) = &noted {
        log_failure("note what a delete left", error);
    }
    noted.is_ok()
}

fn read_pending(path: &Path) -> io::Result<Pending> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Pending::default()),
        Err(error) => Err(error),
    }
}

/// Change the note under its lock; a note left with nothing to name goes.
fn change_pending(home: &Path, change: impl FnOnce(&mut Pending)) -> io::Result<()> {
    let path = home.join(PENDING_FILE);
    let _lock = SettingsFileLock::acquire(&path)?;
    let mut pending = read_pending(&path)?;
    change(&mut pending);
    if pending.seats.is_empty() && pending.decisions.is_empty() {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec(&pending).map_err(io::Error::other)?;
    let written = fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, &path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// Erase again what [`PENDING_FILE`] names, and strike what went. A seat no
/// longer deleted and a decision the store keeps again owe nothing; a decision
/// stays owed while the store cannot say. Whether the note is broken.
fn settle_owed(settings: &Path, home: &Path) -> bool {
    let path = home.join(PENDING_FILE);
    if !path.exists() {
        return false;
    }
    let pending = match read_pending(&path) {
        Ok(pending) => pending,
        Err(error) => {
            log_failure("read what a delete left", &error);
            return error.kind() == io::ErrorKind::InvalidData;
        }
    };
    let root = match read_settings(settings) {
        Ok(root) => root,
        Err(error) => {
            log_failure("read the settings a delete left", &error);
            return false;
        }
    };
    let settled_seats: BTreeSet<String> = pending
        .seats
        .into_iter()
        .filter(|id| !is_marked(&root, id) || forget_seat(settings, id).is_ok())
        .collect();
    let kept = store::list(home);
    let settled_decisions: BTreeSet<String> = pending
        .decisions
        .into_iter()
        .filter(|id| match &kept {
            Ok(entries) if entries.iter().any(|entry| entry.id == **id) => true,
            Ok(_) => forget_custom(home, id).is_ok(),
            Err(_) => false,
        })
        .collect();
    let struck = change_pending(home, |pending| {
        pending.seats.retain(|id| !settled_seats.contains(id));
        pending
            .decisions
            .retain(|id| !settled_decisions.contains(id));
    });
    if let Err(error) = struck {
        log_failure("strike what a start erased", &error);
    }
    false
}

/// Hold while appending to or rewriting a copy of the ledger every person's
/// decision shares: a rewrite replaces the file whole, and a row appended
/// between its read and its rename would go with the old file. `None` for any
/// other ledger, each of which only ever goes whole.
///
/// # Errors
/// A lock that cannot be opened, or that stays busy past
/// two seconds (`WouldBlock`).
pub fn shared_ledger_lock(ledger: &Path) -> io::Result<Option<HeldFileLock>> {
    let Some(name) = ledger
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| *name == CUSTOM_DECISIONS.ledger)
    else {
        return Ok(None);
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(ledger.with_file_name(format!("{name}.lock")))?;
    let until = Instant::now() + SHARED_LEDGER_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(HeldFileLock::locked(file))),
            Err(TryLockError::WouldBlock) if Instant::now() < until => {
                thread::sleep(SHARED_LEDGER_POLL);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another writer holds the decision ledger",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
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

/// Whether the lift is on record, read with no lock: every start and every
/// deletion of a kept seat asks, and only the first of them has a write to make.
fn already_lifted(settings: &Path) -> bool {
    read_settings(settings).is_ok_and(|root| {
        root.get(SMART_SETTINGS_KEY)
            .and_then(|smart| smart.get(LIFTED_KEY))
            .is_some()
    })
}

/// Lift the deletion marks of the seats the person kept, once per settings
/// file, and put each seat where its treatment says. Returns whether a mark
/// went; the first run on a file with no marks only records the lift.
///
/// # Errors
/// Whatever keeps the settings from being read or replaced.
pub fn lift_kept_seats(settings: &Path) -> io::Result<bool> {
    if !settings.exists() || already_lifted(settings) {
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
    if !settings.exists() || already_lifted(settings) {
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
/// [`AfterLift`] says. What counts is [`is_marked`]: an edited seat keeps its
/// edit, and a malformed value moves nothing.
fn lift_marks(root: &mut Value) -> bool {
    let marked: Vec<(&JevUse, AfterLift, Option<String>)> = KEPT_SEATS
        .iter()
        .filter(|(seat, _)| is_marked(root, seat.id))
        .filter_map(|(seat, after)| {
            let decision = builtin::in_root(root, seat.id)?;
            Some((*seat, *after, decision.mode))
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
        match (after, saved) {
            (AfterLift::AsBefore, Some(word)) => {
                smart.insert(seat.setting.to_string(), Value::String(word));
            }
            (AfterLift::AsBefore, None) => {
                smart.remove(seat.setting);
            }
            (AfterLift::StaysAsWritten, _) => {
                smart
                    .entry(seat.setting)
                    .or_insert_with(|| Value::String(JevMode::Off.key().to_string()));
            }
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

/// Rewrite `ledger` without the rows whose decision `goes` says should. A
/// missing ledger takes no lock, so no lock file is left where none stood.
fn drop_custom_rows(ledger: &Path, goes: impl Fn(&str) -> bool) -> io::Result<usize> {
    if !ledger.exists() {
        return Ok(0);
    }
    let _lock = shared_ledger_lock(ledger)?;
    let bytes = match fs::read(ledger) {
        Ok(bytes) => bytes,
        Err(error) if is_absent(&error) => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut kept = Vec::with_capacity(bytes.len());
    let mut removed = 0;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let names_it = serde_json::from_slice::<Value>(line)
            .ok()
            .is_some_and(|row| {
                row.get(CUSTOM_ROW_ID)
                    .and_then(Value::as_str)
                    .is_some_and(&goes)
            });
        if names_it {
            removed += 1;
        } else {
            kept.extend_from_slice(line);
        }
    }
    if removed == 0 {
        return Ok(0);
    }
    if kept.is_empty() {
        fs::remove_file(ledger)?;
    } else {
        replace_keeping_mode(ledger, &kept)?;
    }
    Ok(removed)
}

fn drop_threshold_line(table: &Path, id: &str) -> io::Result<()> {
    let bytes = match fs::read(table) {
        Ok(bytes) => bytes,
        Err(error) if is_absent(&error) => return Ok(()),
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
/// each project's, and in each of those the provider folders beside them. With
/// them, what kept a folder from being listed, so a caller knows it did not see
/// them all.
fn evidence_folders(home: &Path) -> (Vec<PathBuf>, Option<io::Error>) {
    let mut unread = None;
    let mut bases = vec![home.join(count::REQUESTS_DIR)];
    bases.extend(
        folders_in(&home.join(PROJECTS_DIR), &mut unread)
            .into_iter()
            .map(|project| project.join(PROJECT_STATE_DIR).join(PROJECT_LEDGER_DIR)),
    );
    let mut folders = Vec::new();
    for base in bases {
        folders.push(base.join(UNCONFIGURED_DIR));
        folders.extend(folders_in(&base.join(PROVIDERS_DIR), &mut unread));
        folders.push(base);
    }
    (folders, unread)
}

/// The folders directly in `parent`: a stray file there, such as Finder's
/// `.DS_Store`, is not one. A `parent` not there has none.
fn folders_in(parent: &Path, unread: &mut Option<io::Error>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if is_absent(&error) => return Vec::new(),
        Err(error) => {
            *unread = Some(error);
            return Vec::new();
        }
    };
    let mut folders = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) if entry.path().is_dir() => folders.push(entry.path()),
            Ok(_) => {}
            Err(error) => *unread = Some(error),
        }
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
