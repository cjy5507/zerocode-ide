//! A day's OSV answer about one workspace's lockfiles, kept on disk.
//!
//! The file lives under the cache root, not the config root: every byte of it
//! can be asked for again, and a platform that clears caches loses nothing but
//! one lookup.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zerocode_core::supply_chain::{OsvFindings, OsvRecord};

/// The directory under the cache root: one file per workspace.
pub(crate) const DIR_NAME: &str = "supply-chain";

/// How long an answer about unchanged lockfiles stands: one day, the
/// freshness the design grants a "no known vulnerability" (§2 G8). OSV keeps
/// importing advisories, so this is also how late the picture may learn of a
/// new one; the card shows `checkedAt` and a refresh asks at once.
pub(crate) const FRESH_FOR_MS: i64 = 24 * 60 * 60 * 1_000;

/// The file's shape. A file of another shape is asked again, never guessed at.
const SCHEMA: u32 = 1;

/// What one lookup learned, and about which lockfiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CachedLookup {
    schema: u32,
    /// `zerocode_core::supply_chain::lockfile_fingerprint` of the lockfiles
    /// the answer is about.
    pub(crate) fingerprint: String,
    /// When OSV gave the answer, epoch ms.
    pub(crate) checked_at: i64,
    pub(crate) findings: OsvFindings,
    pub(crate) records: Vec<OsvRecord>,
}

impl CachedLookup {
    pub(crate) fn new(
        fingerprint: String,
        checked_at: i64,
        findings: OsvFindings,
        records: Vec<OsvRecord>,
    ) -> Self {
        Self {
            schema: SCHEMA,
            fingerprint,
            checked_at,
            findings,
            records,
        }
    }

    /// Less than [`FRESH_FOR_MS`] old at `now_ms`. An answer stamped in the
    /// future — a clock set back — is not fresh.
    pub(crate) fn fresh_at(&self, now_ms: i64) -> bool {
        (0..FRESH_FOR_MS).contains(&now_ms.saturating_sub(self.checked_at))
    }
}

/// The workspace's file: named by the SHA-256 of its canonical root, so no
/// path spells a file name and two workspaces never share one.
pub(crate) fn file(cache_root: &Path, root: &Path) -> PathBuf {
    let name = crate::artifact_runtime::sha256_hex(root.to_string_lossy().as_bytes());
    cache_root.join(DIR_NAME).join(format!("{name}.json"))
}

/// The answer on file, when there is one of this shape.
pub(crate) fn read(path: &Path) -> Option<CachedLookup> {
    let bytes = crate::durable_file::read_plain_file(path).ok()?;
    serde_json::from_slice::<CachedLookup>(&bytes)
        .ok()
        .filter(|held| held.schema == SCHEMA)
}

/// Replace the file with `lookup`, atomically.
pub(crate) fn write(path: &Path, lookup: &CachedLookup) -> Result<(), String> {
    let bytes = serde_json::to_vec(lookup).map_err(|error| error.to_string())?;
    crate::durable_file::replace_bytes(path, &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
