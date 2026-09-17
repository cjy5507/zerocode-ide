use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use runtime::{ContentBlock, MessageRole, Session};

use crate::session::{ManagedSessionSummary, SessionHandle};
use crate::{
    format_missing_session_reference, format_no_managed_sessions, format_session_modified_age,
    LEGACY_SESSION_EXTENSION, PRIMARY_SESSION_EXTENSION, SESSION_REFERENCE_ALIASES,
};

/// Where a managed session's JSONL transcript is persisted.
///
/// Interactive REPL sessions are **project-scoped** — they live in Zo's
/// global per-project session store (`~/.zo/projects/<slug>/sessions` by
/// default, via [`runtime::session_control::managed_sessions_dir_path_for`]) so
/// `git status` stays clean while `--resume` and `/resume` still find them per
/// repo. One-shot, non-interactive runs (`zo -p …`, headless slash commands)
/// are **ephemeral** — persisting them into the target repo would pollute
/// `git status` during benchmarks/CI, so they go to an external scratch
/// directory under the OS temp dir instead. Set `ZO_SESSION_ROOT` to pin
/// either scope to an explicit artifact root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionScope {
    /// Persist under Zo's global per-project sessions store (interactive).
    Project,
    /// Persist outside the working tree (non-interactive / headless).
    Ephemeral,
}

/// Resolve the sessions directory for `scope`, rooted at `cwd`. Pure (no IO)
/// so it can be unit-tested without touching the process working directory.
fn sessions_path_for(scope: SessionScope, cwd: &Path) -> PathBuf {
    if let Some(root) = std::env::var_os("ZO_SESSION_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root).join("sessions");
        }
    }
    match scope {
        SessionScope::Project => runtime::session_control::managed_sessions_dir_path_for(cwd),
        SessionScope::Ephemeral => ephemeral_sessions_dir(cwd),
    }
}

/// Global, per-user, per-project sessions directory:
/// `~/.zo/projects/<slug>/sessions`.
///
/// Mirrors Claude Code's storage model: interactive REPL transcripts live in
/// the user's home (`runtime::default_config_home()`, which honors
/// `ZO_CONFIG_HOME`/`ZO_HOME`/`HOME`) rather than inside the working tree,
/// so `git status` stays clean and sessions survive across worktrees. They are
/// still partitioned per workspace by a stable, human-readable slug so
/// `--resume`/`/resume` in one repo never surfaces another repo's history.
#[cfg(test)]
fn global_project_sessions_dir(cwd: &Path) -> PathBuf {
    runtime::session_control::global_project_sessions_dir_for(cwd)
}

/// External (out-of-tree) sessions directory for ephemeral runs, keyed by a
/// stable hash of the workspace so different repos stay isolated.
fn ephemeral_sessions_dir(cwd: &Path) -> PathBuf {
    std::env::temp_dir()
        .join("zo-sessions")
        .join(runtime::sandbox::workspace_scratch_key(cwd))
        .join("sessions")
}

/// External (out-of-tree) directory for the `Workflow` tool's resume cache on
/// ephemeral runs, so a headless `zo -p …` never writes `.zo/workflows/`
/// into the target repo (the same benchmark-pollution rationale as
/// [`ephemeral_sessions_dir`]). Co-located under the workspace-keyed base so
/// per-repo cleanup removes sessions and caches together. The dispatcher points
/// the tool here by setting `ZO_WORKFLOW_STORE` for one-shot runs.
pub(crate) fn ephemeral_workflow_store_dir(cwd: &Path) -> PathBuf {
    std::env::temp_dir()
        .join("zo-sessions")
        .join(runtime::sandbox::workspace_scratch_key(cwd))
        .join("workflows")
}

