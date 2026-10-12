use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::{Artifact, Model, RuntimeProfile, relative};
use crate::{Error, Result};

pub struct Store {
    root: PathBuf,
}

pub struct Stage {
    path: PathBuf,
    committed: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    fingerprint: String,
}

fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|_| Error::Storage)
}

pub fn plain(path: &Path) -> Result<()> {
    let info = io(fs::symlink_metadata(path))?;
    if info.file_type().is_symlink() || (!info.is_file() && !info.is_dir()) {
        return Err(Error::UnsafePath);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if info.file_attributes() & 0x400 != 0 {
            return Err(Error::UnsafePath);
        }
    }
    Ok(())
}

pub fn child(root: &Path, name: &str) -> Result<PathBuf> {
    let name = relative(name)?;
    plain(root)?;
    let mut path = root.to_path_buf();
    for part in name.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(_) => plain(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Storage),
        }
    }
    Ok(path)
}

pub fn private_dir(path: &Path) -> Result<()> {
    io(fs::create_dir_all(path))?;
    plain(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        io(fs::set_permissions(path, fs::Permissions::from_mode(0o700)))?;
    }
    Ok(())
}

impl Store {
    pub fn open(parent: &Path) -> Result<Self> {
        private_dir(parent)?;
        let root = child(parent, "managed-judgment")?;
        private_dir(&root)?;
        let store = Self {
            root: io(fs::canonicalize(root))?,
        };
        for name in ["models", "runtime", "partial"] {
            private_dir(&child(&store.root, name)?)?;
        }
        for entry in io(fs::read_dir(store.root.join("partial")))? {
            let path = io(entry)?.path();
            plain(&path)?;
            if path.is_dir() {
                io(fs::remove_dir_all(path))?;
            } else {
                io(fs::remove_file(path))?;
            }
        }
        Ok(store)
    }

    pub fn stage(&self) -> Result<Stage> {
        let path = child(&self.root, &format!("partial/{}", uuid::Uuid::new_v4()))?;
        private_dir(&path)?;
        Ok(Stage {
            path,
            committed: false,
        })
    }

    pub fn model_path(&self, model: &Model) -> Result<PathBuf> {
        child(&self.root, &format!("models/{}", model.id))
    }

    pub fn runtime_path(&self, profile: &RuntimeProfile) -> Result<PathBuf> {
        child(&self.root, &format!("runtime/{}", profile.id))
    }

    pub fn installed(&self, model: &Model) -> bool {
        self.model_path(model).is_ok_and(|path| {
            valid_receipt(&path, model.fingerprint())
                && model.files.iter().all(|file| {
                    child(&path, &file.path).is_ok_and(|path| {
                        fs::metadata(path)
                            .is_ok_and(|info| info.is_file() && info.len() == file.bytes)
                    })
                })
        })
    }

    pub fn runtime_ready(&self, profile: &RuntimeProfile) -> bool {
        self.runtime_path(profile).is_ok_and(|path| {
            valid_receipt(&path, profile.fingerprint())
                && child(&path, &profile.executable).is_ok_and(|executable| executable.is_file())
        })
    }

    pub fn commit_model(&self, stage: &mut Stage, model: &Model) -> Result<()> {
        stage.commit(&self.model_path(model)?, &model.fingerprint()?)
    }

    pub fn commit_runtime(&self, stage: &mut Stage, profile: &RuntimeProfile) -> Result<()> {
        stage.commit(&self.runtime_path(profile)?, &profile.fingerprint()?)
    }

    pub fn remove(&self, model: &Model) -> Result<()> {
        let path = self.model_path(model)?;
        if path.exists() {
            plain(&path)?;
            io(fs::remove_dir_all(path))?;
        }
        Ok(())
    }

    pub fn require_space(&self, bytes: u64) -> Result<()> {
        let available = available_space(&self.root)?;
        if available < bytes.saturating_add(512 * 1024 * 1024) {
            return Err(Error::Space);
        }
        Ok(())
    }
}

impl Stage {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn commit(&mut self, destination: &Path, fingerprint: &str) -> Result<()> {
        if destination.exists() {
            return Err(Error::Storage);
        }
        let receipt = serde_json::to_vec(&Receipt {
            fingerprint: fingerprint.to_owned(),
        })
        .map_err(|_| Error::Storage)?;
        let mut file = io(OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.path.join("receipt.json")))?;
        io(file.write_all(&receipt))?;
        io(file.sync_all())?;
        drop(file);
        io(fs::rename(&self.path, destination))?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn valid_receipt(path: &Path, expected: Result<String>) -> bool {
    let Ok(expected) = expected else { return false };
    child(path, "receipt.json")
        .and_then(|path| bounded_read(&path, 1024))
        .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).map_err(|_| Error::Storage))
        .is_ok_and(|receipt| receipt.fingerprint == expected)
}

pub fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    plain(path)?;
    let mut bytes = Vec::new();
    io(File::open(path))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > limit {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

pub fn verify(path: &Path, artifact: &Artifact, cancel: &AtomicBool) -> Result<()> {
    plain(path)?;
    let mut file = io(File::open(path))?;
    if io(file.metadata())?.len() != artifact.bytes {
        return Err(Error::Integrity);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let read = io(file.read(&mut buffer))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if format!("{:x}", digest.finalize()) != artifact.sha256.to_ascii_lowercase() {
        return Err(Error::Integrity);
    }
    Ok(())
}

#[cfg(unix)]
fn available_space(path: &Path) -> Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| Error::UnsafePath)?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is terminated, and statvfs initializes info on success.
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(Error::Storage);
    }
    // SAFETY: statvfs succeeded above.
    let info = unsafe { info.assume_init() };
    let bytes = u128::from(info.f_bavail).saturating_mul(u128::from(info.f_frsize));
    Ok(u64::try_from(bytes).unwrap_or(u64::MAX))
}

#[cfg(windows)]
fn available_space(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    // SAFETY: path is terminated, and the output points to one writable u64.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(Error::Storage);
    }
    Ok(available)
}

#[cfg(not(any(unix, windows)))]
fn available_space(_: &Path) -> Result<u64> {
    Err(Error::Unsupported)
}
