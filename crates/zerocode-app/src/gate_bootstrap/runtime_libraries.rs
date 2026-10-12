use std::fs;
use std::io::Read;
use std::path::{Component, Path};

use zerocode_core::orchestration::gate_reuse::{LibraryIdentity, validate_libraries};

const TOTAL_CAP: u64 = 256 * 1024 * 1024;
pub(super) const DECLARATION: &str = "runtime-libraries.json";

pub(super) fn verify_entries(libraries: &[LibraryIdentity]) -> Result<(), &'static str> {
    validate_libraries(libraries).map_err(|_| "digest_mismatch")?;
    let mut bytes = 0_u64;
    let mut targets = std::collections::BTreeSet::new();
    for library in libraries {
        let path = Path::new(&library.path);
        if path.components().any(|part| part == Component::ParentDir) {
            return Err("digest_mismatch");
        }
        let resolved = super::canonical(&library.resolved_path).map_err(|_| "digest_mismatch")?;
        if path.canonicalize().map_err(|_| "digest_mismatch")? != resolved {
            return Err("digest_mismatch");
        }
        let metadata = fs::symlink_metadata(&resolved).map_err(|_| "digest_mismatch")?;
        if !metadata.is_file() {
            return Err("digest_mismatch");
        }
        if targets.insert(resolved.clone()) {
            bytes = bytes.checked_add(metadata.len()).ok_or("digest_mismatch")?;
        }
        if bytes > TOTAL_CAP {
            return Err("invalid_request");
        }
        if super::file_hash(&resolved).map_err(|_| "digest_mismatch")? != library.sha256 {
            return Err("digest_mismatch");
        }
    }
    Ok(())
}

pub(super) fn read_and_verify(
    python: &Path,
    runtime: &Path,
) -> Result<Vec<LibraryIdentity>, &'static str> {
    let path = runtime.join(DECLARATION);
    if !fs::symlink_metadata(&path)
        .map_err(|_| "identity_unavailable")?
        .is_file()
    {
        return Err("identity_unavailable");
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "identity_unavailable")?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "identity_unavailable")?;
    if bytes.len() > 64 * 1024 {
        return Err("digest_mismatch");
    }
    let libraries: Vec<LibraryIdentity> =
        serde_json::from_slice(&bytes).map_err(|_| "digest_mismatch")?;
    verify_entries(&libraries)?;
    verify_closure(python, runtime, &libraries)?;
    verify_entries(&libraries)?;
    Ok(libraries)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn verify_closure(
    _python: &Path,
    _runtime: &Path,
    _libraries: &[LibraryIdentity],
) -> Result<(), &'static str> {
    Err("unsupported_runtime")
}

#[cfg(target_os = "macos")]
pub(super) fn verify_closure(
    python: &Path,
    runtime: &Path,
    libraries: &[LibraryIdentity],
) -> Result<(), &'static str> {
    use std::collections::{BTreeMap, BTreeSet};
    let declared = libraries
        .iter()
        .map(|library| (library.path.as_str(), library))
        .collect::<BTreeMap<_, _>>();
    let mut pending = vec![python.to_path_buf()];
    let mut directories = vec![runtime.to_path_buf()];
    let mut entries = 0;
    while let Some(directory) = directories.pop() {
        for item in fs::read_dir(directory).map_err(|_| "identity_unavailable")? {
            let path = item.map_err(|_| "identity_unavailable")?.path();
            entries += 1;
            if entries > super::ENTRY_CAP {
                return Err("identity_unavailable");
            }
            let metadata = fs::symlink_metadata(&path).map_err(|_| "identity_unavailable")?;
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() && is_macho(&path)? {
                pending.push(path);
            }
        }
    }
    let mut seen = BTreeSet::new();
    while let Some(path) = pending.pop() {
        let canonical = path.canonicalize().map_err(|_| "identity_unavailable")?;
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if !is_macho(&canonical)? {
            return Err("unsupported_runtime");
        }
        for name in load_names(&canonical)? {
            let dependency = Path::new(&name);
            if !dependency.is_absolute()
                || dependency
                    .components()
                    .any(|part| part == Component::ParentDir)
            {
                return Err("unknown_runtime_dependency");
            }
            if dependency.starts_with("/System/Library") || dependency.starts_with("/usr/lib") {
                continue;
            }
            let library = declared
                .get(name.as_str())
                .ok_or("unknown_runtime_dependency")?;
            if dependency.canonicalize().map_err(|_| "digest_mismatch")?
                != Path::new(&library.resolved_path)
            {
                return Err("digest_mismatch");
            }
            pending.push(Path::new(&library.resolved_path).to_path_buf());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_macho(path: &Path) -> Result<bool, &'static str> {
    let mut bytes = [0_u8; 4];
    let count = fs::File::open(path)
        .map_err(|_| "identity_unavailable")?
        .read(&mut bytes)
        .map_err(|_| "identity_unavailable")?;
    Ok(count == 4
        && matches!(
            bytes,
            [0xfe, 0xed, 0xfa, 0xce]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        ))
}

#[cfg(target_os = "macos")]
fn load_names(path: &Path) -> Result<Vec<String>, &'static str> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/otool")
        .args(["-l"])
        .arg(path)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "identity_unavailable")?;
    let mut raw = Vec::new();
    let read = child
        .stdout
        .take()
        .ok_or("identity_unavailable")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut raw);
    if read.is_err() || raw.len() > 1024 * 1024 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("identity_unavailable");
    }
    if !child.wait().map_err(|_| "identity_unavailable")?.success() {
        return Err("identity_unavailable");
    }
    let text = std::str::from_utf8(&raw).map_err(|_| "unknown_runtime_dependency")?;
    let mut needed = false;
    let mut names = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(command) = line.strip_prefix("cmd ") {
            if command == "LC_DYLD_ENVIRONMENT" {
                return Err("unsupported_runtime");
            }
            if needed {
                return Err("unknown_runtime_dependency");
            }
            needed = matches!(
                command,
                "LC_LOAD_DYLIB"
                    | "LC_LOAD_WEAK_DYLIB"
                    | "LC_REEXPORT_DYLIB"
                    | "LC_LOAD_UPWARD_DYLIB"
                    | "LC_LAZY_LOAD_DYLIB"
                    | "LC_LOAD_DYLINKER"
            );
        } else if needed
            && let Some(name) = line
                .strip_prefix("name ")
                .and_then(|value| value.rsplit_once(" (offset ").map(|(name, _)| name))
        {
            names.push(name.into());
            needed = false;
        }
    }
    if needed {
        return Err("unknown_runtime_dependency");
    }
    Ok(names)
}