fn sessions_dir(scope: SessionScope) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    let path = sessions_path_for(scope, &cwd);
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn session_search_dirs() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    let mut dirs = runtime::session_control::managed_session_search_dirs_for(&cwd);
    // Ephemeral one-shot sessions live out-of-tree; include them so
    // `--resume latest` still finds the most recent headless run.
    let ephemeral = ephemeral_sessions_dir(&cwd);
    if !dirs.iter().any(|existing| existing == &ephemeral) {
        dirs.push(ephemeral);
    }
    Ok(dirs)
}

pub(crate) fn create_managed_session_handle(
    session_id: &str,
    scope: SessionScope,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    create_managed_session_handle_at(session_id, scope, &cwd)
}

pub(crate) fn create_managed_session_handle_at(
    session_id: &str,
    scope: SessionScope,
    cwd: &Path,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let id = session_id.to_string();
    let directory = sessions_path_for(scope, cwd);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{id}.{PRIMARY_SESSION_EXTENSION}"));
    Ok(SessionHandle { id, path })
}

/// Project-scoped session transcript files (`.zo/sessions/*.jsonl`), for
/// `zo serve` restart rehydration.
///
/// Deliberately **Project-only** — unlike [`list_managed_sessions`], which
/// sweeps every search dir (legacy + out-of-tree ephemeral) so `--resume latest`
/// can find a headless run. A server must not resurrect one-shot ephemeral
/// sessions into its long-lived pool, so this looks at the interactive
/// `.zo/sessions/` directory alone. Returns an empty vec when it does not
/// exist (a fresh server with no prior sessions).
pub(crate) fn project_session_files() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let dir = sessions_path_for(SessionScope::Project, &crate::current_cli_cwd()?);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if is_managed_session_file(&path) {
            files.push(path);
        }
    }
    Ok(files)
}

pub(crate) fn resolve_session_reference(
    reference: &str,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    if SESSION_REFERENCE_ALIASES
        .iter()
        .any(|alias| reference.eq_ignore_ascii_case(alias))
    {
        let latest = latest_managed_session()?;
        return Ok(SessionHandle {
            id: latest.id,
            path: latest.path,
        });
    }

    let direct = PathBuf::from(reference);
    let looks_like_path = direct.extension().is_some() || direct.components().count() > 1;
    let path = if direct.exists() {
        direct
    } else if looks_like_path {
        return Err(format_missing_session_reference(reference).into());
    } else {
        resolve_managed_session_path(reference)?
    };
    let id = path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|name| {
            name.strip_suffix(&format!(".{PRIMARY_SESSION_EXTENSION}"))
                .or_else(|| name.strip_suffix(&format!(".{LEGACY_SESSION_EXTENSION}")))
        })
        .unwrap_or(reference)
        .to_string();
    Ok(SessionHandle { id, path })
}

fn resolve_managed_session_path(session_id: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    for directory in session_search_dirs()? {
        for extension in [PRIMARY_SESSION_EXTENSION, LEGACY_SESSION_EXTENSION] {
            let path = directory.join(format!("{session_id}.{extension}"));
            if path.exists() {
                return Ok(path);
            }
        }
    }
    Err(format_missing_session_reference(session_id).into())
}

// The canonical sidecar filter lives in `runtime::session_control` so serve
// rehydration, session listing, and `resume latest` all exclude the same
// non-transcript companions (`.vault.jsonl`, `.rot-<ts>.jsonl`, `.todos.json`,
// `.prefs.json`). Re-exported here rather than duplicated so the two paths can
// never drift apart.
use runtime::session_control::is_managed_session_file;

pub(crate) fn list_managed_sessions(
) -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    list_managed_sessions_limited(None)
}

pub(crate) fn managed_session_paths_limited(
    limit: Option<usize>,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut candidates = recent_session_candidates(limit)?;
    Ok(candidates.drain(..).map(|(path, _)| path).collect())
}

