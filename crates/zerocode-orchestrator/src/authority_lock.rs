//! Private cross-process liveness lock for one orchestration ledger.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use sha2::{Digest, Sha256};

static PROCESS_LOCKS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerLockError {
    Busy,
    Unavailable,
    #[cfg(not(any(unix, windows)))]
    Unsupported,
}

pub(crate) struct OwnerLock {
    file: File,
    path: PathBuf,
}

impl std::fmt::Debug for OwnerLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OwnerLock(<private-authority-lock>)")
    }
}

impl OwnerLock {
    pub(crate) fn try_acquire(store_path: &Path, ledger_id: &str) -> Result<Self, OwnerLockError> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (store_path, ledger_id);
            return Err(OwnerLockError::Unsupported);
        }
        #[cfg(any(unix, windows))]
        {
            let path = lock_path(store_path, ledger_id)?;
            {
                let mut held = PROCESS_LOCKS
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !held.insert(path.clone()) {
                    return Err(OwnerLockError::Busy);
                }
            }
            let acquired = open_private_lock(&path).and_then(|file| {
                try_lock(&file)?;
                Ok(Self {
                    file,
                    path: path.clone(),
                })
            });
            if acquired.is_err() {
                PROCESS_LOCKS
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&path);
            }
            acquired
        }
    }

    pub(crate) fn release(mut self) -> Result<(), OwnerLockError> {
        unlock(&self.file)?;
        PROCESS_LOCKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.path);
        self.path.clear();
        Ok(())
    }
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        let _ = unlock(&self.file);
        if !self.path.as_os_str().is_empty() {
            PROCESS_LOCKS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&self.path);
        }
    }
}

fn lock_path(store_path: &Path, ledger_id: &str) -> Result<PathBuf, OwnerLockError> {
    let parent = store_path.parent().ok_or(OwnerLockError::Unavailable)?;
    let mut digest = Sha256::new();
    digest.update(b"zerocode.runtime-owner-lock.v1");
    let store = stable_path_bytes(store_path.as_os_str());
    digest.update((store.len() as u64).to_le_bytes());
    digest.update(&store);
    digest.update((ledger_id.len() as u64).to_le_bytes());
    digest.update(ledger_id.as_bytes());
    Ok(parent.join(format!(".runtime-owner-{:x}.lock", digest.finalize())))
}

/// Stable platform spelling for a canonical path. Unlike
/// `OsStr::as_encoded_bytes`, this is an explicit rolling-upgrade contract.
#[cfg(unix)]
pub(crate) fn stable_path_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(windows)]
pub(crate) fn stable_path_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    utf16le(value.encode_wide())
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn stable_path_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().as_bytes().to_vec()
}

#[cfg(any(windows, test))]
fn utf16le(units: impl IntoIterator<Item = u16>) -> Vec<u8> {
    units.into_iter().flat_map(u16::to_le_bytes).collect()
}

fn open_private_lock(path: &Path) -> Result<File, OwnerLockError> {
    if std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.file_type().is_symlink() || !meta.is_file())
    {
        return Err(OwnerLockError::Unavailable);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .map_err(|_| OwnerLockError::Unavailable)?;
    let metadata = file.metadata().map_err(|_| OwnerLockError::Unavailable)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OwnerLockError::Unavailable);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(OwnerLockError::Unavailable);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| OwnerLockError::Unavailable)?;
        }
    }
    Ok(file)
}

#[cfg(unix)]
fn try_lock(file: &File) -> Result<(), OwnerLockError> {
    use std::os::fd::AsRawFd;
    // SAFETY: file owns the descriptor for the lifetime of OwnerLock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    let code = error.raw_os_error();
    if code == Some(libc::EWOULDBLOCK) || code == Some(libc::EAGAIN) {
        Err(OwnerLockError::Busy)
    } else {
        Err(OwnerLockError::Unavailable)
    }
}

#[cfg(unix)]
fn unlock(file: &File) -> Result<(), OwnerLockError> {
    use std::os::fd::AsRawFd;
    // SAFETY: file owns the descriptor and the lock was acquired through it.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(OwnerLockError::Unavailable)
    }
}

#[cfg(windows)]
fn try_lock(file: &File) -> Result<(), OwnerLockError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::LockFile;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    // SAFETY: file owns a non-inheritable handle for OwnerLock's lifetime.
    if unsafe { LockFile(file.as_raw_handle() as _, 0, 0, 1, 0) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
        Err(OwnerLockError::Busy)
    } else {
        Err(OwnerLockError::Unavailable)
    }
}

