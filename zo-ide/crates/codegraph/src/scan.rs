//! The freshness check: which supported files the workspace holds, and the
//! modification time and length each was last written with.
//!
//! Every query pays for this walk before it reads the index, so it runs on the
//! walker's thread pool instead of one thread, and it reads metadata only —
//! parsing is for the files whose fingerprint moved.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use ignore::{WalkBuilder, WalkState};

use crate::index::{MAX_INDEXABLE_FILE_SIZE, MAX_INDEXED_FILES};
use crate::language::spec_for_path;
use crate::model::FileFingerprint;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScanEntry {
    pub(crate) fingerprint: FileFingerprint,
    pub(crate) too_large: bool,
}

pub(crate) struct WorkspaceScan {
    pub(crate) files: BTreeMap<PathBuf, ScanEntry>,
    pub(crate) file_limit_reached: bool,
}

/// Walk `root` honouring its ignore files and keep the first
/// [`MAX_INDEXED_FILES`] supported files in path order — the same set a
/// sorted walk that stopped at the cap would keep, so the set cannot change
/// between two walks of an unchanged tree.
pub(crate) fn scan_workspace(root: &Path) -> WorkspaceScan {
    let found = Mutex::new(Vec::new());
    let mut builder = WalkBuilder::new(root);
    builder.require_git(false);
    builder.build_parallel().run(|| {
        let found = &found;
        Box::new(move |result| {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!("[codegraph] workspace walk skipped an entry: {error}");
                    return WalkState::Continue;
                }
            };
            if !entry.file_type().is_some_and(|file_type| file_type.is_file())
                || spec_for_path(entry.path()).is_none()
            {
                return WalkState::Continue;
            }
            let Ok(relative) = entry.path().strip_prefix(root) else {
                return WalkState::Continue;
            };
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    eprintln!(
                        "[codegraph] metadata read failed for {}: {error}",
                        entry.path().display()
                    );
                    return WalkState::Continue;
                }
            };
            let fingerprint = fingerprint(&metadata);
            let scanned = ScanEntry {
                fingerprint,
                too_large: fingerprint.size > MAX_INDEXABLE_FILE_SIZE,
            };
            found
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((relative.to_path_buf(), scanned));
            WalkState::Continue
        })
    });
    let mut found = found.into_inner().unwrap_or_else(PoisonError::into_inner);
    found.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let file_limit_reached = found.len() > MAX_INDEXED_FILES;
    if file_limit_reached {
        eprintln!(
            "[codegraph] reached MAX_INDEXED_FILES ({MAX_INDEXED_FILES}); remaining supported files are not indexed"
        );
        found.truncate(MAX_INDEXED_FILES);
    }
    WorkspaceScan {
        files: found.into_iter().collect(),
        file_limit_reached,
    }
}

/// The modification time is clamped to what the index stores (nanoseconds
/// in an `i64`, good until 2262), so a stored fingerprint and a fresh one of
/// an untouched file are always equal.
pub(crate) fn fingerprint(metadata: &fs::Metadata) -> FileFingerprint {
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos())
        .min(i64::MAX.unsigned_abs().into());
    FileFingerprint {
        modified_nanos,
        size: metadata.len(),
    }
}
