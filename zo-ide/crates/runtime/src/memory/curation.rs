use std::collections::BTreeSet;
#[cfg(any(not(unix), test))]
use std::fs;
use std::io;
use std::path::Path;
#[cfg(not(unix))]
use std::path::PathBuf;

#[must_use]
pub fn is_safe_memory_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug != "MEMORY"
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn archive_matching_memory_entries<F>(
    memory_dir: &Path,
    archive_dir: &Path,
    should_archive: F,
) -> io::Result<Vec<String>>
where
    F: Fn(&str, &str) -> bool,
{
    #[cfg(unix)]
    {
        archive_matching_memory_entries_retained(
            memory_dir,
            archive_dir,
            |_memory, slug, body| should_archive(slug, body),
            |_memory, _slug| Ok(()),
        )
    }
    #[cfg(not(unix))]
    {
        archive_matching_memory_entries_path(memory_dir, archive_dir, should_archive)
    }
}

#[cfg(unix)]
pub(crate) fn archive_matching_memory_entries_retained<F, R>(
    memory_dir: &Path,
    archive_dir: &Path,
    mut should_archive: F,
    mut after_archive: R,
) -> io::Result<Vec<String>>
where
    F: FnMut(&crate::secure_fs::RetainedDir, &str, &str) -> bool,
    R: FnMut(&crate::secure_fs::RetainedDir, &str) -> io::Result<()>,
{
    use crate::secure_fs::RetainedDir;

    let archive_parent_path = archive_dir.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive path must have a parent directory",
        )
    })?;
    let archive_name = archive_dir.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive path must name a directory",
        )
    })?;
    let Ok(memory) = RetainedDir::open(memory_dir) else {
        return Ok(Vec::new());
    };
    // Retain the archive before evaluating attacker-controlled entries. A child
    // archive is derived from the same memory capability; sibling archives keep
    // their separately retained parent capability.
    let archive = match archive_dir.strip_prefix(memory_dir) {
        Ok(relative) if !relative.as_os_str().is_empty() => {
            memory.ensure_private_subdir(relative)?
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive path must not be the memory directory",
            ));
        }
        Err(_) => {
            let archive_parent = RetainedDir::open(archive_parent_path)?;
            archive_parent.ensure_private_subdir(Path::new(archive_name))?
        }
    };
    let Ok(entry_names) = memory.entry_names() else {
        return Ok(Vec::new());
    };
    let mut archived = Vec::new();
    let mut errors = Vec::new();
    for file_name in entry_names {
        let path = Path::new(&file_name);
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !is_safe_memory_slug(slug) {
            continue;
        }
        let Ok(mut entry) = memory.open_regular_file(path) else {
            continue;
        };
        let Ok(body) = entry.read_to_string() else {
            continue;
        };
        if !should_archive(&memory, slug, &body) {
            continue;
        }
        if memory
            .rename_file_no_replace(path, &entry, &archive, path)
            .unwrap_or(false)
        {
            // Every completed rename must participate in the index cleanup,
            // even if its post-archive action fails.
            archived.push(slug.to_string());
            if let Err(error) = after_archive(&memory, slug) {
                errors.push(io::Error::new(
                    error.kind(),
                    format!("post-archive action failed for {slug}: {error}"),
                ));
            }
        }
    }

    if !archived.is_empty() {
        if let Err(error) =
            remove_index_pointers_retained(&memory, Path::new("MEMORY.md"), &archived)
        {
            errors.push(io::Error::new(
                error.kind(),
                format!("memory index update failed: {error}"),
            ));
        }
    }
    if let Some(first) = errors.first() {
        let kind = first.kind();
        let message = errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(io::Error::new(kind, message));
    }
    Ok(archived)
}