fn recent_session_candidates(
    limit: Option<usize>,
) -> Result<Vec<(PathBuf, u128)>, Box<dyn std::error::Error>> {
    // Cheap enumeration: collect (path, mtime) using metadata only.
    let mut candidates: Vec<(PathBuf, u128)> = Vec::new();
    for directory in session_search_dirs()? {
        if !directory.exists() {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if !is_managed_session_file(&path) {
                continue;
            }
            let modified_epoch_millis = entry
                .metadata()?
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or_default();
            candidates.push((path, modified_epoch_millis));
        }
    }
    // Most-recent first; the path tiebreak keeps top-N selection deterministic
    // when modification times collide.
    candidates.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
    if let Some(limit) = limit {
        candidates.truncate(limit);
    }
    Ok(candidates)
}

/// Like [`list_managed_sessions`] but, when `limit` is `Some(n)`, parses only
/// the `n` most-recently-modified session files.
///
/// The modification time comes from a cheap `stat` (no file contents read), so
/// the expensive [`Session::load_from_path`] JSONL parse runs for at most `n`
/// files. This is the key to a fast cold start when the registry holds many
/// sessions: the launchpad and `/resume` picker only need a handful, yet the
/// unbounded variant used to parse *every* session on disk.
pub(crate) fn list_managed_sessions_limited(
    limit: Option<usize>,
) -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    let candidates = recent_session_candidates(limit)?;

    // Phase 2 — parse only the retained candidates.
    let mut sessions = Vec::with_capacity(candidates.len());
    for (path, modified_epoch_millis) in candidates {
        let (id, name, message_count, parent_session_id, branch_name, first_user_text) =
            match Session::load_from_path(&path) {
                Ok(session) => {
                    let parent_session_id = session
                        .fork
                        .as_ref()
                        .map(|fork| fork.parent_session_id.clone());
                    let branch_name = session
                        .fork
                        .as_ref()
                        .and_then(|fork| fork.branch_name.clone());
                    let first_user_text = session
                        .messages
                        .iter()
                        .find(|m| m.role == MessageRole::User)
                        .and_then(|m| {
                            m.blocks.iter().find_map(|b| match b {
                                ContentBlock::Text { text } => {
                                    let trimmed: String = text.chars().take(60).collect();
                                    if trimmed.is_empty() {
                                        None
                                    } else {
                                        Some(trimmed)
                                    }
                                }
                                _ => None,
                            })
                        });
                    (
                        session.session_id,
                        session.name,
                        session.messages.len(),
                        parent_session_id,
                        branch_name,
                        first_user_text,
                    )
                }
                Err(_) => (
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    None,
                    0,
                    None,
                    None,
                    None,
                ),
            };
        sessions.push(ManagedSessionSummary {
            id,
            name,
            path,
            modified_epoch_millis,
            message_count,
            parent_session_id,
            branch_name,
            first_user_text,
        });
    }
    sessions.sort_by(|left, right| {
        right
            .modified_epoch_millis
            .cmp(&left.modified_epoch_millis)
            .then_with(|| right.id.cmp(&left.id))
    });
    sessions.dedup_by(|left, right| left.id == right.id);
    Ok(sessions)
}

fn latest_managed_session() -> Result<ManagedSessionSummary, Box<dyn std::error::Error>> {
    list_managed_sessions_limited(Some(1))?
        .into_iter()
        .next()
        .ok_or_else(|| format_no_managed_sessions().into())
}

