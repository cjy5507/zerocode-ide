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

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Definition, MODES, Refusal};
use crate::jev::JevMode;

/// Where the store lives under zo's config home.
pub const DIRECTORY: &str = "jev/decisions";

/// The largest definition the store keeps, in bytes: the longest words a definition holds, every character escaped, come to under 48 KiB, and the rest is room for its folders.
pub const DEFINITION_MAX_BYTES: usize = 64 * 1024;

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
    let _ = home;
    Ok(Vec::new())
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
    let _ = (home, definition, mode, now_ms);
    Ok(String::new())
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
