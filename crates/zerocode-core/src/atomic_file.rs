//! Replacing a whole file so that no reader ever sees half of it.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

/// Temp beside, then rename: a reader never sees half a file, and a crash
/// mid-write leaves the old file whole.
///
/// # Errors
/// The temporary sibling could not be created or written, or the rename over
/// `path` failed; the temporary is removed either way.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // An existing temporary path may be a link planted to redirect the write.
    // Each writer owns a fresh sibling; create_new refuses even a dangling
    // link and keeps simultaneous writers from sharing a buffer.
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let written = file.write_all(bytes);
    drop(file);
    let result = written.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