pub(crate) fn render_session_list(
    active_session_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let sessions = list_managed_sessions()?;
    let mut lines = vec![
        "Sessions".to_string(),
        format!(
            "  Directory         {}",
            sessions_dir(SessionScope::Project)?.display()
        ),
    ];
    if sessions.is_empty() {
        lines.push("  No managed sessions saved yet.".to_string());
        return Ok(lines.join("\n"));
    }
    for session in sessions {
        let marker = if session.id == active_session_id {
            "● current"
        } else {
            "○ saved"
        };
        let name = session
            .name
            .as_deref()
            .map_or_else(String::new, |name| format!(" ● {name}"));
        let lineage = match (
            session.branch_name.as_deref(),
            session.parent_session_id.as_deref(),
        ) {
            (Some(branch_name), Some(parent_session_id)) => {
                format!(" branch={branch_name} from={parent_session_id}")
            }
            (None, Some(parent_session_id)) => format!(" from={parent_session_id}"),
            (Some(branch_name), None) => format!(" branch={branch_name}"),
            (None, None) => String::new(),
        };
        lines.push(format!(
            "  {id:<20} {marker:<10}{name} msgs={msgs:<4} modified={modified}{lineage} path={path}",
            id = session.id,
            name = name,
            msgs = session.message_count,
            modified = format_session_modified_age(session.modified_epoch_millis),
            lineage = lineage,
            path = session.path.display(),
        ));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn write_session_clear_backup(
    session: &Session,
    session_path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let backup_path = session_clear_backup_path(session_path);
    session.save_to_path(&backup_path)?;
    Ok(backup_path)
}

fn session_clear_backup_path(session_path: &Path) -> PathBuf {
    let timestamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| duration.as_millis());
    let file_name = session_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session.jsonl");
    session_path.with_file_name(format!("{file_name}.before-clear-{timestamp}.bak"))
}

fn default_export_filename(session: &Session) -> String {
    let stem = session
        .messages
        .iter()
        .find_map(|message| match message.role {
            MessageRole::User => message.blocks.iter().find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }),
            _ => None,
        })
        .map_or("conversation", |text| {
            text.lines().next().unwrap_or("conversation")
        })
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let fallback = if stem.is_empty() {
        "conversation"
    } else {
        &stem
    };
    format!("{fallback}.txt")
}

pub(crate) fn resolve_export_path(
    requested_path: Option<&str>,
    session: &Session,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let file_name =
        requested_path.map_or_else(|| default_export_filename(session), ToOwned::to_owned);
    let final_name = if Path::new(&file_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
    {
        file_name
    } else {
        format!("{file_name}.txt")
    };
    Ok(cwd.join(final_name))
}

static ATOMIC_WRITE_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    // Crash/power-loss durability has no deterministic in-process reproduction,
    // so a red-first test is impractical for the fsync effect itself.
    // Plain `fs::write` followed a leaf symlink and updated its target;
    // renaming a sibling temp over the link would instead replace the link
    // itself. Resolve the leaf chain so replacement lands on the real
    // destination.
    let destination = resolve_leaf_symlinks(path)?;
    let (temp_path, mut temp_file) = create_atomic_temp_file(&destination)?;
    // The fresh temp file is umask-default; carry over an existing
    // destination's permissions so replacement does not downgrade e.g. a 0600
    // file to 0644. A missing destination keeps the default (plain create).
    let write_result = match fs::metadata(&destination) {
        Ok(metadata) => temp_file.set_permissions(metadata.permissions()),
        // Only a missing destination (plain create) keeps the umask default;
        // any other stat failure must not silently drop the original's mode.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .and_then(|()| temp_file.write_all(bytes))
    .and_then(|()| temp_file.flush())
    .and_then(|()| temp_file.sync_all());
    drop(temp_file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    // `std::fs::rename` replaces an existing destination file on Unix
    // (`rename(2)`) and Windows (`MoveFileExW` + `MOVEFILE_REPLACE_EXISTING`),
    // the same guarantee the plugin-registry and session-store helpers rely on.
    match fs::rename(&temp_path, &destination) {
        Ok(()) => {
            #[cfg(unix)]
            {
                let parent = destination
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                // The rename above published the new inode; fsync the directory
                // so the rename survives a crash. Best-effort: some filesystems
                // reject directory fsync, and the file data is already durable.
                let _ = fs::File::open(parent).and_then(|dir| dir.sync_all());
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(temp_path);
            Err(error)
        }
    }
}

/// Follow a leaf-symlink chain (bounded like Linux `MAXSYMLINKS`) to the file
/// replacement actually targets. Mirrors the leaf-following half of
/// `tools`' `resolve_for_boundary_check`; parent directories resolve through
/// the OS during the rename itself. A cycle (or an unreadable/absurdly long
/// chain) refuses with the `ELOOP`-style error plain `fs::write` produced,
/// instead of renaming the temp file over the link itself.
fn resolve_leaf_symlinks(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match current.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&current)?;
                if target.is_absolute() {
                    current = target;
                } else {
                    let base = current.parent().unwrap_or_else(|| Path::new(""));
                    current = base.join(target);
                }
            }
            Ok(_) => return Ok(current),
            // A missing leaf is the plain-create case; any other lstat failure
            // (e.g. an unsearchable parent) must propagate — proceeding could
            // rename over a path we could not prove is not a symlink.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
        }
    }
    // The hop budget is spent. A chain of exactly 40 links that settled on a
    // non-symlink is within the budget (Linux errors on the 41st hop, not the
    // 40th); refuse only when the path is STILL a symlink — a cycle or an
    // over-budget chain. `ErrorKind::FilesystemLoop` is still unstable
    // (`io_error_more`), so the ELOOP meaning travels in the message.
    match current.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => Err(std::io::Error::other(format!(
            "too many levels of symbolic links resolving {}",
            path.display()
        ))),
        Ok(_) => Ok(current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(current),
        Err(error) => Err(error),
    }
}

