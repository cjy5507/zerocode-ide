//! The home store of a person's decisions (t-62170): `jev/decisions/` under
//! zo's config home, and nowhere else. No folder's own `.zo/` is read, so a
//! repository cannot hand whoever opens it a decision they never made.
//!
//! - `index.json` lists every decision: its id, name and mode, the version of
//!   its definition, and when it was made and last changed.
//! - `definitions/<version>.json` is one version of a definition, never
//!   rewritten. An edit writes another, and the old one stays.
//! - A deleted decision is gone: its row leaves the index and every version of
//!   its definition is removed ([`delete`]). An older window kept a `trash/`
//!   folder; [`forget_deleted`] removes it.
//! - `.lock` is held by whatever writes.
//!
//! Every write is the project-rules store's (`super::super::project_rules`):
//! under the lock, into a temporary file renamed over the old one; and a
//! definition is read back only while its bytes still hash to its name.
//! New directories and files are owner-only on Unix.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ask_when::{AskWhen, Unfit};
use super::{DECISIONS_MAX, Definition, MODES, Refusal, SCHEMA_VERSION, check, is_id, version_of};
use crate::civil;
use crate::held_file_lock::HeldFileLock;
use crate::jev::JevMode;

/// Where the store lives under zo's config home.
pub const DIRECTORY: &str = "jev/decisions";

/// The largest definition the store keeps, in bytes: the longest words a definition holds, every character escaped, come to under 48 KiB, and the rest is room for its folders.
pub const DEFINITION_MAX_BYTES: usize = 64 * 1024;

/// The largest index the store reads back, in bytes: twenty rows of the longest name, every character escaped, come to under 9 KiB.
const INDEX_MAX_BYTES: usize = 16 * 1024;

/// The most bytes the ask-when file is read back up to: forty words of sixty
/// characters for every one of twenty decisions, with room to spare.
const ASK_WHEN_MAX_BYTES: usize = 256 * 1024;

/// Hex digits in a version's name: a SHA-256, written out.
const VERSION_HEX_DIGITS: usize = 64;

/// How long a writer waits for another to let go of the store before it reports it busy: the project-rules store's wait.
const LOCK_WAIT: Duration = Duration::from_millis(25);

/// How often a waiting writer looks again.
const LOCK_POLL: Duration = Duration::from_millis(1);

/// Who may read and write the store's files: the person alone, whose words they are.
#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o600;

/// Who may enter the store's new directories: the person alone.
#[cfg(unix)]
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;

pub(crate) const INDEX: &str = "index.json";
/// The file beside the index that says when each decision asks
/// ([`super::ask_when`]): a file no build before it opens, so a decision told
/// when to ask is still one every build reads.
pub const ASK_WHEN: &str = "ask-when.json";
const DEFINITIONS: &str = "definitions";
const TRASH: &str = "trash";
const LOCK: &str = ".lock";
const JSON: &str = "json";

/// One decision as the index lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// One of [`MODES`]; a word that is not reads as `off`.
    #[serde(serialize_with = "write_mode", deserialize_with = "read_mode")]
    pub mode: JevMode,
    /// The version of its definition: the SHA-256 of the definition's file.
    pub definition: String,
    /// When it was made and last changed, as [`civil::iso_utc_of`] writes a
    /// stamp.
    pub created_at: String,
    pub updated_at: String,
}