#[cfg(windows)]
fn unlock(file: &File) -> Result<(), OwnerLockError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFile;
    // SAFETY: file owns the handle and the lock covers this exact byte.
    if unsafe { UnlockFile(file.as_raw_handle() as _, 0, 0, 1, 0) } != 0 {
        Ok(())
    } else {
        Err(OwnerLockError::Unavailable)
    }
}

#[cfg(not(any(unix, windows)))]
fn try_lock(_file: &File) -> Result<(), OwnerLockError> {
    Err(OwnerLockError::Unsupported)
}

#[cfg(not(any(unix, windows)))]
fn unlock(_file: &File) -> Result<(), OwnerLockError> {
    Err(OwnerLockError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_process_has_one_lock_and_drop_allows_takeover() {
        let root = tempfile::tempdir().expect("lock root");
        let store = root.path().join("authority.sqlite");
        std::fs::write(&store, b"").expect("store sentinel");
        let first = OwnerLock::try_acquire(&store, "main-ledger").expect("first lock");
        assert!(matches!(
            OwnerLock::try_acquire(&store, "main-ledger"),
            Err(OwnerLockError::Busy)
        ));
        let busy = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "authority_lock::tests::child_lock_probe"])
            .env("ZEROCODE_LOCK_PROBE_STORE", &store)
            .env("ZEROCODE_LOCK_PROBE_BUSY", "1")
            .status()
            .expect("busy child probe");
        assert!(
            busy.success(),
            "another process acquired the live owner lock"
        );
        drop(first);
        let acquired = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "authority_lock::tests::child_lock_probe"])
            .env("ZEROCODE_LOCK_PROBE_STORE", &store)
            .env("ZEROCODE_LOCK_PROBE_BUSY", "0")
            .status()
            .expect("takeover child probe");
        assert!(
            acquired.success(),
            "crash-style close did not release OS lock"
        );
        let second = OwnerLock::try_acquire(&store, "main-ledger").expect("takeover lock");
        second.release().expect("explicit unlock");
    }

    #[test]
    fn child_lock_probe() {
        let Some(store) = std::env::var_os("ZEROCODE_LOCK_PROBE_STORE") else {
            return;
        };
        let expected_busy = std::env::var("ZEROCODE_LOCK_PROBE_BUSY").as_deref() == Ok("1");
        let answer = OwnerLock::try_acquire(Path::new(&store), "main-ledger");
        if expected_busy {
            assert!(matches!(answer, Err(OwnerLockError::Busy)));
        } else {
            answer
                .expect("child takeover lock")
                .release()
                .expect("child unlock");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_lock_leaf_is_refused_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("lock root");
        let store = root.path().join("authority.sqlite");
        std::fs::write(&store, b"").expect("store sentinel");
        let path = lock_path(&store, "main-ledger").expect("lock path");
        let victim = root.path().join("victim");
        std::fs::write(&victim, b"keep").expect("victim");
        symlink(&victim, &path).expect("lock symlink");
        assert!(matches!(
            OwnerLock::try_acquire(&store, "main-ledger"),
            Err(OwnerLockError::Unavailable)
        ));
        assert_eq!(std::fs::read(victim).expect("victim bytes"), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_store_paths_use_the_exact_unix_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let store = PathBuf::from(std::ffi::OsString::from_vec(
            b"/private/authority-\xff.sqlite".to_vec(),
        ));
        let path = lock_path(&store, "main-ledger").expect("lock path");
        let mut digest = Sha256::new();
        digest.update(b"zerocode.runtime-owner-lock.v1");
        digest.update((store.as_os_str().as_bytes().len() as u64).to_le_bytes());
        digest.update(store.as_os_str().as_bytes());
        digest.update(("main-ledger".len() as u64).to_le_bytes());
        digest.update(b"main-ledger");
        let expected = format!(".runtime-owner-{:x}.lock", digest.finalize());
        assert_eq!(path.file_name().and_then(OsStr::to_str), Some(&*expected));
    }

    #[test]
    fn windows_path_contract_is_utf16_little_endian() {
        assert_eq!(
            utf16le([0x0041, 0xd55c, 0xd83d, 0xde80]),
            [0x41, 0x00, 0x5c, 0xd5, 0x3d, 0xd8, 0x80, 0xde]
        );
    }
}
