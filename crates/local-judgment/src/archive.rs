use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::catalog::relative;
use crate::storage::{child, plain, private_dir};
use crate::{Error, Result};

pub struct Budget {
    bytes: u64,
    entries: u32,
}

pub(crate) fn native(
    archive: &Path,
    root: &Path,
    runtime: &crate::llama::Runtime,
    cancel: &AtomicBool,
) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};

    runtime.validate()?;
    plain(archive)?;
    let mut package = tar::Archive::new(flate2::read::GzDecoder::new(
        File::open(archive).map_err(|_| Error::Storage)?,
    ));
    let target = child(root, "native")?;
    private_dir(&target)?;
    let mut budget = Budget {
        bytes: 256 * 1024 * 1024,
        entries: 256,
    };
    let declared: BTreeMap<_, _> = runtime
        .files
        .iter()
        .map(|artifact| (PathBuf::from(&artifact.path), artifact))
        .collect();
    let mut seen = BTreeSet::new();
    let mut copied = BTreeSet::new();
    let mut links = BTreeMap::new();
    for entry in package.entries().map_err(|_| Error::Integrity)? {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let mut entry = entry.map_err(|_| Error::Integrity)?;
        let path = entry.path().map_err(|_| Error::UnsafePath)?.into_owned();
        let name = path.to_str().ok_or(Error::UnsafePath)?;
        relative(name)?;
        if !seen.insert(path.clone()) {
            return Err(Error::UnsafePath);
        }
        let size = entry.size();
        budget.take(size)?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if kind.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|_| Error::UnsafePath)?
                .ok_or(Error::UnsafePath)?;
            let link = link.to_str().ok_or(Error::UnsafePath)?;
            relative(link)?;
            let parent = path
                .parent()
                .and_then(Path::to_str)
                .ok_or(Error::UnsafePath)?;
            // Joined in the archive's own `/` form: `Path::join` writes `\` on
            // Windows, and the copy reads this name back through `relative`.
            let joined = if parent.is_empty() {
                link.to_string()
            } else {
                format!("{parent}/{link}")
            };
            links.insert(path.clone(), PathBuf::from(joined));
        } else if kind.is_file() {
            if let Some(artifact) = declared.get(&path) {
                if size != artifact.bytes {
                    return Err(Error::Integrity);
                }
                let destination = child(&target, name)?;
                copy(&mut entry, &destination, size, cancel)?;
                set_native_mode(&destination, name)?;
                copied.insert(path);
            }
        } else {
            return Err(Error::UnsafePath);
        }
    }
    for (path, artifact) in &declared {
        if copied.contains(path) {
            continue;
        }
        let mut source = path.clone();
        let mut chain = BTreeSet::new();
        while let Some(next) = links.get(&source) {
            if !chain.insert(source.clone()) || chain.len() > 16 {
                return Err(Error::UnsafePath);
            }
            source = next.clone();
        }
        if !copied.contains(&source) || !declared.contains_key(&source) {
            return Err(Error::Integrity);
        }
        let source = child(&target, source.to_str().ok_or(Error::UnsafePath)?)?;
        plain(&source)?;
        budget.take(artifact.bytes)?;
        let name = path.to_str().ok_or(Error::UnsafePath)?;
        let destination = child(&target, name)?;
        copy(
            File::open(source).map_err(|_| Error::Storage)?,
            &destination,
            artifact.bytes,
            cancel,
        )?;
        set_native_mode(&destination, name)?;
    }
    for artifact in &runtime.files {
        crate::storage::verify(&child(&target, &artifact.path)?, artifact, cancel)?;
    }
    Ok(())
}

fn set_native_mode(path: &Path, name: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if name.ends_with("/LICENSE") {
            0o600
        } else {
            0o700
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|_| Error::Storage)?;
    }
    #[cfg(not(unix))]
    let _ = (path, name);
    Ok(())
}

impl Budget {
    pub fn new() -> Self {
        Self {
            bytes: 4 * 1024 * 1024 * 1024,
            entries: 100_000,
        }
    }

    fn take(&mut self, bytes: u64) -> Result<()> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(Error::TooLarge)?;
        self.entries = self.entries.checked_sub(1).ok_or(Error::TooLarge)?;
        Ok(())
    }
}

fn copy(mut source: impl Read, path: &Path, size: u64, cancel: &AtomicBool) -> Result<()> {
    let parent = path.parent().ok_or(Error::UnsafePath)?;
    private_dir(parent)?;
    let mut target = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|_| Error::Storage)?;
    let mut buffer = [0; 64 * 1024];
    let mut written = 0u64;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let read = source.read(&mut buffer).map_err(|_| Error::Storage)?;
        if read == 0 {
            break;
        }
        written = written.checked_add(read as u64).ok_or(Error::TooLarge)?;
        if written > size {
            return Err(Error::TooLarge);
        }
        target
            .write_all(&buffer[..read])
            .map_err(|_| Error::Storage)?;
    }
    if written != size {
        return Err(Error::Integrity);
    }
    Ok(())
}

