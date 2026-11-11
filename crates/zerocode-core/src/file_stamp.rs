//! What a reader compares to tell whether a file moved, without reading it.

use std::fs::Metadata;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// A file's size, times and identity: what a look compares, never reads.
///
/// It can be written down beside what was read of the file (the session
/// vault's index keeps one per transcript, [`crate::vault_index`]), so the
/// next look — in another run of the window — reads only what moved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileStamp {
    pub len: u64,
    pub modified_ms: i64,
    /// The inode's last change: a rename moves it, so a file moved into a
    /// folder says when it was moved there.
    pub changed_ms: i64,
    /// Device and inode: a file replaced by another one is read again.
    pub file: (u64, u64),
}

#[must_use]
pub fn stamp(path: &Path) -> Option<FileStamp> {
    std::fs::metadata(path).ok().map(|meta| stamp_of(&meta))
}

/// The stamp of a plain file at `path` — none for a link, a pipe or a folder:
/// a file another process owns is not followed to wherever it points.
#[must_use]
pub fn stamp_plain(path: &Path) -> Option<FileStamp> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(Metadata::is_file)
        .map(|meta| stamp_of(&meta))
}

#[must_use]
pub fn stamp_of(meta: &Metadata) -> FileStamp {
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as i64);
    #[cfg(unix)]
    let (changed_ms, file) = {
        use std::os::unix::fs::MetadataExt;
        (
            meta.ctime() * 1000 + meta.ctime_nsec() / 1_000_000,
            (meta.dev(), meta.ino()),
        )
    };
    #[cfg(not(unix))]
    let (changed_ms, file) = (modified_ms, (0, 0));
    FileStamp {
        len: meta.len(),
        modified_ms,
        changed_ms,
        file,
    }
}
