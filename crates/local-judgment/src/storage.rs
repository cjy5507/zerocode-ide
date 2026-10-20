use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::{
    Artifact, Catalog, MAX_CATALOG_BYTES, MAX_MODELS, Model, RuntimeProfile, relative,
};
use crate::{Error, Result};

pub struct Store {
    root: PathBuf,
}

pub struct Stage {
    path: PathBuf,
    committed: bool,
    external: Option<Location>,
    tracker: Option<PathBuf>,
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Location {
    parent: PathBuf,
    model_id: String,
    fingerprint: String,
    nonce: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ownership {
    fingerprint: String,
    nonce: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    fingerprint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeMetadata {
    fingerprint: String,
    input_template: String,
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
        for name in [
            "models",
            "runtime",
            "partial",
            "sources",
            "locations",
            "external-partial",
        ] {
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
        store.clean_external_partial()?;
        Ok(store)
    }

    pub fn stage(&self) -> Result<Stage> {
        let path = child(&self.root, &format!("partial/{}", uuid::Uuid::new_v4()))?;
        private_dir(&path)?;
        Ok(Stage {
            path,
            committed: false,
            external: None,
            tracker: None,
        })
    }

    pub fn default_models_directory(&self) -> PathBuf {
        self.root.join("models")
    }

    pub fn require_model_space(&self, bytes: u64, destination: Option<&Path>) -> Result<()> {
        let directory = match destination {
            Some(directory) => selected_parent(directory)?,
            None => self.root.clone(),
        };
        if available_space(&directory)? < bytes.saturating_add(512 * 1024 * 1024) {
            return Err(Error::Space);
        }
        Ok(())
    }

    pub fn model_stage(&self, model: &Model, destination: Option<&Path>) -> Result<Stage> {
        let Some(destination) = destination else {
            return self.stage();
        };
        let parent = selected_parent(destination)?;
        if parent == self.default_models_directory() {
            return self.stage();
        }
        if self.location(model)?.is_some() || self.model_path(model)?.exists() {
            return Err(Error::DestinationUnavailable);
        }
        let location = Location {
            parent,
            model_id: model.id.clone(),
            fingerprint: model.fingerprint()?,
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let path = location_path(&location, true)?;
        io(fs::create_dir(&path))?;
        let mut stage = Stage {
            path,
            committed: false,
            external: Some(location.clone()),
            tracker: None,
        };
        private_dir(stage.path())?;
        let ownership = serde_json::to_vec(&Ownership {
            fingerprint: location.fingerprint.clone(),
            nonce: location.nonce.clone(),
        })
        .map_err(|_| Error::Storage)?;
        write_new(&child(stage.path(), "ownership.json")?, &ownership)?;
        let tracker = child(
            &self.root,
            &format!("external-partial/{}.json", location.nonce),
        )?;
        write_new(
            &tracker,
            &serde_json::to_vec(&location).map_err(|_| Error::Storage)?,
        )?;
        stage.tracker = Some(tracker);
        Ok(stage)
    }

    fn location(&self, model: &Model) -> Result<Option<Location>> {
        let path = child(&self.root, &format!("locations/{}", model.id))?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = bounded_read(&child(&path, "location.json")?, 16 * 1024)?;
        let location: Location = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if location.model_id != model.id
            || location.fingerprint != model.fingerprint()?
            || !valid_receipt(&path, Ok(digest))
        {
            return Err(Error::Integrity);
        }
        Ok(Some(location))
    }

    fn commit_location(&self, location: &Location) -> Result<()> {
        let bytes = serde_json::to_vec(location).map_err(|_| Error::Storage)?;
        let mut stage = self.stage()?;
        write_new(&child(stage.path(), "location.json")?, &bytes)?;
        stage.commit(
            &child(&self.root, &format!("locations/{}", location.model_id))?,
            &format!("{:x}", Sha256::digest(&bytes)),
        )
    }

    fn clean_external_partial(&self) -> Result<()> {
        let directory = child(&self.root, "external-partial")?;
        let mut count = 0usize;
        for entry in io(fs::read_dir(directory))? {
            count += 1;
            if count > MAX_MODELS {
                return Err(Error::TooLarge);
            }
            let tracker = io(entry)?.path();
            let location: Location = serde_json::from_slice(&bounded_read(&tracker, 16 * 1024)?)
                .map_err(|_| Error::Integrity)?;
            let partial = location_path(&location, true)?;
            if partial.exists() {
                owned_location(&partial, &location)?;
                io(fs::remove_dir_all(&partial))?;
            }
            let destination = location_path(&location, false)?;
            let registered = child(&self.root, &format!("locations/{}", location.model_id))?;
            if destination.exists() && !registered.exists() {
                owned_location(&destination, &location)?;
                io(fs::remove_dir_all(destination))?;
            }
            io(fs::remove_file(tracker))?;
        }
        Ok(())
    }

    pub fn registered_catalogs(&self) -> Result<Vec<Catalog>> {
        let sources = child(&self.root, "sources")?;
        let mut paths = Vec::new();
        for entry in io(fs::read_dir(sources))? {
            if paths.len() >= MAX_MODELS {
                return Err(Error::TooLarge);
            }
            paths.push(io(entry)?.path());
        }
        paths.sort();
        let mut catalogs = Vec::new();
        for path in paths {
            plain(&path)?;
            let document = bounded_read(&child(&path, "catalog.json")?, MAX_CATALOG_BYTES)?;
            let document = std::str::from_utf8(&document).map_err(|_| Error::Manifest)?;
            let catalog = Catalog::parse(document)?;
            let fingerprint = catalog.fingerprint()?;
            if path.file_name().and_then(|name| name.to_str()) != Some(fingerprint.as_str())
                || !valid_receipt(&path, Ok(fingerprint))
            {
                return Err(Error::Integrity);
            }
            catalogs.push(catalog);
        }
        Ok(catalogs)
    }

    pub fn register_catalog(&self, catalog: &Catalog) -> Result<()> {
        catalog.validate()?;
        let document = serde_json::to_vec(catalog).map_err(|_| Error::Manifest)?;
        if document.len() > MAX_CATALOG_BYTES {
            return Err(Error::TooLarge);
        }
        self.require_space(document.len() as u64)?;
        let mut stage = self.stage()?;
        let mut file = io(OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(stage.path.join("catalog.json")))?;
        io(file.write_all(&document))?;
        io(file.sync_all())?;
        drop(file);
        let fingerprint = catalog.fingerprint()?;
        stage.commit(
            &child(&self.root, &format!("sources/{fingerprint}"))?,
            &fingerprint,
        )
    }

    pub fn model_path(&self, model: &Model) -> Result<PathBuf> {
        if let Some(location) = self.location(model)? {
            let path = location_path(&location, false)?;
            if path.exists() {
                owned_location(&path, &location)?;
            }
            return Ok(path);
        }
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
                && profile.native.as_ref().is_none_or(|native| {
                    native.files.iter().all(|artifact| {
                        native.file_path(&path, &artifact.path).is_ok_and(|file| {
                            fs::metadata(file)
                                .is_ok_and(|info| info.is_file() && info.len() == artifact.bytes)
                        })
                    })
                })
        })
    }

    pub fn model_template(&self, model: &Model) -> Option<String> {
        if model.engine != crate::catalog::EngineKind::LlamaCpp {
            return None;
        }
        let directory = self.model_path(model).ok()?;
        if !valid_receipt(&directory, model.fingerprint()) {
            return None;
        }
        let file = child(&directory, "native-facts.json").ok()?;
        let metadata: NativeMetadata =
            serde_json::from_slice(&bounded_read(&file, 4096).ok()?).ok()?;
        if metadata.fingerprint != model.fingerprint().ok()?
            || crate::gguf::builtin_template(&metadata.input_template)
                != Some(metadata.input_template.as_str())
        {
            return None;
        }
        Some(metadata.input_template)
    }

    pub fn prepare_model_metadata(
        directory: &Path,
        model: &Model,
        cancel: &AtomicBool,
    ) -> Result<()> {
        if model.engine == crate::catalog::EngineKind::LlamaCpp {
            let (_, _, template) = crate::llama::model_info_with_cancel(directory, model, cancel)?;
            let metadata = NativeMetadata {
                fingerprint: model.fingerprint()?,
                input_template: template.to_owned(),
            };
            let bytes = serde_json::to_vec(&metadata).map_err(|_| Error::Storage)?;
            write_new(&child(directory, "native-facts.json")?, &bytes)?;
        }
        Ok(())
    }

    pub fn commit_model(&self, stage: &mut Stage, model: &Model) -> Result<()> {
        if let Some(location) = stage.external.clone() {
            if location.fingerprint != model.fingerprint()? {
                return Err(Error::Integrity);
            }
            owned_location(stage.path(), &location)?;
            let destination = location_path(&location, false)?;
            stage.commit(&destination, &location.fingerprint)?;
            if let Err(error) = self.commit_location(&location) {
                owned_location(&destination, &location)?;
                io(fs::remove_dir_all(destination))?;
                return Err(error);
            }
            return Ok(());
        }
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
        if self.location(model)?.is_some() {
            io(fs::remove_dir_all(child(
                &self.root,
                &format!("locations/{}", model.id),
            )?))?;
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
        let mut cleaned = true;
        if !self.committed {
            cleaned = if let Some(location) = &self.external {
                if owned_location(&self.path, location).is_ok() {
                    fs::remove_dir_all(&self.path).is_ok()
                } else {
                    fs::remove_dir(&self.path).is_ok()
                }
            } else {
                fs::remove_dir_all(&self.path).is_ok()
            };
        }
        if cleaned && let Some(tracker) = &self.tracker {
            let _ = fs::remove_file(tracker);
        }
    }
}

fn selected_parent(directory: &Path) -> Result<PathBuf> {
    plain(directory).map_err(|error| {
        if error == Error::UnsafePath {
            error
        } else {
            Error::DestinationUnavailable
        }
    })?;
    let parent = fs::canonicalize(directory).map_err(|_| Error::DestinationUnavailable)?;
    if !parent.is_dir() {
        return Err(Error::DestinationUnavailable);
    }
    Ok(parent)
}

fn location_path(location: &Location, partial: bool) -> Result<PathBuf> {
    relative(&location.model_id)?;
    if location.model_id.contains('/')
        || uuid::Uuid::parse_str(&location.nonce).is_err()
        || location.fingerprint.len() != 64
        || !location
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !location.parent.is_absolute()
        || selected_parent(&location.parent)? != location.parent
    {
        return Err(Error::UnsafePath);
    }
    let name = if partial {
        format!(".zerocode-native-{}.partial", location.nonce)
    } else {
        format!("zerocode-native-{}-{}", location.model_id, location.nonce)
    };
    child(&location.parent, &name)
}

fn owned_location(path: &Path, location: &Location) -> Result<()> {
    plain(path)?;
    let bytes = bounded_read(&child(path, "ownership.json")?, 1024)?;
    let ownership: Ownership = serde_json::from_slice(&bytes).map_err(|_| Error::Integrity)?;
    if ownership.nonce != location.nonce || ownership.fingerprint != location.fingerprint {
        return Err(Error::Integrity);
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = io(OpenOptions::new().create_new(true).write(true).open(path))?;
    io(file.write_all(bytes))?;
    io(file.sync_all())
}

pub fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or(Error::UnsafePath)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::UnsafePath)?;
    let destination = child(parent, name)?;
    let partial = child(parent, &format!("{name}.partial"))?;
    remove(&partial)?;
    write_new(&partial, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        io(fs::set_permissions(
            &partial,
            fs::Permissions::from_mode(0o600),
        ))?;
    }
    io(fs::rename(partial, destination))
}

pub fn remove(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            plain(path.parent().ok_or(Error::UnsafePath)?)?;
            plain(path)?;
            if !metadata.is_file() {
                return Err(Error::UnsafePath);
            }
            io(fs::remove_file(path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Error::Storage),
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

#[cfg(test)]
mod replacement_tests {
    use super::*;

    #[test]
    fn replacement_overwrites_an_existing_file_and_remove_is_repeatable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("intent.json");
        fs::write(&path, b"before").unwrap();
        replace(&path, b"after").unwrap();
        assert_eq!(
            bounded_read(&path, 1024).unwrap(),
            b"after",
            "replace must publish replacement bytes"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        remove(&path).unwrap();
        assert!(!path.exists());
        remove(&path).unwrap();
    }
}
