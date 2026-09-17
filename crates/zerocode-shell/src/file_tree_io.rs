//! Native path primitives: no-clobber moves and exact system-trash receipts.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: std::time::SystemTime,
    is_dir: bool,
}
impl Identity {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| "tree.changed")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                is_dir: metadata.is_dir(),
            })
        }
        #[cfg(not(unix))]
        Ok(Self {
            created: metadata.created().map_err(|_| "tree.changed")?,
            is_dir: metadata.is_dir(),
        })
    }
    pub(crate) fn check(&self, path: &Path) -> Result<(), String> {
        if &Self::read(path)? == self {
            Ok(())
        } else {
            Err("tree.changed".into())
        }
    }
}

pub(crate) fn vacant(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err("tree.conflict".into()),
    }
}

/// The kernel refuses a destination created after preflight, including an
/// empty directory or a dangling symlink. std::fs::rename can overwrite it.
pub(crate) fn move_no_replace(from: &Path, to: &Path) -> Result<(), String> {
    vacant(to)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let source =
            std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| "tree.invalidMove")?;
        let target =
            std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| "tree.invalidMove")?;
        #[cfg(target_os = "macos")]
        let answer =
            unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let answer = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err("tree.failed".into());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if answer == 0 {
            Ok(())
        } else {
            Err("tree.failed".into())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{Win32::Storage::FileSystem::MoveFileW, core::PCWSTR};
        let source: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe { MoveFileW(PCWSTR(source.as_ptr()), PCWSTR(target.as_ptr())) }
            .map_err(|_| "tree.failed".into())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TrashReceipt {
    pub(crate) original: PathBuf,
    #[cfg(target_os = "macos")]
    trashed: PathBuf,
    #[cfg(not(target_os = "macos"))]
    item: trash::TrashItem,
}
impl TrashReceipt {
    pub(crate) fn check(&self, identity: &Identity) -> Result<(), String> {
        vacant(&self.original)?;
        #[cfg(target_os = "macos")]
        identity.check(&self.trashed)?;
        #[cfg(not(target_os = "macos"))]
        {
            let _ = identity;
            if !trash::os_limited::list()
                .map_err(|_| "tree.failed")?
                .contains(&self.item)
            {
                return Err("tree.changed".into());
            }
        }
        Ok(())
    }
    pub(crate) fn restore(&self) -> Result<(), String> {
        vacant(&self.original)?;
        #[cfg(target_os = "macos")]
        return move_no_replace(&self.trashed, &self.original);
        #[cfg(not(target_os = "macos"))]
        trash::os_limited::restore_all([self.item.clone()]).map_err(|_| "tree.failed".into())
    }
}

pub(crate) fn trash_path(path: &Path) -> Result<TrashReceipt, String> {
    #[cfg(target_os = "macos")]
    {
        // trash-rs calls this same native primitive but discards its resulting
        // URL and does not expose restore on macOS. Retain that receipt here;
        // no copying, no private trash tree, and no filename guessing.
        use objc2_foundation::{NSFileManager, NSString, NSURL};
        let source = path.to_str().ok_or("tree.invalidMove")?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(source));
        let mut result = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut result))
            .map_err(|_| "tree.failed")?;
        let trashed = result.and_then(|url| url.path()).ok_or("tree.failed")?;
        Ok(TrashReceipt {
            original: path.to_path_buf(),
            trashed: PathBuf::from(trashed.to_string()),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let before = trash::os_limited::list().map_err(|_| "tree.failed")?;
        trash::delete(path).map_err(|_| "tree.failed")?;
        let item = trash::os_limited::list()
            .map_err(|_| "tree.failed")?
            .into_iter()
            .find(|item| item.original_path() == path && !before.contains(item))
            .ok_or("tree.failed")?;
        Ok(TrashReceipt {
            original: path.to_path_buf(),
            item,
        })
    }
}