#[cfg(not(unix))]
fn archive_matching_memory_entries_path<F>(
    memory_dir: &Path,
    archive_dir: &Path,
    should_archive: F,
) -> io::Result<Vec<String>>
where
    F: Fn(&str, &str) -> bool,
{
    let Ok(read_dir) = fs::read_dir(memory_dir) else {
        return Ok(Vec::new());
    };

    let mut archived = Vec::new();
    for dir_entry in read_dir.flatten() {
        let path = dir_entry.path();
        if !is_regular_markdown_entry(&path) {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !is_safe_memory_slug(slug) {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        if !should_archive(slug, &body) {
            continue;
        }

        ensure_real_dir(archive_dir)?;
        let archive_path = archive_dir.join(format!("{slug}.md"));
        if archive_path.exists() || fs::rename(&path, archive_path).is_err() {
            continue;
        }
        archived.push(slug.to_string());
    }

    if !archived.is_empty() {
        remove_index_pointers(&memory_dir.join("MEMORY.md"), &archived)?;
    }
    Ok(archived)
}

#[cfg(not(unix))]
fn ensure_real_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive path is not a real directory",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn is_regular_markdown_entry(path: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_file()
}

pub fn remove_index_pointers(index_path: &Path, slugs: &[String]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use crate::secure_fs::RetainedDir;

        let parent = index_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let Some(file_name) = index_path.file_name() else {
            return Ok(());
        };
        let Ok(parent) = RetainedDir::open(parent) else {
            return Ok(());
        };
        remove_index_pointers_retained(&parent, Path::new(file_name), slugs)
    }
    #[cfg(not(unix))]
    {
        remove_index_pointers_path(index_path, slugs)
    }
}

#[cfg(unix)]
fn remove_index_pointers_retained(
    parent: &crate::secure_fs::RetainedDir,
    index_name: &Path,
    slugs: &[String],
) -> io::Result<()> {
    let Ok(mut index) = parent.open_regular_file(index_name) else {
        return Ok(());
    };
    let Ok(content) = index.read_to_string() else {
        return Ok(());
    };
    let Some(replacement) = index_without_slugs(&content, slugs) else {
        return Ok(());
    };
    parent.write_atomic_owner_only_retained(index_name, replacement.as_bytes())
}

#[cfg(not(unix))]
fn remove_index_pointers_path(index_path: &Path, slugs: &[String]) -> io::Result<()> {
    let Ok(content) = fs::read_to_string(index_path) else {
        return Ok(());
    };
    let Some(replacement) = index_without_slugs(&content, slugs) else {
        return Ok(());
    };
    atomic_write(index_path, &replacement)
}

fn index_without_slugs(content: &str, slugs: &[String]) -> Option<String> {
    let slugs: BTreeSet<&str> = slugs.iter().map(String::as_str).collect();
    let mut removed_any = false;
    let kept = content
        .lines()
        .filter(|line| {
            let remove = parse_index_slug(line).is_some_and(|slug| slugs.contains(slug.as_str()));
            removed_any |= remove;
            !remove
        })
        .collect::<Vec<_>>();
    removed_any.then(|| format!("{}\n", kept.join("\n")))
}

#[cfg(not(unix))]
fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let tmp_path = tmp_path_for(path);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, content.as_bytes()))?;
    fs::rename(&tmp_path, path).inspect_err(|_error| {
        let _ = fs::remove_file(&tmp_path);
    })
}

#[cfg(not(unix))]
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("MEMORY.md");
    tmp.set_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    tmp
}