/// Why the store kept nothing.
#[derive(Debug)]
pub enum Error {
    /// What it was handed breaks these rules: [`check`]'s, and those only
    /// the store can see — an id or a name taken, a mode no decision offers,
    /// a store already full, a definition past [`DEFINITION_MAX_BYTES`].
    Refused(Vec<Refusal>),
    /// No decision has this id.
    Missing,
    /// A file could not be read or written, a definition no longer hashes to
    /// its name, or another writer held the store past `LOCK_WAIT`
    /// (`WouldBlock`).
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The store's folder under `home`, zo's config home.
#[must_use]
pub fn root(home: &Path) -> PathBuf {
    home.join(DIRECTORY)
}

/// The store's index under `home`: the one file every write of the store
/// rewrites, so a reader that watches it sees every decision made, edited,
/// switched or deleted.
#[must_use]
pub fn index_path(home: &Path) -> PathBuf {
    root(home).join(INDEX)
}

/// The file beside the index that says when each decision asks
/// ([`ask_when`]): [`set_ask_when`] writes it alone, never the index, so a
/// reader that holds what it read must watch this file too.
#[must_use]
pub fn ask_when_path(home: &Path) -> PathBuf {
    root(home).join(ASK_WHEN)
}

/// Every decision the store keeps, in the order they were made; none before
/// one is.
///
/// # Errors
/// An index that cannot be read, or is not one this store writes.
pub fn list(home: &Path) -> io::Result<Vec<Entry>> {
    read_index(&root(home))
}

/// The definition `version` names, read only while its bytes still hash to
/// that name.
///
/// # Errors
/// No such version, a file past [`DEFINITION_MAX_BYTES`], bytes that no
/// longer hash to `version`, or bytes that are not a definition.
pub fn definition(home: &Path, version: &str) -> io::Result<Definition> {
    read_definition(&root(home).join(DEFINITIONS), version)
}

/// The decisions that ask in `workspace`, each with its definition: those not
/// `off` whose folders hold it ([`super::asks_in`]). Read from `home` alone —
/// nothing under `workspace` is opened. A definition that no longer reads is
/// left out: a damaged file asks nothing.
///
/// # Errors
/// [`list`]'s.
pub fn asking_in(home: &Path, workspace: &Path) -> io::Result<Vec<(Entry, Definition)>> {
    Ok(list(home)?
        .into_iter()
        .filter(|entry| entry.mode.asks())
        .filter_map(|entry| {
            let definition = definition(home, &entry.definition).ok()?;
            (definition.id == entry.id && super::asks_in(&definition, workspace))
                .then_some((entry, definition))
        })
        .collect())
}

/// Keep `definition` as a new decision in `mode`, made at `now_ms`; its
/// version.
///
/// # Errors
/// [`Error::Refused`] with every rule it breaks, and [`Error::Io`].
pub fn create(
    home: &Path,
    definition: &Definition,
    mode: JevMode,
    now_ms: i64,
) -> Result<String, Error> {
    let root = root(home);
    let _lock = lock(&root)?;
    let mut index = read_index(&root)?;
    let mut refused = check(definition);
    if !MODES.contains(&mode) {
        refused.push(Refusal::Mode);
    }
    if index.iter().any(|entry| entry.id == definition.id) {
        refused.push(Refusal::IdTaken);
    }
    if name_taken(&index, definition) {
        refused.push(Refusal::NameTaken);
    }
    if index.len() >= DECISIONS_MAX {
        refused.push(Refusal::TooMany);
    }
    let bytes = definition.to_bytes().map_err(io::Error::other)?;
    if bytes.len() > DEFINITION_MAX_BYTES {
        refused.push(Refusal::TooLarge);
    }
    if !refused.is_empty() {
        return Err(Error::Refused(refused));
    }
    let version = write_definition(&root, &bytes)?;
    let stamp = civil::iso_utc_of(now_ms);
    index.push(Entry {
        id: definition.id.clone(),
        name: definition.name.clone(),
        mode,
        definition: version.clone(),
        created_at: stamp.clone(),
        updated_at: stamp,
    });
    write_index(&root, index)?;
    Ok(version)
}

/// Keep `definition` as the newest version of the decision with its id,
/// changed at `now_ms`; that version. The old version stays, and so does the
/// decision's mode. A decision moved to another point loses what it was told
/// about when to ask ([`set_ask_when`]): words chosen for one point's text are
/// never looked for in another's (t-107869), so it asks every time until it
/// is told again.
///
/// # Errors
/// [`Error::Missing`] when no decision has its id, [`Error::Refused`] with
/// every rule it breaks, and [`Error::Io`].
pub fn edit(home: &Path, definition: &Definition, now_ms: i64) -> Result<String, Error> {
    let root = root(home);
    let _lock = lock(&root)?;
    let index = read_index(&root)?;
    edit_held(&root, index, definition, now_ms)
}

/// Set how many requests the decision `id` may send in an hour to `most`,
/// at `now_ms`, on the version it asks with now — read under the store's
/// lock, so an edit saved meanwhile is never written over by an older
/// question (t-107869). The bound it had, and the version that holds `most`:
/// the same one when it already did.
///
/// # Errors
/// As [`edit`].
pub fn set_max_per_hour(
    home: &Path,
    id: &str,
    most: u64,
    now_ms: i64,
) -> Result<(u64, String), Error> {
    let root = root(home);
    let _lock = lock(&root)?;
    let index = read_index(&root)?;
    let Some(entry) = index.iter().find(|entry| entry.id == id) else {
        return Err(Error::Missing);
    };
    let mut definition = read_definition(&root.join(DEFINITIONS), &entry.definition)?;
    let before = definition.max_per_hour;
    if before == most {
        return Ok((before, entry.definition.clone()));
    }
    definition.max_per_hour = most;
    edit_held(&root, index, &definition, now_ms).map(|version| (before, version))
}

/// [`edit`] under the lock its caller holds, on the `index` it read there.
fn edit_held(
    root: &Path,
    mut index: Vec<Entry>,
    definition: &Definition,
    now_ms: i64,
) -> Result<String, Error> {
    let Some(at) = index.iter().position(|entry| entry.id == definition.id) else {
        return Err(Error::Missing);
    };
    let mut refused = check(definition);
    if name_taken(&index, definition) {
        refused.push(Refusal::NameTaken);
    }
    let bytes = definition.to_bytes().map_err(io::Error::other)?;
    if bytes.len() > DEFINITION_MAX_BYTES {
        refused.push(Refusal::TooLarge);
    }
    if !refused.is_empty() {
        return Err(Error::Refused(refused));
    }
    let moved = read_definition(&root.join(DEFINITIONS), &index[at].definition)
        .ok()
        .is_none_or(|before| before.at != definition.at);
    let version = write_definition(root, &bytes)?;
    let entry = &mut index[at];
    entry.name.clone_from(&definition.name);
    entry.definition.clone_from(&version);
    entry.updated_at = civil::iso_utc_of(now_ms);
    if moved {
        // Best effort, as a delete's: a line left behind is read as its new
        // point keeps it ([`AskWhen::kept_at`]).
        let _ = keep_ask_when(root, &index, |decisions| {
            decisions.remove(&definition.id);
        });
    }
    write_index(root, index)?;
    Ok(version)
}

/// Set the decision `id` to `mode` at `now_ms`. Only the index changes: the
/// definition, and so its version, stays where it was.
///
/// # Errors
/// [`Error::Missing`], [`Error::Refused`] with [`Refusal::Mode`] for a mode
/// no decision offers, and [`Error::Io`].
pub fn set_mode(home: &Path, id: &str, mode: JevMode, now_ms: i64) -> Result<(), Error> {
    if !MODES.contains(&mode) {
        return Err(Error::Refused(vec![Refusal::Mode]));
    }
    let root = root(home);
    let _lock = lock(&root)?;
    let mut index = read_index(&root)?;
    let Some(entry) = index.iter_mut().find(|entry| entry.id == id) else {
        return Err(Error::Missing);
    };
    entry.mode = mode;
    entry.updated_at = civil::iso_utc_of(now_ms);
    write_index(&root, index)?;
    Ok(())
}

/// Delete the decision `id` for good: its row leaves the index, and every
/// version of its definition no other row uses is removed with it. Its ledger
/// lines stay where they are.
///
/// Once the row has left the index the delete has happened: a definition file
/// that then will not go is not an error the person is shown. It names a
/// decision no row lists, and [`forget_deleted`] removes it on a later pass.
///
/// # Errors
/// [`Error::Missing`], and [`Error::Io`] for a store whose index cannot be
/// read or written.
pub fn delete(home: &Path, id: &str) -> Result<(), Error> {
    if !is_id(id) {
        return Err(Error::Missing);
    }
    let root = root(home);
    let _lock = lock(&root)?;
    let mut index = read_index(&root)?;
    let Some(at) = index.iter().position(|entry| entry.id == id) else {
        return Err(Error::Missing);
    };
    index.remove(at);
    write_index(&root, index.clone())?;
    // What it was told about when to ask goes with it. The row has left the
    // index already, so a file that will not change is no error: a line no
    // row lists is read past and dropped at the next write.
    let _ = keep_ask_when(&root, &index, |_| {});
    // Its left-out counts too: nothing counts them again to clear them.
    crate::jev::count::forget_left_out(home, id);
    // An edit writes another definition and keeps the old one, so a decision
    // may leave several: read what is there and remove those that are this
    // decision's and that no remaining row names.
    let definitions = root.join(DEFINITIONS);
    let used: BTreeSet<&str> = index
        .iter()
        .map(|entry| entry.definition.as_str())
        .collect();
    for path in definition_files(&definitions) {
        let Some(version) = version_named(&path) else {
            continue;
        };
        if !used.contains(version)
            && read_definition(&definitions, version).is_ok_and(|one| one.id == id)
        {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

/// When each decision under `home` asks, by its id — a decision the file
/// says nothing of asks at every moment of its point. A file that is not
/// there says nothing of any; a line whose id the index no longer lists (a
/// decision an older window deleted) is left out.
///
/// # Errors
/// A file that cannot be read or is not one this store writes, and an index
/// that cannot be read.
pub fn ask_when(home: &Path) -> io::Result<BTreeMap<String, AskWhen>> {
    let root = root(home);
    let mut said = read_ask_when(&root)?;
    if said.is_empty() {
        return Ok(said);
    }
    let listed: BTreeSet<String> = read_index(&root)?
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    said.retain(|id, _| listed.contains(id));
    Ok(said)
}

/// Why the store did not keep when a decision asks.
#[derive(Debug)]
pub enum AskWhenError {
    /// No decision has this id.
    Missing,
    /// The decision's point does not take it ([`AskWhen::fit_for`]).
    Unfit(Unfit),
    /// The store or the file could not be read or written.
    Io(io::Error),
}

impl From<io::Error> for AskWhenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Say when the decision `id` asks: `said` as its point keeps it
/// ([`AskWhen::fit_for`]), nothing said taking its line out. Answers what it
/// said before and what it says now. Under the store's lock; lines whose id
/// the index no longer lists are dropped on the way.
///
/// # Errors
/// [`AskWhenError::Missing`], [`AskWhenError::Unfit`], and
/// [`AskWhenError::Io`] for a store that cannot be read or written.
pub fn set_ask_when(
    home: &Path,
    id: &str,
    said: &AskWhen,
) -> Result<(AskWhen, AskWhen), AskWhenError> {
    if !is_id(id) {
        return Err(AskWhenError::Missing);
    }
    let root = root(home);
    let _lock = lock(&root)?;
    let index = read_index(&root)?;
    let entry = index
        .iter()
        .find(|entry| entry.id == id)
        .ok_or(AskWhenError::Missing)?;
    let definition = read_definition(&root.join(DEFINITIONS), &entry.definition)?;
    let kept = said.fit_for(definition.at).map_err(AskWhenError::Unfit)?;
    let mut before = AskWhen::default();
    keep_ask_when(&root, &index, |decisions| {
        before = decisions.remove(id).unwrap_or_default();
        if !kept.always() {
            decisions.insert(id.to_string(), kept.clone());
        }
    })?;
    Ok((before, kept))
}

/// Tidy the store of what a delete left: the `trash/` folder an older window
/// kept for deleted decisions, and every definition file whose decision the
/// index no longer lists (an older window moved only a deleted decision's
/// current version out, and a delete here may have failed to remove a file).
/// Whether anything was removed. A listed decision keeps every version it
/// has, and a file that does not read as a definition is left alone, as is
/// everything when the index itself cannot be read.
///
/// Under the store's lock; a home with no store gets none made.
///
/// # Errors
/// A `trash/` folder that cannot be removed, or an index that cannot be read.
/// The other half is done either way.
pub fn forget_deleted(home: &Path) -> io::Result<bool> {
    let root = root(home);
    if !root.is_dir() {
        return Ok(false);
    }
    let _lock = lock(&root)?;
    let trash = match fs::remove_dir_all(root.join(TRASH)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    };
    let definitions = sweep_definitions(&root);
    Ok(trash? | definitions?)
}

fn sweep_definitions(root: &Path) -> io::Result<bool> {
    // No index is no list to judge by: every file would read as a stranger's.
    if !root.join(INDEX).is_file() {
        return Ok(false);
    }
    let listed: BTreeSet<String> = read_index(root)?
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    let definitions = root.join(DEFINITIONS);
    let mut removed = false;
    for path in definition_files(&definitions) {
        let Some(version) = version_named(&path) else {
            continue;
        };
        if read_definition(&definitions, version).is_ok_and(|one| !listed.contains(&one.id))
            && fs::remove_file(&path).is_ok()
        {
            removed = true;
        }
    }
    Ok(removed)
}

/// The files in `definitions/`; none when the folder is not there or not
/// readable.
fn definition_files(definitions: &Path) -> Vec<PathBuf> {
    fs::read_dir(definitions)
        .map(|files| files.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

/// The version a definition file's name carries, when it is named like one.
fn version_named(path: &Path) -> Option<&str> {
    path.file_stem()
        .and_then(OsStr::to_str)
        .filter(|version| path.extension() == Some(OsStr::new(JSON)) && is_version(version))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IndexFile {
    schema_version: u32,
    decisions: Vec<Entry>,
}

fn write_mode<S: Serializer>(mode: &JevMode, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(mode.key())
}

fn read_mode<'de, D: Deserializer<'de>>(deserializer: D) -> Result<JevMode, D::Error> {
    let word = String::deserialize(deserializer)?;
    Ok(MODES
        .into_iter()
        .find(|mode| mode.key() == word)
        .unwrap_or_default())
}

fn read_index(root: &Path) -> io::Result<Vec<Entry>> {
    let bytes = match bounded(&root.join(INDEX), INDEX_MAX_BYTES) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        read => read?,
    };
    let index: IndexFile =
        serde_json::from_slice(&bytes).map_err(|_| unreadable("not an index this store writes"))?;
    let ids: BTreeSet<&str> = index
        .decisions
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    if index.schema_version != SCHEMA_VERSION
        || ids.len() != index.decisions.len()
        || !index.decisions.iter().all(well_named)
    {
        return Err(unreadable("not an index this store writes"));
    }
    Ok(index.decisions)
}

/// The ask-when file as written: the schema, and each decision's line by id.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskWhenFile {
    schema_version: u32,
    #[serde(default)]
    decisions: BTreeMap<String, AskWhen>,
}

fn read_ask_when(root: &Path) -> io::Result<BTreeMap<String, AskWhen>> {
    let bytes = match bounded(&root.join(ASK_WHEN), ASK_WHEN_MAX_BYTES) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        read => read?,
    };
    // Every reader takes a file that does not read as saying nothing — every
    // decision asks every time — and so does the store once it is removed:
    // the error says so, for a person who cannot write past it.
    let broken = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not an ask-when file this store writes; removing it makes every decision ask every time",
                root.join(ASK_WHEN).display()
            ),
        )
    };
    let file: AskWhenFile = serde_json::from_slice(&bytes).map_err(|_| broken())?;
    if file.schema_version != SCHEMA_VERSION || !file.decisions.keys().all(|id| is_id(id)) {
        return Err(broken());
    }
    Ok(file.decisions)
}

/// Change the ask-when file by `change`, under the lock the caller holds,
/// keeping only lines that say something for a decision `index` lists. A
/// file left saying nothing is removed rather than written empty.
fn keep_ask_when(
    root: &Path,
    index: &[Entry],
    change: impl FnOnce(&mut BTreeMap<String, AskWhen>),
) -> io::Result<()> {
    let path = root.join(ASK_WHEN);
    let held = read_ask_when(root)?;
    let mut decisions = held.clone();
    change(&mut decisions);
    decisions.retain(|id, said| !said.always() && index.iter().any(|entry| entry.id == *id));
    if decisions == held {
        return Ok(());
    }
    if decisions.is_empty() {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let file = AskWhenFile {
        schema_version: SCHEMA_VERSION,
        decisions,
    };
    replace(&path, &serde_json::to_vec(&file).map_err(io::Error::other)?)
}

fn write_index(root: &Path, decisions: Vec<Entry>) -> io::Result<()> {
    let index = IndexFile {
        schema_version: SCHEMA_VERSION,
        decisions,
    };
    replace(
        &root.join(INDEX),
        &serde_json::to_vec(&index).map_err(io::Error::other)?,
    )
}

fn read_definition(dir: &Path, version: &str) -> io::Result<Definition> {
    if !is_version(version) {
        return Err(unreadable("not a version's name"));
    }
    let bytes = bounded(&dir.join(json(version)), DEFINITION_MAX_BYTES)?;
    if version_of(&bytes) != version {
        return Err(unreadable("a definition that no longer hashes to its name"));
    }
    Definition::from_bytes(&bytes).map_err(|_| unreadable("not a definition this store writes"))
}

/// Write a definition's bytes under the name they hash to, unless that file
/// holds them already; the name.
fn write_definition(root: &Path, bytes: &[u8]) -> io::Result<String> {
    let version = version_of(bytes);
    let dir = root.join(DEFINITIONS);
    private_directory(&dir)?;
    let path = dir.join(json(&version));
    if !path.exists() {
        replace(&path, bytes)?;
    } else if bounded(&path, DEFINITION_MAX_BYTES)? != bytes {
        return Err(unreadable("a definition that no longer hashes to its name"));
    }
    Ok(version)
}

fn well_named(entry: &Entry) -> bool {
    is_id(&entry.id) && is_version(&entry.definition)
}

fn name_taken(index: &[Entry], definition: &Definition) -> bool {
    index
        .iter()
        .any(|entry| entry.id != definition.id && entry.name.trim() == definition.name.trim())
}

fn is_version(word: &str) -> bool {
    word.len() == VERSION_HEX_DIGITS
        && word
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn json(stem: &str) -> String {
    format!("{stem}.{JSON}")
}

fn unreadable(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn bounded(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?
        .take(u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut data)?;
    if data.len() > cap {
        return Err(unreadable("a file past what the store reads back"));
    }
    Ok(data)
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(PRIVATE_DIRECTORY_MODE);
    }
    builder.create(path)
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_MODE);
    }
    options.open(path)
}

fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut out = private_file(&temp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn lock(root: &Path) -> io::Result<HeldFileLock> {
    private_directory(root)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_MODE);
    }
    let file = options.open(root.join(LOCK))?;
    let until = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(HeldFileLock::locked(file)),
            Err(TryLockError::WouldBlock) if Instant::now() < until => thread::sleep(LOCK_POLL),
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another writer holds the decisions store",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::thread;
    use std::time::Duration;