fn create_atomic_temp_file(path: &Path) -> std::io::Result<(PathBuf, fs::File)> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("output");
    for _ in 0..128 {
        let counter =
            ATOMIC_WRITE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{name}.tmp-{}-{counter}",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("could not allocate a temporary file for {}", path.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        ephemeral_sessions_dir, ephemeral_workflow_store_dir, global_project_sessions_dir,
        project_session_files, sessions_path_for, write_atomic, SessionScope,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "zo-{label}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create temporary directory");
        path
    }

    #[test]
    fn atomic_write_replaces_existing_destination() {
        let dir = temp_dir("atomic-success");
        let destination = dir.join("export.txt");
        std::fs::write(&destination, b"old export").expect("seed export");

        write_atomic(&destination, b"new export").expect("replace export atomically");

        assert_eq!(
            std::fs::read(&destination).expect("read replaced export"),
            b"new export"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_destination_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("atomic-mode");
        let destination = dir.join("export.txt");
        std::fs::write(&destination, b"old export").expect("seed export");
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600))
            .expect("restrict export mode");

        write_atomic(&destination, b"new export").expect("replace export atomically");

        let mode = std::fs::metadata(&destination)
            .expect("stat replaced export")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "replacement must not downgrade the file mode");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_leaf_symlink() {
        let dir = temp_dir("atomic-symlink");
        let target = dir.join("real-export.txt");
        std::fs::write(&target, b"old export").expect("seed symlink target");
        let link = dir.join("export-link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("create leaf symlink");

        write_atomic(&link, b"new export").expect("replace through symlink");

        assert!(
            link.symlink_metadata()
                .expect("lstat destination")
                .file_type()
                .is_symlink(),
            "the destination symlink must survive replacement"
        );
        assert_eq!(
            std::fs::read(&target).expect("read symlink target"),
            b"new export",
            "the write must land on the symlink's target"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_follows_exactly_forty_symlink_hops() {
        let dir = temp_dir("atomic-forty");
        let real = dir.join("real-export.txt");
        std::fs::write(&real, b"old export").expect("seed chain target");
        let mut previous = real.clone();
        for i in 1..=40 {
            let link = dir.join(format!("link-{i}"));
            std::os::unix::fs::symlink(&previous, &link).expect("create chain link");
            previous = link;
        }
        let head = previous;

        write_atomic(&head, b"new export").expect("a 40-hop chain is within the budget");

        assert_eq!(
            std::fs::read(&real).expect("read chain target"),
            b"new export",
            "the write must land on the chain's final target"
        );
        assert!(
            head.symlink_metadata()
                .expect("lstat chain head")
                .file_type()
                .is_symlink(),
            "the chain head must remain a symlink"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_resolver_propagates_unreadable_parent_errors() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("resolver-unreadable");
        let target = dir.join("export.txt");
        std::fs::write(&target, b"export").expect("seed target");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))
            .expect("lock parent directory");

        // Root ignores permission bits: probe and skip instead of failing.
        if target.symlink_metadata().is_ok() {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore parent permissions");
            let _ = std::fs::remove_dir_all(dir);
            return;
        }

        let result = super::resolve_leaf_symlinks(&target);

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore parent permissions");
        let _ = std::fs::remove_dir_all(dir);
        assert!(
            result.is_err(),
            "an unreadable parent must propagate its error, not fail open as a regular file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_refuses_symlink_cycle() {
        let dir = temp_dir("atomic-cycle");
        let a = dir.join("a-link");
        let b = dir.join("b-link");
        std::os::unix::fs::symlink(&b, &a).expect("create a->b");
        std::os::unix::fs::symlink(&a, &b).expect("create b->a");

        let result = write_atomic(&a, b"new export");

        assert!(result.is_err(), "a symlink cycle must refuse replacement");
        assert!(
            a.symlink_metadata()
                .expect("lstat cyclic link")
                .file_type()
                .is_symlink(),
            "the cyclic link must be left intact, not renamed over"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_failure_preserves_existing_destination() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("atomic-failure");
        let destination = dir.join("export.txt");
        std::fs::write(&destination, b"old export").expect("seed export");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
            .expect("make export directory read-only");

        let probe = dir.join("probe");
        if std::fs::write(&probe, b"probe").is_ok() {
            let _ = std::fs::remove_file(probe);
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore export directory permissions");
            let _ = std::fs::remove_dir_all(dir);
            return;
        }

        let result = write_atomic(&destination, b"new export");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore export directory permissions");
        assert!(result.is_err(), "creating the sibling temp file must fail");
        assert_eq!(
            std::fs::read(&destination).expect("read export after failed replacement"),
            b"old export",
            "failed replacement must leave the previous export intact"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `zo serve` rehydration enumerates only the in-tree `.zo/sessions/`
    /// `*.jsonl` transcripts — never the ephemeral one-shot dir, never
    /// non-session files. (`ZO_SESSION_ROOT` overrides the Project dir, so
    /// this exercises the real path without touching the working tree.)
    #[test]
    fn project_session_files_lists_only_project_jsonl() {
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!(
            "zo-f1-psf-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let dir = root.join("sessions");
        std::fs::create_dir_all(&dir).expect("sessions dir");
        std::fs::write(dir.join("s1.jsonl"), "{}\n").expect("session file");
        std::fs::write(dir.join("s1.prefs.json"), "{}\n").expect("prefs file");
        std::fs::write(dir.join("s1.vault.jsonl"), "{}\n").expect("vault companion");
        std::fs::write(dir.join("notes.txt"), "ignore me").expect("noise file");

        std::env::set_var("ZO_SESSION_ROOT", &root);
        let files = project_session_files().expect("enumerate");
        std::env::remove_var("ZO_SESSION_ROOT");
        let _ = std::fs::remove_dir_all(&root);

        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        assert!(names.contains(&"s1.jsonl".to_string()), "got {names:?}");
        assert!(
            !names.contains(&"s1.vault.jsonl".to_string()),
            "vault companions are not sessions: {names:?}"
        );
        assert!(
            files
                .iter()
                .all(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")),
            "only .jsonl session files may be listed: {names:?}"
        );
    }

    #[test]
    fn empty_session_root_falls_back_to_scoped_zo_paths() {
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_root = std::env::var_os("ZO_SESSION_ROOT");
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let home = std::env::temp_dir().join(format!("zo-empty-root-{}", std::process::id()));
        std::env::set_var("ZO_SESSION_ROOT", "");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let cwd = Path::new("/some/empty-override/repo");
        let project = sessions_path_for(SessionScope::Project, cwd);
        let ephemeral = sessions_path_for(SessionScope::Ephemeral, cwd);

        match prior_root {
            Some(value) => std::env::set_var("ZO_SESSION_ROOT", value),
            None => std::env::remove_var("ZO_SESSION_ROOT"),
        }
        match prior_config_home {
            Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
            None => std::env::remove_var("ZO_CONFIG_HOME"),
        }

        assert!(project.starts_with(&home), "project path: {project:?}");
        assert_ne!(project, Path::new("sessions"));
        assert!(ephemeral.starts_with(std::env::temp_dir()));
        assert_ne!(ephemeral, Path::new("sessions"));
    }

    /// Regression guard for the global-storage migration: interactive
    /// (project) sessions now live in the user's home under
    /// `~/.zo/projects/<slug>/sessions` — NOT inside the working tree — so
    /// `git status` stays clean. Ephemeral one-shot sessions also stay
    /// out-of-tree (the original benchmark-pollution guard).
    #[test]
    fn project_and_ephemeral_sessions_persist_outside_the_working_tree() {
        // `sessions_path_for` honors `ZO_SESSION_ROOT`/`ZO_CONFIG_HOME`;
        // share the env lock with the rehydration test that sets it.
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Pin a deterministic global home so the assertions don't depend on the
        // developer's real `~/.zo`. `ZO_SESSION_ROOT` would short-circuit
        // `sessions_path_for`, so make sure it is unset for this case.
        let prior_root = std::env::var_os("ZO_SESSION_ROOT");
        std::env::remove_var("ZO_SESSION_ROOT");
        let home = std::env::temp_dir().join(format!("zo-global-home-{}", std::process::id()));
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let cwd = Path::new("/some/benchmark/repo");
        let project = sessions_path_for(SessionScope::Project, cwd);
        let ephemeral = sessions_path_for(SessionScope::Ephemeral, cwd);

        if let Some(value) = prior_config_home {
            std::env::set_var("ZO_CONFIG_HOME", value);
        } else {
            std::env::remove_var("ZO_CONFIG_HOME");
        }
        if let Some(value) = prior_root {
            std::env::set_var("ZO_SESSION_ROOT", value);
        }

        assert!(
            !project.starts_with(cwd),
            "project sessions must NOT live inside the working tree: {project:?}"
        );
        assert!(
            project.starts_with(&home),
            "project sessions live under the global config home: {project:?}"
        );
        assert!(project.ends_with("sessions"));

        assert!(
            !ephemeral.starts_with(cwd),
            "ephemeral sessions must NOT live inside the working tree: {ephemeral:?}"
        );
        assert!(ephemeral.ends_with("sessions"));
    }

    /// Distinct workspaces map to distinct, stable global session homes, and the
    /// slug carries a readable stem so `~/.zo/projects/` stays browsable.
    #[test]
    fn global_project_sessions_are_repo_distinct_and_readable() {
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_root = std::env::var_os("ZO_SESSION_ROOT");
        std::env::remove_var("ZO_SESSION_ROOT");
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let home = std::env::temp_dir().join(format!("zo-gph-{}", std::process::id()));
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let a1 = global_project_sessions_dir(Path::new("/Users/dev/work/zo"));
        let a2 = global_project_sessions_dir(Path::new("/Users/dev/work/zo"));
        let b = global_project_sessions_dir(Path::new("/Users/dev/work/other-repo"));

        if let Some(value) = prior_config_home {
            std::env::set_var("ZO_CONFIG_HOME", value);
        } else {
            std::env::remove_var("ZO_CONFIG_HOME");
        }
        if let Some(value) = prior_root {
            std::env::set_var("ZO_SESSION_ROOT", value);
        }

        assert_eq!(a1, a2, "same repo maps to a stable global sessions dir");
        assert_ne!(a1, b, "different repos get distinct global sessions dirs");
        let slug = a1
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .expect("slug component");
        assert!(
            slug.contains("zo"),
            "slug keeps a readable stem: {slug}"
        );
    }

    #[test]
    fn ephemeral_sessions_persist_outside_the_working_tree() {
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cwd = Path::new("/some/benchmark/repo");
        let ephemeral = sessions_path_for(SessionScope::Ephemeral, cwd);
        assert!(!ephemeral.starts_with(cwd));
    }

    #[test]
    fn ephemeral_sessions_are_repo_distinct_and_stable() {
        let a1 = ephemeral_sessions_dir(Path::new("/repo/a"));
        let a2 = ephemeral_sessions_dir(Path::new("/repo/a"));
        let b = ephemeral_sessions_dir(Path::new("/repo/b"));
        assert_eq!(a1, a2, "same repo maps to a stable ephemeral dir");
        assert_ne!(a1, b, "different repos get distinct ephemeral dirs");
    }

    #[test]
    fn ephemeral_workflow_store_is_out_of_tree_and_repo_distinct() {
        let repo = Path::new("/some/benchmark/repo");
        let cache = ephemeral_workflow_store_dir(repo);
        assert!(
            !cache.starts_with(repo),
            "the workflow cache must NOT live inside the working tree: {cache:?}"
        );
        assert!(cache.ends_with("workflows"));
        assert_ne!(
            cache,
            ephemeral_workflow_store_dir(Path::new("/some/other/repo")),
            "different repos get distinct workflow caches"
        );
    }

    /// Cold-start guard: `list_managed_sessions_limited(Some(n))` truncates the
    /// candidate set *before* the expensive [`Session::load_from_path`] parse, so
    /// at most `n` sessions are ever read from disk — the whole point of the cap
    /// the `/resume` picker and launchpad rely on. The cap holds regardless of
    /// how many other sessions exist in the search dirs (only truncation, then a
    /// dedup that can shrink but never grow the result, runs after).
    #[test]
    fn limited_listing_caps_the_number_of_sessions_parsed() {
        use super::list_managed_sessions_limited;

        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Isolate from the developer's real `~/.zo` (global dir) and pin an
        // explicit, empty session root we fully control.
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let prior_root = std::env::var_os("ZO_SESSION_ROOT");
        let home = std::env::temp_dir().join(format!("zo-lmls-home-{}", std::process::id()));
        let root = std::env::temp_dir().join(format!("zo-lmls-root-{}", std::process::id()));
        let dir = root.join("sessions");
        std::fs::create_dir_all(&dir).expect("session dir");
        // Five managed session files; an Err parse still yields a summary, so
        // even minimal `{}` content counts toward the candidate set.
        for index in 0..5 {
            std::fs::write(dir.join(format!("s{index}.jsonl")), "{}\n").expect("session file");
        }
        std::env::set_var("ZO_CONFIG_HOME", &home);
        std::env::set_var("ZO_SESSION_ROOT", &root);

        let capped = list_managed_sessions_limited(Some(2)).expect("limited list");
        let none = list_managed_sessions_limited(Some(0)).expect("zero list");
        let all = list_managed_sessions_limited(None).expect("full list");

        if let Some(value) = prior_config_home {
            std::env::set_var("ZO_CONFIG_HOME", value);
        } else {
            std::env::remove_var("ZO_CONFIG_HOME");
        }
        if let Some(value) = prior_root {
            std::env::set_var("ZO_SESSION_ROOT", value);
        } else {
            std::env::remove_var("ZO_SESSION_ROOT");
        }
        let _ = std::fs::remove_dir_all(&root);

        assert!(
            capped.len() <= 2,
            "Some(2) must parse at most 2 sessions, got {}",
            capped.len()
        );
        assert!(none.is_empty(), "Some(0) reads nothing");
        assert!(
            all.len() >= 5,
            "None must surface every session on disk, got {}",
            all.len()
        );
    }
}