fn parse_index_slug(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("- [")?;
    let (slug, rest) = rest.split_once("](")?;
    rest.strip_prefix(slug)?.strip_prefix(".md)")?;
    is_safe_memory_slug(slug).then(|| slug.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_moves_matching_files_and_removes_index_pointers() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let archive_dir = tmp.path().join("archive");
        fs::create_dir_all(&memory_dir).unwrap();
        fs::write(memory_dir.join("keep.md"), "keep").unwrap();
        fs::write(memory_dir.join("drop.md"), "drop me").unwrap();
        fs::write(
            memory_dir.join("MEMORY.md"),
            "# Memory\n- [keep](keep.md) — keep\n- [drop](drop.md) — drop\n",
        )
        .unwrap();

        let archived = archive_matching_memory_entries(&memory_dir, &archive_dir, |_slug, body| {
            body.contains("drop")
        })
        .unwrap();

        assert_eq!(archived, vec!["drop"]);
        assert!(memory_dir.join("keep.md").exists());
        assert!(!memory_dir.join("drop.md").exists());
        assert!(archive_dir.join("drop.md").exists());
        let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
        assert!(index.contains("](keep.md)"));
        assert!(!index.contains("](drop.md)"));
    }

    #[cfg(unix)]
    #[test]
    fn archive_removes_index_pointers_after_post_archive_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let archive_dir = memory_dir.join("archive");
        fs::create_dir_all(&memory_dir).unwrap();
        for slug in ["first", "second"] {
            fs::write(memory_dir.join(format!("{slug}.md")), "drop").unwrap();
        }
        fs::write(
            memory_dir.join("MEMORY.md"),
            "# Memory\n- [first](first.md) — first\n- [second](second.md) — second\n",
        )
        .unwrap();
        let mut marker_removals = Vec::new();

        let error = archive_matching_memory_entries_retained(
            &memory_dir,
            &archive_dir,
            |_memory, _slug, _body| true,
            |_memory, slug| {
                marker_removals.push(slug.to_string());
                Err(io::Error::other(format!("marker removal failed for {slug}")))
            },
        )
        .expect_err("post-archive failures must be reported");

        assert_eq!(marker_removals.len(), 2);
        assert!(archive_dir.join("first.md").exists());
        assert!(archive_dir.join("second.md").exists());
        let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
        assert!(!index.contains("](first.md)"));
        assert!(!index.contains("](second.md)"));
        let message = error.to_string();
        assert!(message.contains("marker removal failed for first"), "{message}");
        assert!(message.contains("marker removal failed for second"), "{message}");
    }

    #[test]
    fn archive_skips_unsafe_slug_and_existing_archive_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let archive_dir = tmp.path().join("archive");
        fs::create_dir_all(&memory_dir).unwrap();
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(memory_dir.join("unsafe.slug.md"), "drop").unwrap();
        fs::write(memory_dir.join("safe.md"), "drop").unwrap();
        fs::write(archive_dir.join("safe.md"), "existing").unwrap();

        let archived = archive_matching_memory_entries(&memory_dir, &archive_dir, |_, _| true)
            .unwrap();

        assert!(archived.is_empty());
        assert!(memory_dir.join("unsafe.slug.md").exists());
        assert!(memory_dir.join("safe.md").exists());
        assert_eq!(
            fs::read_to_string(archive_dir.join("safe.md")).unwrap(),
            "existing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn archive_rejects_symlink_and_hardlink_entries() {
        use std::os::unix::fs::{MetadataExt, symlink};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let archive_dir = memory_dir.join("archive");
        let victim = tmp.path().join("victim.md");
        fs::create_dir(&memory_dir).unwrap();
        fs::write(&victim, "outside").unwrap();
        symlink(&victim, memory_dir.join("symlink.md")).unwrap();
        fs::hard_link(&victim, memory_dir.join("hardlink.md")).unwrap();

        let archived = archive_matching_memory_entries(&memory_dir, &archive_dir, |_, _| true)
            .unwrap();

        assert!(archived.is_empty());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "outside");
        assert!(archive_dir.is_dir());
        assert_eq!(fs::metadata(&archive_dir).unwrap().mode() & 0o777, 0o700);
        assert!(fs::read_dir(&archive_dir).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn archive_and_index_do_not_follow_symlinks_or_mutate_hardlink_targets() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let archive_dir = memory_dir.join("archive");
        let outside_archive = tmp.path().join("outside-archive");
        fs::create_dir(&memory_dir).unwrap();
        fs::create_dir(&outside_archive).unwrap();
        fs::write(memory_dir.join("drop.md"), "drop").unwrap();
        symlink(&outside_archive, &archive_dir).unwrap();

        assert!(archive_matching_memory_entries(&memory_dir, &archive_dir, |_, _| true).is_err());
        assert!(memory_dir.join("drop.md").exists());
        assert!(fs::read_dir(&outside_archive).unwrap().next().is_none());

        fs::remove_file(&archive_dir).unwrap();
        let outside_index = tmp.path().join("outside-index.md");
        fs::write(&outside_index, "- [drop](drop.md) — outside\n").unwrap();
        fs::hard_link(&outside_index, memory_dir.join("MEMORY.md")).unwrap();
        let archived = archive_matching_memory_entries(&memory_dir, &archive_dir, |_, _| true)
            .unwrap();

        assert_eq!(archived, vec!["drop"]);
        assert_eq!(
            fs::read_to_string(&outside_index).unwrap(),
            "- [drop](drop.md) — outside\n"
        );
        assert_eq!(
            fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap(),
            "- [drop](drop.md) — outside\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retained_handles_survive_memory_path_replacement() {
        use std::cell::Cell;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let retained_path = tmp.path().join("retained-memory");
        let archive_dir = memory_dir.join("archive");
        fs::create_dir(&memory_dir).unwrap();
        fs::write(memory_dir.join("drop.md"), "drop").unwrap();
        fs::write(
            memory_dir.join("MEMORY.md"),
            "# Memory\n- [drop](drop.md) — drop\n",
        )
        .unwrap();
        let replaced = Cell::new(false);

        let archived = archive_matching_memory_entries(&memory_dir, &archive_dir, |_, _| {
            if !replaced.replace(true) {
                fs::rename(&memory_dir, &retained_path).unwrap();
                fs::create_dir(&memory_dir).unwrap();
                fs::write(memory_dir.join("drop.md"), "attacker replacement").unwrap();
                fs::write(memory_dir.join("MEMORY.md"), "attacker index").unwrap();
            }
            true
        })
        .unwrap();

        assert_eq!(archived, vec!["drop"]);
        assert_eq!(
            fs::read_to_string(retained_path.join("archive/drop.md")).unwrap(),
            "drop"
        );
        assert!(!fs::read_to_string(retained_path.join("MEMORY.md"))
            .unwrap()
            .contains("](drop.md)"));
        assert_eq!(
            fs::read_to_string(memory_dir.join("drop.md")).unwrap(),
            "attacker replacement"
        );
        assert_eq!(
            fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap(),
            "attacker index"
        );
    }

    #[cfg(unix)]
    #[test]
    fn archive_skips_entry_replaced_after_retained_read() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let archive_dir = memory_dir.join("archive");
        fs::create_dir(&memory_dir).unwrap();
        fs::write(memory_dir.join("drop.md"), "original").unwrap();

        let archived = archive_matching_memory_entries(&memory_dir, &archive_dir, |_, _| {
            fs::rename(memory_dir.join("drop.md"), memory_dir.join("original.md")).unwrap();
            fs::write(memory_dir.join("drop.md"), "replacement").unwrap();
            true
        })
        .unwrap();

        assert!(archived.is_empty());
        assert_eq!(
            fs::read_to_string(memory_dir.join("original.md")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(memory_dir.join("drop.md")).unwrap(),
            "replacement"
        );
        assert!(!archive_dir.join("drop.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn archive_directory_and_replacement_files_are_owner_only() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let archive_dir = memory_dir.join("archive");
        fs::create_dir(&memory_dir).unwrap();
        fs::create_dir(&archive_dir).unwrap();
        fs::set_permissions(&archive_dir, fs::Permissions::from_mode(0o777)).unwrap();
        fs::write(memory_dir.join("drop.md"), "drop").unwrap();
        fs::set_permissions(
            memory_dir.join("drop.md"),
            fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        fs::write(
            memory_dir.join("MEMORY.md"),
            "# Memory\n- [drop](drop.md) — drop\n",
        )
        .unwrap();
        fs::set_permissions(
            memory_dir.join("MEMORY.md"),
            fs::Permissions::from_mode(0o666),
        )
        .unwrap();

        let archived = archive_matching_memory_entries(&memory_dir, &archive_dir, |_, _| true)
            .unwrap();

        assert_eq!(archived, vec!["drop"]);
        assert_eq!(fs::metadata(&archive_dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(archive_dir.join("drop.md")).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(memory_dir.join("MEMORY.md"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
    }
}
