//! The home store of a person's decisions (t-62170): `jev/decisions/` under
//! zo's config home, and nowhere else. No folder's own `.zo/` is read, so a
//! repository cannot hand whoever opens it a decision they never made.
//!
//! - `index.json` lists every decision: its id, name and mode, the version of
//!   its definition, and when it was made and last changed.
//! - `definitions/<version>.json` is one version of a definition, never
//!   rewritten. An edit writes another, and the old one stays.
//! - `trash/<id>.json` is a deleted decision's row and when it was deleted,
//!   and `trash/<version>.json` its definition, until [`TRASH_KEPT_DAYS`] have
//!   passed ([`empty_trash`]).
//! - `.lock` is held by whatever writes.
//!
//! Every write is the project-rules store's (`super::super::project_rules`):
//! under the lock, into a temporary file renamed over the old one; and a
//! definition is read back only while its bytes still hash to its name.
//! New directories and files are owner-only on Unix.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{DECISIONS_MAX, Definition, MODES, Refusal, SCHEMA_VERSION, check, is_id, version_of};
use crate::civil;
use crate::jev::JevMode;

/// Where the store lives under zo's config home.
pub const DIRECTORY: &str = "jev/decisions";

/// Days a deleted decision waits in the trash before [`empty_trash`] removes it: the person's choice of 2026-10-17 (t-62170).
pub const TRASH_KEPT_DAYS: i64 = 30;

/// The largest definition the store keeps, in bytes: the longest words a definition holds, every character escaped, come to under 48 KiB, and the rest is room for its folders.
pub const DEFINITION_MAX_BYTES: usize = 64 * 1024;

/// The largest index, or trash row, the store reads back, in bytes: twenty rows of the longest name, every character escaped, come to under 9 KiB.
const INDEX_MAX_BYTES: usize = 16 * 1024;

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

const INDEX: &str = "index.json";
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

/// A deleted decision waiting in the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    /// Its row as the index held it.
    pub decision: Entry,
    /// When it was deleted, as [`civil::iso_utc_of`] writes a stamp.
    pub deleted_at: String,
}

