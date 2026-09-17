//! Install pipeline helpers: parse user-supplied sources, materialise
//! them on disk, walk plugin roots, and update the `settings.json`
//! claimed regions atomically.
//!
//! Everything here is shared by the manager's install / update /
//! uninstall flows; nothing should call out to the outside world
//! beyond `fs`, `Command::new("git")`, and the typed plugin error.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value};

use super::manifest_io::plugin_manifest_path;
use super::util::unix_time_ms;
use super::{PluginError, PluginInstallSource};

pub(crate) fn resolve_local_source(source: &str) -> Result<PathBuf, PluginError> {
    let path = PathBuf::from(source);
    if path.exists() {
        Ok(path)
    } else {
        Err(PluginError::NotFound(format!(
            "plugin source `{source}` was not found"
        )))
    }
}

pub(crate) fn parse_install_source(source: &str) -> Result<PluginInstallSource, PluginError> {
    // A `url#ref` suffix pins the install to a specific commit SHA, tag, or
    // branch. Split it off before the git-ness sniff so the bare URL is what
    // we test for the `.git` extension.
    let (url_part, reference) = split_git_reference(source);
    if url_part.starts_with("http://")
        || url_part.starts_with("https://")
        || url_part.starts_with("git@")
        || Path::new(url_part)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("git"))
    {
        Ok(PluginInstallSource::GitUrl {
            url: url_part.to_string(),
            reference,
        })
    } else {
        // Local paths are never pinned; keep any `#` as part of the path.
        Ok(PluginInstallSource::LocalPath {
            path: resolve_local_source(source)?,
        })
    }
}

/// Split a `url#ref` install spec into `(url, Some(ref))`, or `(source, None)`
/// when no pin is present. Only the first `#` is treated as the separator so
/// refs containing `#` (which git refs never do) stay intact.
fn split_git_reference(source: &str) -> (&str, Option<String>) {
    match source.split_once('#') {
        Some((url, reference)) if !reference.trim().is_empty() => {
            (url, Some(reference.trim().to_string()))
        }
        _ => (source, None),
    }
}