    use super::super::tests::{NOW, yes_no};
    use super::super::{Folders, Point};
    use super::*;

    fn ids(decisions: &[(Entry, Definition)]) -> Vec<&str> {
        decisions
            .iter()
            .map(|(entry, _)| entry.id.as_str())
            .collect()
    }

    fn folder(path: &Path) -> Folders {
        Folders::Chosen(BTreeSet::from([path.to_string_lossy().into_owned()]))
    }

    #[cfg(unix)]
    #[test]
    fn store_directories_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("config");
        let decision = yes_no("d-0000000a", "Release notes");
        create(&home, &decision, JevMode::On, NOW).unwrap();
        let store = root(&home);
        for directory in [
            home,
            store.parent().unwrap().to_path_buf(),
            store.join(DEFINITIONS),
            store,
        ] {
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700,
                "only the owner may access {}",
                directory.display(),
            );
        }
    }

    /// A repository that ships a store of its own under its `.zo/` defines
    /// nothing: decisions are the person's, read from their home alone.
    #[test]
    fn only_the_home_store_defines_decisions() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mine = yes_no("d-0000000a", "Release notes");
        create(home.path(), &mine, JevMode::Shadow, NOW).unwrap();
        let planted = workspace.path().join(".zo");
        create(&planted, &yes_no("d-0000000b", "Planted"), JevMode::On, NOW).unwrap();
        assert!(
            root(&planted).join(INDEX).is_file(),
            "the planted store is one this store would read",
        );

        let asking = asking_in(home.path(), workspace.path()).unwrap();
        assert_eq!(
            ids(&asking),
            [mine.id.as_str()],
            "only the home store's decision asks"
        );
        assert_eq!(asking[0].1, mine);

        let mut elsewhere = yes_no("d-0000000c", "Elsewhere");
        elsewhere.folders = folder(&home.path().join("elsewhere"));
        create(home.path(), &elsewhere, JevMode::On, NOW).unwrap();
        create(
            home.path(),
            &yes_no("d-0000000d", "Silent"),
            JevMode::Off,
            NOW,
        )
        .unwrap();
        let mut here = yes_no("d-0000000e", "Here");
        here.folders = folder(workspace.path());
        create(home.path(), &here, JevMode::On, NOW).unwrap();
        assert_eq!(
            ids(&asking_in(home.path(), &workspace.path().join("crates")).unwrap()),
            ["d-0000000a", "d-0000000e"],
            "an off decision asks nothing, and a chosen folder holds the folders under it",
        );
        assert!(
            asking_in(&planted, workspace.path()).is_ok(),
            "the planted store reads — it is only never asked",
        );
    }

    /// Deleting is final: the row leaves the list, every version of the
    /// definition is removed — the edited ones too — and nothing is kept
    /// anywhere, so the id is free again. Another decision's files stay.
    #[test]
    fn delete_removes_the_row_and_every_version_and_keeps_nothing() {
        let home = tempfile::tempdir().unwrap();
        let decision = yes_no("d-0000000a", "Release notes");
        let other = yes_no("d-0000000b", "Other");
        let first = create(home.path(), &decision, JevMode::On, NOW).unwrap();
        let other_version = create(home.path(), &other, JevMode::On, NOW).unwrap();
        let mut edited = decision.clone();
        edited.question = "Is this the newer question?".to_string();
        let second = edit(home.path(), &edited, NOW + 1).unwrap();
        assert_ne!(first, second);
        let store = root(home.path());
        assert!(store.join(DEFINITIONS).join(json(&first)).is_file());

        delete(home.path(), &decision.id).unwrap();
        let left: Vec<String> = list(home.path())
            .unwrap()
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(
            left,
            std::slice::from_ref(&other.id),
            "a deleted decision leaves the list"
        );
        for version in [&first, &second] {
            assert!(
                !store.join(DEFINITIONS).join(json(version)).exists(),
                "version {version} went with it"
            );
        }
        assert!(
            store.join(DEFINITIONS).join(json(&other_version)).is_file(),
            "another decision's definition stays"
        );
        assert!(!store.join(TRASH).exists(), "no trash is made");
        let everything: Vec<_> = walk(&store);
        assert!(
            everything.iter().all(|path| !path.contains(&decision.id)),
            "nothing under the store names it: {everything:?}"
        );
        assert!(matches!(
            delete(home.path(), &decision.id),
            Err(Error::Missing)
        ));
        assert!(matches!(
            delete(home.path(), "d-ffffffff"),
            Err(Error::Missing)
        ));
        create(home.path(), &decision, JevMode::On, NOW + 2)
            .expect("its id and its name are free again");
    }

    /// A definition another row still names is not removed with a delete.
    #[test]
    fn delete_keeps_a_definition_another_row_names() {
        let home = tempfile::tempdir().unwrap();
        let one = yes_no("d-0000000a", "One");
        let version = create(home.path(), &one, JevMode::On, NOW).unwrap();
        let mut index = read_index(&root(home.path())).unwrap();
        let mut twin = index[0].clone();
        twin.id = "d-0000000b".to_string();
        twin.name = "Twin".to_string();
        index.push(twin);
        write_index(&root(home.path()), index).unwrap();
        delete(home.path(), &one.id).unwrap();
        assert!(
            root(home.path())
                .join(DEFINITIONS)
                .join(json(&version))
                .is_file()
        );
    }

    /// The `trash/` folder an older window kept is removed, only when it is
    /// there, and the store keeps working either way. A home with no store
    /// gets no folder made.
    #[test]
    fn the_old_trash_folder_is_forgotten() {
        let home = tempfile::tempdir().unwrap();
        assert!(!forget_deleted(home.path()).unwrap(), "nothing to remove");
        assert!(!root(home.path()).exists(), "no store is made to be swept");
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
        let decision = yes_no("d-0000000a", "Release notes");
        create(home.path(), &decision, JevMode::On, NOW).unwrap();
        assert!(!forget_deleted(home.path()).unwrap());
        let trash = root(home.path()).join(TRASH);
        fs::create_dir_all(&trash).unwrap();
        fs::write(trash.join("d-0000000z.json"), b"{}").unwrap();
        assert!(forget_deleted(home.path()).unwrap());
        assert!(!trash.exists());
        assert!(!forget_deleted(home.path()).unwrap());
        assert_eq!(list(home.path()).unwrap().len(), 1, "the list is untouched");
    }

    /// An older window moved only a deleted decision's current version to
    /// `trash/` and left the versions before it in `definitions/`. The sweep
    /// removes those whose decision is not listed — with no `trash/` there —
    /// and leaves a listed decision's older versions, and a file it cannot
    /// read, where they are.
    #[test]
    fn the_sweep_removes_the_old_versions_of_a_decision_that_is_gone() {
        let home = tempfile::tempdir().unwrap();
        let gone = yes_no("d-0000000a", "Gone");
        let kept = yes_no("d-0000000b", "Kept");
        let gone_first = create(home.path(), &gone, JevMode::On, NOW).unwrap();
        let mut gone_edit = gone.clone();
        gone_edit.question = "Is this the newer question?".to_string();
        let gone_second = edit(home.path(), &gone_edit, NOW + 1).unwrap();
        let kept_first = create(home.path(), &kept, JevMode::On, NOW).unwrap();
        let mut kept_edit = kept.clone();
        kept_edit.question = "Is this the newer question?".to_string();
        let kept_second = edit(home.path(), &kept_edit, NOW + 1).unwrap();
        let store = root(home.path());
        let definitions = store.join(DEFINITIONS);
        // What the older window's delete left: the row gone, the current
        // version moved out, the first version standing.
        let rows = read_index(&store)
            .unwrap()
            .into_iter()
            .filter(|entry| entry.id != gone.id)
            .collect();
        write_index(&store, rows).unwrap();
        fs::remove_file(definitions.join(json(&gone_second))).unwrap();
        let unreadable = definitions.join(json(&"f".repeat(VERSION_HEX_DIGITS)));
        fs::write(&unreadable, b"not a definition").unwrap();
        assert!(!store.join(TRASH).exists());

        assert!(forget_deleted(home.path()).unwrap());
        assert!(
            !definitions.join(json(&gone_first)).exists(),
            "the deleted decision's old version went"
        );
        for version in [&kept_first, &kept_second] {
            assert!(
                definitions.join(json(version)).is_file(),
                "a listed decision keeps every version: {version}"
            );
        }
        assert!(unreadable.is_file(), "a file it cannot read stays");
        assert!(!forget_deleted(home.path()).unwrap(), "nothing left to do");
    }

    /// An index the store cannot read says nothing about which decisions are
    /// gone, so the sweep removes no definition.
    #[test]
    fn the_sweep_removes_nothing_when_the_index_cannot_be_read() {
        let home = tempfile::tempdir().unwrap();
        let version = create(
            home.path(),
            &yes_no("d-0000000a", "Release notes"),
            JevMode::On,
            NOW,
        )
        .unwrap();
        let store = root(home.path());
        fs::write(store.join(INDEX), b"{ torn").unwrap();
        assert!(forget_deleted(home.path()).is_err());
        assert!(store.join(DEFINITIONS).join(json(&version)).is_file());
        fs::remove_file(store.join(INDEX)).unwrap();
        let _ = forget_deleted(home.path());
        assert!(
            store.join(DEFINITIONS).join(json(&version)).is_file(),
            "a store with no index is not read as an empty list"
        );
    }

    /// The row has left the list when a definition file will not go. The
    /// delete is done all the same — the person sees it succeed — and the
    /// next sweep removes what was left.
    #[cfg(unix)]
    #[test]
    fn a_definition_that_will_not_go_does_not_fail_the_delete() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let decision = yes_no("d-0000000a", "Release notes");
        let version = create(home.path(), &decision, JevMode::On, NOW).unwrap();
        let definitions = root(home.path()).join(DEFINITIONS);
        fs::set_permissions(&definitions, fs::Permissions::from_mode(0o500)).unwrap();
        let stuck = fs::remove_file(definitions.join(json(&version))).is_err();
        let deleted = delete(home.path(), &decision.id);
        let listed = list(home.path()).unwrap();
        let left = definitions.join(json(&version)).exists();
        fs::set_permissions(&definitions, fs::Permissions::from_mode(0o700)).unwrap();
        if !stuck {
            return; // the folder lock does not bind this user (root)
        }
        assert!(deleted.is_ok(), "{deleted:?}");
        assert!(listed.is_empty(), "the row left the list");
        assert!(left, "the file is what was left");
        assert!(forget_deleted(home.path()).unwrap());
        assert!(!definitions.join(json(&version)).exists());
    }

    fn walk(path: &Path) -> Vec<String> {
        let mut found = vec![path.display().to_string()];
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                found.extend(walk(&entry.path()));
            }
        }
        found
    }

    /// Every write holds the store's lock, so two writers at once each land
    /// whole: neither tears the index, and neither loses the other's change.
    #[test]
    fn two_writers_do_not_tear_the_index() {
        let home = tempfile::tempdir().unwrap();
        let held = lock(&root(home.path())).unwrap();
        let waited = create(
            home.path(),
            &yes_no("d-0000000a", "First"),
            JevMode::Shadow,
            NOW,
        );
        assert!(
            matches!(&waited, Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock),
            "a writer does not write while another holds the store: {waited:?}",
        );
        assert!(list(home.path()).unwrap().is_empty());
        drop(held);

        let writers: Vec<_> = (0..2)
            .map(|writer| {
                let home = home.path().to_path_buf();
                thread::spawn(move || {
                    for n in 0..5 {
                        let decision =
                            yes_no(&format!("d-{writer}000000{n}"), &format!("{writer}.{n}"));
                        loop {
                            match create(&home, &decision, JevMode::Shadow, NOW) {
                                Ok(_) => break,
                                Err(Error::Io(error))
                                    if error.kind() == io::ErrorKind::WouldBlock =>
                                {
                                    thread::sleep(Duration::from_millis(1));
                                }
                                Err(error) => panic!("{error:?}"),
                            }
                        }
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let listed = list(home.path()).unwrap();
        assert_eq!(listed.len(), 10, "no writer's change was lost: {listed:?}");
        for entry in &listed {
            assert_eq!(
                definition(home.path(), &entry.definition).unwrap().id,
                entry.id
            );
        }
    }

    /// A process launch on another thread that forks before it execs holds a
    /// copy of the store's lock until the exec; the duplicate here is that
    /// copy, without a launch winning a race. The next writer must not wait
    /// on it.
    #[test]
    fn a_copy_of_the_lock_left_in_a_launch_does_not_hold_the_next_writer() {
        let home = tempfile::tempdir().unwrap();
        let held = lock(&root(home.path())).unwrap();
        let inherited = held.try_clone().unwrap();
        drop(held);
        let next = lock(&root(home.path()));
        drop(inherited);
        assert!(
            next.is_ok(),
            "a lock its holder let go is still held by a copy: {next:?}"
        );
    }

    /// [`yes_no`] asking at `point`, reading what that point lends.
    fn asking_at(id: &str, name: &str, point: Point) -> Definition {
        let mut decision = yes_no(id, name);
        decision.at = point;
        decision.reads = point.inputs().iter().copied().collect();
        decision
    }

    fn asks_when(words: &[&str], edited: bool) -> AskWhen {
        AskWhen {
            words: words.iter().map(|word| (*word).to_string()).collect(),
            edited,
        }
    }

    /// When a decision asks is kept beside the index, read back by id, and
    /// taken out when nothing is said again.
    #[test]
    fn when_a_decision_asks_is_kept_beside_the_index_and_read_back() {
        let home = tempfile::tempdir().unwrap();
        let before_command = asking_at("d-0000000a", "Build line", Point::BeforeCommand);
        create(home.path(), &before_command, JevMode::On, NOW).unwrap();
        create(
            home.path(),
            &yes_no("d-0000000b", "Checked"),
            JevMode::On,
            NOW,
        )
        .unwrap();
        assert!(
            ask_when(home.path()).unwrap().is_empty(),
            "nothing said is no file"
        );
        assert!(!root(home.path()).join(ASK_WHEN).exists());

        let said = asks_when(&[" cargo ", "just", "just"], false);
        let (before, now) = set_ask_when(home.path(), "d-0000000a", &said).unwrap();
        assert_eq!(before, AskWhen::default());
        assert_eq!(now, asks_when(&["cargo", "just"], false));
        let (_, edited) = set_ask_when(home.path(), "d-0000000b", &asks_when(&[], true)).unwrap();
        assert!(edited.edited);
        let read = ask_when(home.path()).unwrap();
        assert_eq!(read.get("d-0000000a"), Some(&now));
        assert_eq!(read.get("d-0000000b"), Some(&edited));

        let (before, now) = set_ask_when(home.path(), "d-0000000a", &AskWhen::default()).unwrap();
        assert_eq!(before, asks_when(&["cargo", "just"], false));
        assert!(now.always());
        assert_eq!(
            ask_when(home.path()).unwrap().keys().collect::<Vec<_>>(),
            ["d-0000000b"]
        );
    }

    /// A point that cannot take what was said refuses it, and so does a
    /// decision nobody keeps.
    #[test]
    fn a_point_that_cannot_take_it_and_a_decision_nobody_keeps_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let asked = asking_at("d-0000000a", "Asked", Point::AgentAsks);
        create(home.path(), &asked, JevMode::On, NOW).unwrap();
        create(
            home.path(),
            &yes_no("d-0000000b", "Turn end"),
            JevMode::On,
            NOW,
        )
        .unwrap();
        let before_command = asking_at("d-0000000c", "Command", Point::BeforeCommand);
        create(home.path(), &before_command, JevMode::On, NOW).unwrap();
        assert!(matches!(
            set_ask_when(home.path(), "d-0000000a", &asks_when(&["rm"], false)),
            Err(AskWhenError::Unfit(Unfit::AskedByAnAgent))
        ));
        assert!(matches!(
            set_ask_when(home.path(), "d-0000000c", &asks_when(&[], true)),
            Err(AskWhenError::Unfit(Unfit::EditedOutsideATurnsEnd))
        ));
        assert!(matches!(
            set_ask_when(home.path(), "d-0000000f", &asks_when(&["rm"], false)),
            Err(AskWhenError::Missing)
        ));
        assert!(ask_when(home.path()).unwrap().is_empty());
    }

    /// A deleted decision's line goes with it; a line an older window left
    /// for a decision it deleted is read past and dropped at the next write.
    #[test]
    fn a_deleted_decisions_line_goes_with_it() {
        let home = tempfile::tempdir().unwrap();
        for id in ["d-0000000a", "d-0000000b"] {
            let one = asking_at(id, id, Point::BeforeCommand);
            create(home.path(), &one, JevMode::On, NOW).unwrap();
            set_ask_when(home.path(), id, &asks_when(&["rm"], false)).unwrap();
            crate::jev::count::count_left_out(home.path(), id, "2026-11-14").unwrap();
        }
        delete(home.path(), "d-0000000a").unwrap();
        let left_out = |id: &str| crate::jev::count::left_out_path(home.path(), id, "2026-11-14");
        assert!(
            !left_out("d-0000000a").exists() && left_out("d-0000000b").exists(),
            "its left-out days go with it, and no other decision's"
        );
        assert_eq!(
            ask_when(home.path()).unwrap().keys().collect::<Vec<_>>(),
            ["d-0000000b"]
        );
        let file = fs::read_to_string(root(home.path()).join(ASK_WHEN)).unwrap();
        assert!(!file.contains("d-0000000a"), "{file}");

        // An older window deletes without knowing the file: its line stays
        // until a write, and no reader takes it for a living decision.
        let store = root(home.path());
        let mut index = read_index(&store).unwrap();
        index.retain(|entry| entry.id != "d-0000000b");
        write_index(&store, index).unwrap();
        assert!(ask_when(home.path()).unwrap().is_empty());
        let again = asking_at("d-0000000c", "Again", Point::BeforeCommand);
        create(home.path(), &again, JevMode::On, NOW).unwrap();
        set_ask_when(home.path(), "d-0000000c", &asks_when(&["rm"], false)).unwrap();
        let file = fs::read_to_string(store.join(ASK_WHEN)).unwrap();
        assert!(!file.contains("d-0000000b"), "{file}");
    }

    /// The file is one no build before it opens: with it beside the index,
    /// every decision and every definition reads exactly as without it, and
    /// tidying the store leaves it where it is.
    #[test]
    fn the_index_and_every_definition_read_the_same_with_the_file_beside_them() {
        let home = tempfile::tempdir().unwrap();
        let before_command = asking_at("d-0000000a", "Build line", Point::BeforeCommand);
        create(home.path(), &before_command, JevMode::On, NOW).unwrap();
        create(
            home.path(),
            &yes_no("d-0000000b", "Checked"),
            JevMode::Shadow,
            NOW,
        )
        .unwrap();
        let listed = list(home.path()).unwrap();
        let definitions: Vec<Definition> = listed
            .iter()
            .map(|entry| definition(home.path(), &entry.definition).unwrap())
            .collect();
        set_ask_when(home.path(), "d-0000000a", &asks_when(&["cargo"], false)).unwrap();
        set_ask_when(home.path(), "d-0000000b", &asks_when(&[], true)).unwrap();

        assert_eq!(
            list(home.path()).unwrap(),
            listed,
            "the index reads the same"
        );
        for (entry, held) in listed.iter().zip(&definitions) {
            assert_eq!(&definition(home.path(), &entry.definition).unwrap(), held);
        }
        assert!(
            !forget_deleted(home.path()).unwrap(),
            "tidying finds nothing to take"
        );
        assert!(
            root(home.path()).join(ASK_WHEN).is_file(),
            "and leaves the file be"
        );
        assert_eq!(ask_when(home.path()).unwrap().len(), 2);
    }
    /// A bound is set on the decision's newest version under the store's
    /// lock (t-107869): the question an edit saved before stays, the bound
    /// moves, and the same bound again writes no version.
    #[test]
    fn a_bound_is_set_on_the_newest_version_and_keeps_its_edits() {
        let home = tempfile::tempdir().unwrap();
        let mut decision = asking_at("d-0000000a", "Bound", Point::BeforeCommand);
        create(home.path(), &decision, JevMode::On, NOW).unwrap();
        decision.question = "Does this command delete anything?".to_string();
        edit(home.path(), &decision, NOW + 1).unwrap();
        let (before, version) = set_max_per_hour(home.path(), "d-0000000a", 60, NOW + 2).unwrap();
        assert_eq!(before, decision.max_per_hour);
        let now = definition(home.path(), &version).unwrap();
        assert_eq!(
            (now.question.as_str(), now.max_per_hour),
            (decision.question.as_str(), 60)
        );
        assert_eq!(list(home.path()).unwrap()[0].definition, version);
        assert_eq!(
            set_max_per_hour(home.path(), "d-0000000a", 60, NOW + 3).unwrap(),
            (60, version)
        );
        assert!(matches!(
            set_max_per_hour(home.path(), "d-0000000f", 60, NOW),
            Err(Error::Missing)
        ));
        assert!(matches!(
            set_max_per_hour(home.path(), "d-0000000a", 0, NOW),
            Err(Error::Refused(_))
        ));
    }

    /// A decision moved to another point loses its words (t-107869): words
    /// chosen for a command are never looked for in a turn's last answer. An
    /// edit that keeps the point keeps them.
    #[test]
    fn an_edit_that_moves_the_point_takes_the_words_and_one_that_does_not_keeps_them() {
        let home = tempfile::tempdir().unwrap();
        let mut decision = asking_at("d-0000000a", "Deletes", Point::BeforeCommand);
        create(home.path(), &decision, JevMode::On, NOW).unwrap();
        let (_, said) =
            set_ask_when(home.path(), "d-0000000a", &asks_when(&["rm"], false)).unwrap();
        decision.question = "Does this command remove anything?".to_string();
        edit(home.path(), &decision, NOW + 1).unwrap();
        assert_eq!(
            ask_when(home.path()).unwrap().get("d-0000000a"),
            Some(&said)
        );
        let moved = asking_at("d-0000000a", "Deletes", Point::TurnEnd);
        edit(home.path(), &moved, NOW + 2).unwrap();
        assert!(ask_when(home.path()).unwrap().is_empty());
        assert!(!ask_when_path(home.path()).exists());
    }

    /// A file beside the index that does not read says how to get back to
    /// asking every time (t-107869): remove it, which is what every reader
    /// already takes it to mean.
    #[test]
    fn a_broken_file_says_that_removing_it_asks_every_time() {
        let home = tempfile::tempdir().unwrap();
        let decision = asking_at("d-0000000a", "Deletes", Point::BeforeCommand);
        create(home.path(), &decision, JevMode::On, NOW).unwrap();
        let path = ask_when_path(home.path());
        fs::write(&path, "{ not json").unwrap();
        let said = ask_when(home.path()).unwrap_err().to_string();
        assert!(
            said.contains(&path.display().to_string())
                && said.contains("removing it makes every decision ask every time"),
            "{said}"
        );
        let Err(AskWhenError::Io(error)) =
            set_ask_when(home.path(), "d-0000000a", &asks_when(&["rm"], false))
        else {
            panic!("a broken file was written past");
        };
        assert_eq!(error.to_string(), said);
    }
}