pub fn python(archive: &Path, root: &Path, budget: &mut Budget, cancel: &AtomicBool) -> Result<()> {
    plain(archive)?;
    let gzip = flate2::read::GzDecoder::new(File::open(archive).map_err(|_| Error::Storage)?);
    let mut tar = tar::Archive::new(gzip);
    let mut links = Vec::new();
    for entry in tar.entries().map_err(|_| Error::Integrity)? {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let mut entry = entry.map_err(|_| Error::Integrity)?;
        let name = entry
            .path()
            .map_err(|_| Error::UnsafePath)?
            .to_string_lossy()
            .trim_end_matches('/')
            .to_owned();
        if !name.starts_with("python/") && name != "python" {
            return Err(Error::UnsafePath);
        }
        let path = child(root, &name)?;
        let kind = entry.header().entry_type();
        budget.take(entry.size())?;
        if kind.is_dir() {
            private_dir(&path)?;
        } else if kind.is_file() {
            let size = entry.size();
            let mode = entry.header().mode().map_err(|_| Error::Integrity)?;
            copy(&mut entry, &path, size, cancel)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600 | (mode & 0o111)))
                    .map_err(|_| Error::Storage)?;
            }
            #[cfg(not(unix))]
            let _ = mode;
        } else if kind.is_symlink() || kind.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(|_| Error::UnsafePath)?
                .ok_or(Error::UnsafePath)?;
            let target = target.to_str().ok_or(Error::UnsafePath)?.to_owned();
            links.push((name, target, kind.is_hard_link()));
        } else {
            return Err(Error::UnsafePath);
        }
    }
    let targets = links
        .iter()
        .map(|(name, target, hard)| Ok((name.clone(), link_target(name, target, *hard)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
    for (name, _, hard) in links {
        let path = child(root, &name)?;
        let mut resolved = targets.get(&name).ok_or(Error::UnsafePath)?.clone();
        let mut seen = std::collections::BTreeSet::from([name.clone()]);
        while let Some(next) = targets.get(&resolved) {
            if !seen.insert(resolved.clone()) || seen.len() > 64 {
                return Err(Error::UnsafePath);
            }
            resolved.clone_from(next);
        }
        if Path::new(&resolved)
            .ancestors()
            .skip(1)
            .any(|path| path.to_str().is_some_and(|path| targets.contains_key(path)))
        {
            return Err(Error::UnsafePath);
        }
        let original = child(root, &resolved)?;
        plain(&original)?;
        if hard {
            fs::hard_link(original, path).map_err(|_| Error::Storage)?;
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(relative_link(&name, &resolved), path)
                .map_err(|_| Error::Storage)?;
            #[cfg(not(unix))]
            {
                if !original.is_file() {
                    return Err(Error::UnsafePath);
                }
                fs::hard_link(original, path).map_err(|_| Error::Storage)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn relative_link(name: &str, target: &str) -> String {
    let mut parent: Vec<_> = name.split('/').collect();
    parent.pop();
    let target: Vec<_> = target.split('/').collect();
    let common = parent
        .iter()
        .zip(&target)
        .take_while(|(a, b)| a == b)
        .count();
    let mut result = vec![".."; parent.len() - common];
    result.extend_from_slice(&target[common..]);
    result.join("/")
}

fn link_target(name: &str, target: &str, hard: bool) -> Result<String> {
    if target.contains(['\\', ':', '\0']) || Path::new(target).is_absolute() {
        return Err(Error::UnsafePath);
    }
    let mut parts = if hard {
        Vec::new()
    } else {
        name.split('/').map(str::to_owned).collect::<Vec<_>>()
    };
    if !hard {
        parts.pop();
    }
    for part in Path::new(target).components() {
        match part {
            Component::Normal(part) => {
                parts.push(part.to_str().ok_or(Error::UnsafePath)?.to_owned())
            }
            Component::ParentDir if parts.len() > 1 => {
                parts.pop();
            }
            Component::CurDir => {}
            _ => return Err(Error::UnsafePath),
        }
    }
    let result = parts.join("/");
    relative(&result)?;
    if !result.starts_with("python/") {
        return Err(Error::UnsafePath);
    }
    Ok(result)
}

pub fn wheel(
    archive: &Path,
    root: &Path,
    site: &str,
    budget: &mut Budget,
    cancel: &AtomicBool,
) -> Result<()> {
    plain(archive)?;
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|_| Error::Storage)?)
        .map_err(|_| Error::Integrity)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|_| Error::Integrity)?;
        let name = entry.name().trim_end_matches('/');
        relative(name)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(Error::UnsafePath);
        }
        budget.take(entry.size())?;
        if entry.is_dir() || name.ends_with(".pth") {
            continue;
        }
        let destination = wheel_path(name, site)?;
        let Some(destination) = destination else {
            continue;
        };
        let path = child(root, &destination)?;
        let size = entry.size();
        copy(&mut entry, &path, size, cancel)?;
    }
    Ok(())
}

fn wheel_path(name: &str, site: &str) -> Result<Option<String>> {
    let parts: Vec<_> = name.split('/').collect();
    if parts[0].ends_with(".data") {
        if parts.len() < 3 {
            return Err(Error::UnsafePath);
        }
        let rest = parts[2..].join("/");
        match parts[1] {
            "purelib" | "platlib" => Ok(Some(format!("{site}/{rest}"))),
            "data" => Ok(Some(format!("python/{rest}"))),
            "headers" => Ok(Some(format!("python/include/{rest}"))),
            "scripts" => Ok(None),
            _ => Err(Error::UnsafePath),
        }
    } else {
        Ok(Some(format!("{site}/{name}")))
    }
}

#[cfg(test)]
pub(crate) fn resolve_test_link(name: &str, target: &str) -> Result<PathBuf> {
    link_target(name, target, false).map(PathBuf::from)
}
