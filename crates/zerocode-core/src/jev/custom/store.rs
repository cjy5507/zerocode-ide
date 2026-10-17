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

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{DECISIONS_MAX, Definition, MODES, Refusal, SCHEMA_VERSION, check, is_id, version_of};
use crate::civil;
use crate::jev::JevMode;

/// Where the store lives under zo's config home.
pub const DIRECTORY: &str = "jev/decisions";

/// The largest definition the store keeps, in bytes: the longest words a definition holds, every character escaped, come to under 48 KiB, and the rest is room for its folders.
pub const DEFINITION_MAX_BYTES: usize = 64 * 1024;

/// The largest index, or trash row, the store reads back, in bytes: twenty rows of the longest name, every character escaped, come to under 9 KiB.
const INDEX_MAX_BYTES: usize = 16 * 1024;

/// Hex digits in a version's name: a SHA-256, written out.
const VERSION_HEX_DIGITS: usize = 64;

/// Who may read and write the store's files: the person alone, whose words they are.
#[cfg(unix)]
const PRIVATE_MODE: u32 = 0o600;

const INDEX: &str = "index.json";
const DEFINITIONS: &str = "definitions";
const TRASH: &str = "trash";
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
    /// No decision, or no deleted one, has this id.
    Missing,
    /// A file could not be read or written, a definition no longer hashes to
    /// its name, or another writer held the store past [`LOCK_WAIT`]
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

/// Write a definition's bytes under the name they hash to, unless that file
/// holds them already; the name.
fn write_definition(root: &Path, bytes: &[u8]) -> io::Result<String> {
    let version = version_of(bytes);
    let dir = root.join(DEFINITIONS);
    fs::create_dir_all(&dir)?;
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
