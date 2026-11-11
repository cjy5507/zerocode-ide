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
    replace(path, bytes, None, None::<fn(&Path) -> io::Result<()>>)
}

/// [`write_atomically`] for a file whose permissions matter — a settings file
/// that may hold a key, a credential.
///
/// On unix the temporary is born with `mode` (the umask may only narrow it),
/// so no byte is ever readable more widely than the finished file. The
/// written temporary is synced, then `seal` gets its path before the rename:
/// the place to give it its exact mode, or a platform's ACL where a mode at
/// creation has no road.
///
/// # Errors
/// Creating, writing, syncing, sealing or renaming failed; the temporary is
/// removed whichever step it was, so nothing is left holding the bytes.
pub fn write_atomically_sealed(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    seal: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    replace(path, bytes, mode, Some(seal))
}

fn replace(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    seal: Option<impl FnOnce(&Path) -> io::Result<()>>,
) -> io::Result<()> {
    // An existing temporary path may be a link planted to redirect the write.
    // Each writer owns a fresh sibling; create_new refuses even a dangling
    // link and keeps simultaneous writers from sharing a buffer.
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(&temp)?;
    let mut result = file.write_all(bytes);
    if seal.is_some() {
        result = result.and_then(|()| file.sync_all());
    }
    drop(file);
    if let Some(seal) = seal {
        result = result.and_then(|()| seal(&temp));
    }
    let result = result.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    /// The bytes are never readable more widely than the finished file: the
    /// temporary already has the asked-for mode when it is first looked at.
    /// 0400 is a mode no umask makes by itself, so only the birth mode can
    /// give it.
    #[cfg(unix)]
    #[test]
    fn a_sealed_temporary_is_born_with_its_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let mut born = None;
        write_atomically_sealed(&path, b"example = 1\n", Some(0o400), |temp| {
            born = Some(fs::metadata(temp)?.permissions().mode() & 0o777);
            Ok(())
        })
        .expect("write");
        assert_eq!(
            born,
            Some(0o400),
            "the temporary was born with another mode"
        );
        assert_eq!(fs::read_to_string(&path).expect("read"), "example = 1\n");
    }

    /// Whatever step fails, no temporary is left holding the bytes and the
    /// old file stays whole.
    #[test]
    fn a_failed_seal_leaves_no_temporary_and_the_old_file_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        fs::write(&path, "{}\n").expect("old");
        let refused = write_atomically_sealed(&path, b"{\"env\":1}\n", Some(0o600), |_| {
            Err(io::Error::other("refused"))
        });
        assert!(refused.is_err());
        assert_eq!(leftovers(dir.path()), Vec::<String>::new());
        assert_eq!(fs::read_to_string(&path).expect("read"), "{}\n");
    }

    #[test]
    fn a_failed_rename_leaves_no_temporary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        // A folder with something in it where the file goes: the rename fails.
        fs::create_dir_all(path.join("occupied")).expect("mkdir");
        assert!(write_atomically(&path, b"x").is_err());
        assert!(write_atomically_sealed(&path, b"x", Some(0o600), |_| Ok(())).is_err());
        assert_eq!(leftovers(dir.path()), Vec::<String>::new());
    }
}