/// Why the store kept nothing.
#[derive(Debug)]
pub enum Error {
    /// What it was handed breaks these rules: [`check`]'s, and those only
    /// the store can see — an id or a name taken, a mode no decision offers,
    /// a store already full, a definition past [`DEFINITION_MAX_BYTES`].
    Refused(Vec<Refusal>),
    /// No decision, or no deleted one, has this id.
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
/// [`Error::Refused`] with every rule it breaks — an id taken by a deleted
/// decision too, while it waits in the trash — and [`Error::Io`].
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
    if index.iter().any(|entry| entry.id == definition.id)
        || (is_id(&definition.id) && root.join(TRASH).join(json(&definition.id)).exists())
    {
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
/// decision's mode.
///
/// # Errors
/// [`Error::Missing`] when no decision has its id, [`Error::Refused`] with
/// every rule it breaks, and [`Error::Io`].
pub fn edit(home: &Path, definition: &Definition, now_ms: i64) -> Result<String, Error> {
    let root = root(home);
    let _lock = lock(&root)?;
    let mut index = read_index(&root)?;
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
    let version = write_definition(&root, &bytes)?;
    let entry = &mut index[at];
    entry.name.clone_from(&definition.name);
    entry.definition.clone_from(&version);
    entry.updated_at = civil::iso_utc_of(now_ms);
    write_index(&root, index)?;
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

/// Delete the decision `id` at `now_ms`: its row leaves the index, and the row
/// and its definition wait in the trash. Its ledger lines stay where they
/// are.
///
/// # Errors
/// [`Error::Missing`] and [`Error::Io`].
pub fn delete(home: &Path, id: &str, now_ms: i64) -> Result<(), Error> {
    if !is_id(id) {
        return Err(Error::Missing);
    }
    let root = root(home);
    let _lock = lock(&root)?;
    let mut index = read_index(&root)?;
    let Some(at) = index.iter().position(|entry| entry.id == id) else {
        return Err(Error::Missing);
    };
    let decision = index.remove(at);
    let trash = root.join(TRASH);
    private_directory(&trash)?;
    let from = root.join(DEFINITIONS).join(json(&decision.definition));
    let to = trash.join(json(&decision.definition));
    let row = TrashFile {
        schema_version: SCHEMA_VERSION,
        decision,
        deleted_at: civil::iso_utc_of(now_ms),
    };
    replace(
        &trash.join(json(id)),
        &serde_json::to_vec(&row).map_err(io::Error::other)?,
    )?;
    write_index(&root, index)?;
    match fs::rename(from, to) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// The deleted decisions still waiting in the trash, the earliest deleted
/// first. A row that does not read is left out: it cannot be restored.
///
/// # Errors
/// A trash that cannot be listed.
pub fn trash(home: &Path) -> io::Result<Vec<Trashed>> {
    let mut trashed: Vec<Trashed> = trashed_in(&root(home).join(TRASH))?
        .into_iter()
        .map(|(_, trashed)| trashed)
        .collect();
    trashed.sort_by(|one, other| {
        (&one.deleted_at, &one.decision.id).cmp(&(&other.deleted_at, &other.decision.id))
    });
    Ok(trashed)
}

/// Bring the deleted decision `id` back from the trash as it was: the same
/// row, so the same mode and the same version.
///
/// # Errors
/// [`Error::Missing`] when the trash holds no such decision,
/// [`Error::Refused`] when a decision made since took its name or filled the
/// store, and [`Error::Io`] — a definition that no longer hashes to its name
/// among them.
pub fn restore(home: &Path, id: &str) -> Result<(), Error> {
    if !is_id(id) {
        return Err(Error::Missing);
    }
    let root = root(home);
    let _lock = lock(&root)?;
    let mut index = read_index(&root)?;
    let trash = root.join(TRASH);
    let row = trash.join(json(id));
    let Trashed { decision, .. } = match read_trashed(&row) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(Error::Missing),
        read => read?,
    };
    let mut refused = Vec::new();
    if index.iter().any(|entry| entry.id == decision.id) {
        refused.push(Refusal::IdTaken);
    }
    if index
        .iter()
        .any(|entry| entry.name.trim() == decision.name.trim())
    {
        refused.push(Refusal::NameTaken);
    }
    if index.len() >= DECISIONS_MAX {
        refused.push(Refusal::TooMany);
    }
    if !refused.is_empty() {
        return Err(Error::Refused(refused));
    }
    let definitions = root.join(DEFINITIONS);
    let trashed = trash.join(json(&decision.definition));
    if trashed.exists() {
        private_directory(&definitions)?;
        fs::rename(trashed, definitions.join(json(&decision.definition)))?;
    }
    read_definition(&definitions, &decision.definition)?;
    let at = index.partition_point(|entry| entry.created_at <= decision.created_at);
    index.insert(at, decision);
    write_index(&root, index)?;
    fs::remove_file(row)?;
    Ok(())
}

/// Remove from the trash every decision deleted longer than
/// [`TRASH_KEPT_DAYS`] before `now_ms`, its definition with it; how many it
/// removed. A row whose deletion time does not read stays: what this cannot
/// date, it cannot call old.
///
/// # Errors
/// A trash that cannot be listed, or a file in it that cannot be removed.
pub fn empty_trash(home: &Path, now_ms: i64) -> io::Result<usize> {
    let root = root(home);
    let _lock = lock(&root)?;
    let trash = root.join(TRASH);
    let kept = TRASH_KEPT_DAYS * civil::MS_PER_DAY;
    let mut emptied = 0;
    for (row, trashed) in trashed_in(&trash)? {
        let Some(deleted) = civil::epoch_ms_of_iso(&trashed.deleted_at) else {
            continue;
        };
        if now_ms.saturating_sub(deleted) > kept {
            if let Err(error) = fs::remove_file(trash.join(json(&trashed.decision.definition)))
                && error.kind() != io::ErrorKind::NotFound
            {
                return Err(error);
            }
            fs::remove_file(row)?;
            emptied += 1;
        }
    }
    Ok(emptied)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IndexFile {
    schema_version: u32,
    decisions: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrashFile {
    schema_version: u32,
    decision: Entry,
    deleted_at: String,
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

fn read_trashed(path: &Path) -> io::Result<Trashed> {
    let row: TrashFile = serde_json::from_slice(&bounded(path, INDEX_MAX_BYTES)?)
        .map_err(|_| unreadable("not a trash row this store writes"))?;
    if row.schema_version != SCHEMA_VERSION || !well_named(&row.decision) {
        return Err(unreadable("not a trash row this store writes"));
    }
    Ok(Trashed {
        decision: row.decision,
        deleted_at: row.deleted_at,
    })
}

/// Every row in `trash` that reads, with its file: named by its own
/// decision's id.
fn trashed_in(trash: &Path) -> io::Result<Vec<(PathBuf, Trashed)>> {
    let entries = match fs::read_dir(trash) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        read => read?,
    };
    Ok(entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new(JSON)))
        .filter_map(|path| {
            let id = path.file_stem()?.to_str()?;
            if !is_id(id) {
                return None;
            }
            let trashed = read_trashed(&path).ok()?;
            (trashed.decision.id == id).then_some((path, trashed))
        })
        .collect())
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

fn lock(root: &Path) -> io::Result<File> {
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
            Ok(()) => return Ok(file),
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

    use super::super::Folders;
    use super::super::tests::{NOW, yes_no};
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
        delete(&home, &decision.id, NOW + 1).unwrap();
        restore(&home, &decision.id).unwrap();
        let store = root(&home);
        for directory in [
            home,
            store.parent().unwrap().to_path_buf(),
            store.join(DEFINITIONS),
            store.join(TRASH),
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

    /// Deleting keeps the decision whole in the trash, and restoring puts the
    /// same row back: its mode, and the version its definition hashes to.
    #[test]
    fn delete_moves_to_trash_and_restore_brings_it_back() {
        let home = tempfile::tempdir().unwrap();
        let decision = yes_no("d-0000000a", "Release notes");
        let version = create(home.path(), &decision, JevMode::On, NOW).unwrap();
        let before = list(home.path()).unwrap();

        delete(home.path(), &decision.id, NOW + 1).unwrap();
        assert!(
            list(home.path()).unwrap().is_empty(),
            "a deleted decision leaves the list"
        );
        let store = root(home.path());
        assert!(!store.join(DEFINITIONS).join(json(&version)).exists());
        assert!(
            store.join(TRASH).join(json(&version)).is_file(),
            "its definition waits in the trash",
        );
        assert_eq!(
            trash(home.path()).unwrap(),
            [Trashed {
                decision: before[0].clone(),
                deleted_at: civil::iso_utc_of(NOW + 1),
            }],
        );
        assert!(
            matches!(
                create(home.path(), &decision, JevMode::On, NOW + 2),
                Err(Error::Refused(refused)) if refused == [Refusal::IdTaken]
            ),
            "its id stays taken while it waits",
        );

        restore(home.path(), &decision.id).unwrap();
        assert_eq!(
            list(home.path()).unwrap(),
            before,
            "it comes back as it was: its mode, and the same version",
        );
        assert_eq!(definition(home.path(), &version).unwrap(), decision);
        assert!(trash(home.path()).unwrap().is_empty());
        assert!(matches!(
            restore(home.path(), &decision.id),
            Err(Error::Missing)
        ));
        assert!(matches!(
            delete(home.path(), "d-ffffffff", NOW),
            Err(Error::Missing)
        ));
    }

    /// The trash keeps a deleted decision thirty days, and no longer.
    #[test]
    fn trash_older_than_30_days_is_emptied() {
        let home = tempfile::tempdir().unwrap();
        let old = yes_no("d-0000000a", "Old");
        let recent = yes_no("d-0000000b", "Recent");
        let old_version = create(home.path(), &old, JevMode::Shadow, NOW).unwrap();
        create(home.path(), &recent, JevMode::Shadow, NOW).unwrap();
        delete(home.path(), &old.id, NOW).unwrap();
        delete(home.path(), &recent.id, NOW + civil::MS_PER_DAY).unwrap();
        let kept = TRASH_KEPT_DAYS * civil::MS_PER_DAY;

        assert_eq!(
            empty_trash(home.path(), NOW + kept).unwrap(),
            0,
            "thirty days to the millisecond is not older than thirty days",
        );
        assert_eq!(empty_trash(home.path(), NOW + kept + 1).unwrap(), 1);
        let left: Vec<String> = trash(home.path())
            .unwrap()
            .into_iter()
            .map(|trashed| trashed.decision.id)
            .collect();
        assert_eq!(left, std::slice::from_ref(&recent.id));
        assert!(
            !root(home.path())
                .join(TRASH)
                .join(json(&old_version))
                .exists(),
            "its definition went with it",
        );
        assert!(matches!(restore(home.path(), &old.id), Err(Error::Missing)));

        let undated = root(home.path()).join(TRASH).join(json(&recent.id));
        let mut row: serde_json::Value =
            serde_json::from_slice(&fs::read(&undated).unwrap()).unwrap();
        row["deletedAt"] = "yesterday".into();
        fs::write(undated, serde_json::to_vec(&row).unwrap()).unwrap();
        assert_eq!(
            empty_trash(home.path(), i64::MAX).unwrap(),
            0,
            "a row it cannot date stays"
        );
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
}