pub(crate) fn materialize_source(
    source: &PluginInstallSource,
    temp_root: &Path,
) -> Result<PathBuf, PluginError> {
    fs::create_dir_all(temp_root)?;
    match source {
        PluginInstallSource::LocalPath { path } => Ok(path.clone()),
        PluginInstallSource::GitUrl { url, reference } => {
            let destination = temp_root.join(format!("plugin-{}", unix_time_ms()));
            let mut clone = Command::new("git");
            clone.arg("clone");
            // A shallow clone is fastest, but a pinned commit SHA may be
            // unreachable from a depth-1 tip — fetch full history when pinned
            // so any commit/tag/branch is checkout-able.
            if reference.is_none() {
                clone.arg("--depth").arg("1");
            }
            let output = clone.arg(url).arg(&destination).output()?;
            if !output.status.success() {
                return Err(PluginError::CommandFailed(format!(
                    "git clone failed for `{url}`: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            if let Some(reference) = reference {
                let checkout = Command::new("git")
                    .arg("-C")
                    .arg(&destination)
                    .arg("checkout")
                    .arg("--detach")
                    .arg(reference)
                    .output()?;
                if !checkout.status.success() {
                    // A pinned ref that does not exist is a supply-chain
                    // failure, not a silent fall-through to the default branch.
                    let _ = fs::remove_dir_all(&destination);
                    return Err(PluginError::CommandFailed(format!(
                        "git checkout of pinned ref `{reference}` failed for `{url}`: {}",
                        String::from_utf8_lossy(&checkout.stderr).trim()
                    )));
                }
            }
            Ok(destination)
        }
    }
}

/// Resolve the commit SHA currently checked out in `repo`, if it is a git
/// working tree. Used to record install provenance for `GitUrl` sources.
pub(crate) fn git_head_commit(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Deterministic SHA-256 over a plugin directory tree.
///
/// Each file's repo-relative path and bytes are folded into the digest in
/// sorted order with length-prefixed framing, so the hash is stable across
/// machines and changes if any file's path, size, or contents change. This is
/// the integrity baseline stored at install time and re-checked on load.
pub(crate) fn hash_plugin_tree(root: &Path) -> Result<String, PluginError> {
    use sha2::{Digest, Sha256};

    let mut files = Vec::new();
    collect_files_relative(root, root, &mut files)?;
    files.sort();

    let mut hasher = Sha256::new();
    for relative in &files {
        let bytes = fs::read(root.join(relative))?;
        let path_bytes = relative.to_string_lossy();
        hasher.update((path_bytes.len() as u64).to_le_bytes());
        hasher.update(path_bytes.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn collect_files_relative(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), PluginError> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files_relative(root, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut acc, byte| {
            let _ = write!(acc, "{byte:02x}");
            acc
        })
}

pub(crate) fn discover_plugin_dirs(root: &Path) -> Result<Vec<PathBuf>, PluginError> {
    match fs::read_dir(root) {
        Ok(entries) => {
            let mut paths = Vec::new();
            for entry in entries {
                let path = entry?.path();
                if path.is_dir() && plugin_manifest_path(&path).is_ok() {
                    paths.push(path);
                }
            }
            paths.sort();
            Ok(paths)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(PluginError::Io(error)),
    }
}

pub(crate) fn copy_dir_all(source: &Path, destination: &Path) -> Result<(), PluginError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

pub(crate) fn update_settings_json(
    path: &Path,
    mut update: impl FnMut(&mut Map<String, Value>),
) -> Result<(), PluginError> {
    // Parent creation is deferred to `write_atomic`; the read below already
    // tolerates a missing file (NotFound → empty object).
    let mut root = match fs::read_to_string(path) {
        Ok(contents) if !contents.trim().is_empty() => serde_json::from_str::<Value>(&contents)?,
        Ok(_) => Value::Object(Map::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(PluginError::Io(error)),
    };

    let object = root.as_object_mut().ok_or_else(|| {
        PluginError::InvalidManifest(format!(
            "settings file {} must contain a JSON object",
            path.display()
        ))
    })?;
    update(object);
    write_atomic(path, serde_json::to_string_pretty(&root)?.as_bytes())?;
    Ok(())
}

/// Durably publish `contents` to `path`: write a sibling temp file then rename
/// it into place. `fs::write` truncates-then-writes in place, so a crash or
/// `ENOSPC` between the truncate and the final byte leaves a torn JSON file
/// (`installed.json` / `settings.json`) that no longer parses. A rename over the
/// destination is atomic on the same filesystem — the reader always observes the
/// complete old file or the complete new one, never a half-written one.
///
/// The temp name is per-process so two concurrent sessions writing the same
/// destination never publish each other's partial temp; a failed rename removes
/// its own temp instead of leaking it.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), PluginError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, contents)?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(PluginError::Io(error));
    }
    Ok(())
}

pub(crate) fn ensure_object<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
) -> &'a mut Map<String, Value> {
    if !root.get(key).is_some_and(Value::is_object) {
        root.insert(key.to_string(), Value::Object(Map::new()));
    }
    root.get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object should exist")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("install-{label}-{nanos}"))
    }

    fn git(repo: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git should run")
    }

    #[test]
    fn parse_install_source_pins_git_reference() {
        let source =
            parse_install_source("https://example.com/p.git#v1.2.3").expect("git url parses");
        assert_eq!(
            source,
            PluginInstallSource::GitUrl {
                url: "https://example.com/p.git".to_string(),
                reference: Some("v1.2.3".to_string()),
            }
        );

        let unpinned = parse_install_source("git@github.com:org/p.git").expect("git url parses");
        assert_eq!(
            unpinned,
            PluginInstallSource::GitUrl {
                url: "git@github.com:org/p.git".to_string(),
                reference: None,
            }
        );
    }

    #[test]
    fn hash_plugin_tree_is_deterministic_and_change_sensitive() {
        let root = temp_dir("hash");
        fs::create_dir_all(root.join("nested")).expect("mkdir");
        fs::write(root.join("a.txt"), "alpha").expect("write a");
        fs::write(root.join("nested").join("b.txt"), "beta").expect("write b");

        let first = hash_plugin_tree(&root).expect("hash");
        let second = hash_plugin_tree(&root).expect("hash again");
        assert_eq!(first, second, "same tree hashes identically");

        fs::write(root.join("a.txt"), "ALPHA").expect("rewrite a");
        let changed = hash_plugin_tree(&root).expect("hash after change");
        assert_ne!(first, changed, "content change flips the digest");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn materialize_git_source_checks_out_pinned_ref_and_rejects_unknown() {
        let origin = temp_dir("origin");
        fs::create_dir_all(&origin).expect("mkdir origin");
        // Build a minimal git repo with a tagged commit.
        assert!(git(&origin, &["init", "-q"]).status.success());
        let _ = git(&origin, &["config", "user.email", "t@t.test"]);
        let _ = git(&origin, &["config", "user.name", "Test"]);
        fs::write(origin.join("marker.txt"), "v1").expect("write marker");
        assert!(git(&origin, &["add", "."]).status.success());
        assert!(git(&origin, &["commit", "-q", "-m", "v1"]).status.success());
        assert!(git(&origin, &["tag", "v1.0.0"]).status.success());

        let temp_root = temp_dir("materialize");
        let url = origin.to_string_lossy().to_string();

        // Pinned to an existing tag → checks out, marker present.
        let pinned = PluginInstallSource::GitUrl {
            url: url.clone(),
            reference: Some("v1.0.0".to_string()),
        };
        let dest = materialize_source(&pinned, &temp_root).expect("pinned clone");
        assert!(dest.join("marker.txt").exists());
        assert!(git_head_commit(&dest).is_some(), "records resolved commit");

        // Pinned to a non-existent ref → rejected (supply-chain guard).
        let bad = PluginInstallSource::GitUrl {
            url,
            reference: Some("does-not-exist".to_string()),
        };
        let err = materialize_source(&bad, &temp_root).expect_err("bad ref rejected");
        assert!(matches!(err, PluginError::CommandFailed(_)));

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn write_atomic_replaces_via_rename_and_leaves_no_temp() {
        let root = temp_dir("atomic");
        fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("installed.json");

        // Seed an existing file, then overwrite it: the destination must end up
        // as the new content (never truncated), and no `.tmp.<pid>` sibling may
        // be left behind once the rename publishes the new bytes.
        fs::write(&path, b"{\"old\":true}").expect("seed old");
        write_atomic(&path, b"{\"new\":true}").expect("atomic write");

        assert_eq!(
            fs::read_to_string(&path).expect("read back"),
            "{\"new\":true}",
            "destination holds the complete new content, not a torn write"
        );

        // The temp + rename is the atomicity guarantee: after a successful
        // publish the only file in the directory is the destination itself.
        let leftovers: Vec<_> = fs::read_dir(&root)
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(
            leftovers,
            vec![std::ffi::OsString::from("installed.json")],
            "no temp sibling leaks after rename"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_atomic_creates_missing_parent_dirs() {
        let root = temp_dir("atomic-parent");
        // Nested parent does not exist yet: the helper must create it rather
        // than fail, matching the previous `create_dir_all` behaviour.
        let path = root.join("plugins").join("settings.json");
        write_atomic(&path, b"{}").expect("atomic write into new dir");
        assert_eq!(fs::read_to_string(&path).expect("read back"), "{}");

        let _ = fs::remove_dir_all(&root);
    }
}
