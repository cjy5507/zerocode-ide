use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use runtime::session::TranscriptHead;

use crate::session::{ManagedSessionSummary, SessionHandle};
use crate::{
    format_missing_session_reference, format_no_managed_sessions,
    LEGACY_SESSION_EXTENSION, PRIMARY_SESSION_EXTENSION, SESSION_REFERENCE_ALIASES,
};
/// 세션 저장 위치. 대화형이든 `zo -p` 든 zo 는 늘 **프로젝트 스토어**에 쓴다
/// (`~/.zo/projects/<slug>/sessions`). `ZO_SESSION_ROOT` 로 명시 루트에 고정할 수 있다.
///
/// 순수 함수(IO 없음)라 프로세스 작업 디렉터리를 건드리지 않고 단위 테스트할 수 있다.
fn sessions_path_for(cwd: &Path) -> PathBuf {
    if let Some(root) = std::env::var_os("ZO_SESSION_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root).join("sessions");
        }
    }
    runtime::session_control::managed_sessions_dir_path_for(cwd)
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

fn session_search_dirs() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let cwd = crate::current_cli_cwd()?;
    let dirs = runtime::session_control::managed_session_search_dirs_for(&cwd);
    Ok(dirs)
}

pub(crate) fn create_managed_session_handle_at(
    session_id: &str,
    cwd: &Path,
) -> Result<SessionHandle, Box<dyn std::error::Error>> {
    let id = session_id.to_string();
    let directory = sessions_path_for(cwd);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{id}.{PRIMARY_SESSION_EXTENSION}"));
    Ok(SessionHandle { id, path })
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
        named_session_path(reference)?.ok_or_else(|| format_missing_session_reference(reference))?
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
    named_session_path(session_id)?.ok_or_else(|| format_missing_session_reference(session_id).into())
}

fn named_session_path(name: &str) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let rows = search_session_summaries_inner(name, 0, 2, None, &cancelled, true)?;
    match rows.as_slice() {
        [] => Ok(None), [row] => Ok(Some(row.path.clone())),
        _ => Err(format!("session name '{name}' is ambiguous; use an id").into()),
    }
}

// The canonical sidecar filter lives in `runtime::session_control` so serve
// rehydration, session listing, and `resume latest` all exclude the same
// non-transcript companions (`.vault.jsonl`, `.rot-<ts>.jsonl`, `.todos.json`,
// `.prefs.json`). Re-exported here rather than duplicated so the two paths can
// never drift apart.
use runtime::session_control::is_managed_session_file;

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

/// Fill a page with actual conversations. Empty or unreadable heads do not
/// spend a row, and an image prompt is independent of its text preview.
/// Only transcript heads are read, stopping once enough usable rows exist.
pub(crate) fn list_managed_sessions_limited(
    limit: Option<usize>,
) -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    if limit == Some(0) {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (path, modified) in recent_session_candidates(None)? {
        let session = session_summary(path, modified);
        if !session.has_user_input || session.id.ends_with(".system-prompt") || !seen.insert(session.id.clone()) {
            continue;
        }
        sessions.push(session);
        if limit == Some(sessions.len()) {
            break;
        }
    }
    order_session_summaries(&mut sessions);
    Ok(sessions)
}

/// Search eligible heads once and retain only the requested result page prefix.
/// The ordinary recent-session view keeps its separate cheap metadata-first path.
pub(crate) fn search_session_summaries(
    query: &str, offset: usize, limit: usize, cwd_filter: Option<&Path>, cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    search_session_summaries_inner(query, offset, limit, cwd_filter, cancelled, false)
}

fn search_session_summaries_inner(
    query: &str, offset: usize, limit: usize, cwd_filter: Option<&Path>, cancelled: &std::sync::atomic::AtomicBool, exact_name: bool,
) -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    use std::collections::BTreeMap;
    use std::sync::atomic::Ordering;
    let query = query.to_lowercase();
    let capacity = offset.saturating_add(limit);
    if limit == 0 { return Ok(Vec::new()); }
    // Keep only the requested newest matching rows. Every directory entry and
    // transcript head is visited once, including old and no-match queries.
    let mut newest: BTreeMap<(u128, String, PathBuf), ManagedSessionSummary> = BTreeMap::new();
    for directory in session_search_dirs()? {
        if !directory.exists() { continue; }
        for entry in fs::read_dir(directory)? {
            if cancelled.load(Ordering::Relaxed) { return Ok(Vec::new()); }
            let entry = entry?;
            let path = entry.path();
            if !is_managed_session_file(&path) { continue; }
            let modified = entry.metadata()?.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0, |time| time.as_millis());
            let row = session_summary(path, modified);
            if !row.has_user_input || row.id.ends_with(".system-prompt") { continue; }
            let cwd = crate::resume::load_session_cwd(&row.path);
            if cwd_filter.is_some_and(|filter| cwd.as_deref() != Some(filter)) { continue; }
            if exact_name && row.name.as_deref().map(str::to_lowercase).as_deref() != Some(query.as_str()) { continue; }
            if !exact_name && !query.is_empty() && !row.id.to_lowercase().contains(&query)
                && !row.name.as_deref().is_some_and(|name| name.to_lowercase().contains(&query))
                && !row.first_user_text.as_deref().is_some_and(|text| text.to_lowercase().contains(&query))
                && !cwd.as_ref().is_some_and(|path| path.to_string_lossy().to_lowercase().contains(&query)) { continue; }
            let key = (modified, row.id.clone(), row.path.clone());
            if let Some(previous) = newest.iter().find(|(_, found)| found.id == row.id).map(|(key, _)| key.clone()) {
                if previous >= key { continue; }
                newest.remove(&previous);
            }
            if newest.len() < capacity || newest.first_key_value().is_some_and(|(oldest, _)| key > *oldest) {
                newest.insert(key, row);
                if newest.len() > capacity { let _ = newest.pop_first(); }
            }
        }
    }
    Ok(newest.into_values().rev().skip(offset).take(limit).collect())
}


fn session_summary(path: PathBuf, modified_epoch_millis: u128) -> ManagedSessionSummary {
    let (id, name, parent_session_id, branch_name, first_user_text, has_user_input, created_epoch_millis) =
        match TranscriptHead::read(
            &path,
            Some(crate::resume::limits::FIRST_PROMPT_PREVIEW_CHARS),
        ) {
            Ok(head) => {
                let parent_session_id = head
                    .fork
                    .as_ref()
                    .map(|fork| fork.parent_session_id.clone());
                let branch_name = head.fork.and_then(|fork| fork.branch_name);
                (
                    head.session_id,
                    head.name,
                    parent_session_id,
                    branch_name,
                    head.first_user_text,
                    head.has_user_input == Some(true),
                    u128::from(head.created_at_ms),
                )
            }
            // 파일을 못 읽으면 생성 시각도 없다. 수정 시각으로 떨어뜨린다 —
            // `/resume` 의 `Sort: [Created]` 가 그 행만 0 으로 밀어 목록
            // 맨 아래에 처박는 것보다, 그 행이 아는 유일한 시각으로 서는
            // 편이 사람에게 맞다.
            Err(_) => (
                path.file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
                None,
                None,
                None,
                None,
                false,
                modified_epoch_millis,
            ),
        };
    ManagedSessionSummary {
        id,
        name,
        path,
        modified_epoch_millis,
        created_epoch_millis,
        parent_session_id,
        branch_name,
        first_user_text,
        has_user_input,
    }
}

fn order_session_summaries(sessions: &mut [ManagedSessionSummary]) {
    sessions.sort_by(|left, right| {
        right
            .modified_epoch_millis
            .cmp(&left.modified_epoch_millis)
            .then_with(|| right.id.cmp(&left.id))
    });
}

fn latest_managed_session() -> Result<ManagedSessionSummary, Box<dyn std::error::Error>> {
    list_managed_sessions_limited(Some(1))?
        .into_iter()
        .next()
        .ok_or_else(|| format_no_managed_sessions().into())
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
        global_project_sessions_dir, sessions_path_for, write_atomic,
    };
    use std::path::Path;


    #[test]
    fn atomic_write_replaces_existing_destination() {
        let dir = crate::support::temp_dir("atomic-success");
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

        let dir = crate::support::temp_dir("atomic-mode");
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
        let dir = crate::support::temp_dir("atomic-symlink");
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
        let dir = crate::support::temp_dir("atomic-forty");
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

        let dir = crate::support::temp_dir("resolver-unreadable");
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
        let dir = crate::support::temp_dir("atomic-cycle");
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

        let dir = crate::support::temp_dir("atomic-failure");
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
    fn empty_session_root_falls_back_to_scoped_zo_paths() {
        let _guard = crate::test_env_lock();
        let prior_root = std::env::var_os("ZO_SESSION_ROOT");
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let home = std::env::temp_dir().join(format!("zo-empty-root-{}", std::process::id()));
        std::env::set_var("ZO_SESSION_ROOT", "");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let cwd = Path::new("/some/empty-override/repo");
        let project = sessions_path_for(cwd);

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
    }

    /// Regression guard for the global-storage migration: interactive
    /// (project) sessions now live in the user's home under
    /// `~/.zo/projects/<slug>/sessions` — NOT inside the working tree — so
    /// `git status` stays clean.
    #[test]
    fn project_sessions_persist_outside_the_working_tree() {
        // `sessions_path_for` honors `ZO_SESSION_ROOT`/`ZO_CONFIG_HOME`;
        // share the env lock with the rehydration test that sets it.
        let _guard = crate::test_env_lock();
        // Pin a deterministic global home so the assertions don't depend on the
        // developer's real `~/.zo`. `ZO_SESSION_ROOT` would short-circuit
        // `sessions_path_for`, so make sure it is unset for this case.
        let prior_root = std::env::var_os("ZO_SESSION_ROOT");
        std::env::remove_var("ZO_SESSION_ROOT");
        let home = std::env::temp_dir().join(format!("zo-global-home-{}", std::process::id()));
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let cwd = Path::new("/some/benchmark/repo");
        let project = sessions_path_for(cwd);

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
    }

    /// Distinct workspaces map to distinct, stable global session homes, and the
    /// slug carries a readable stem so `~/.zo/projects/` stays browsable.
    #[test]
    fn global_project_sessions_are_repo_distinct_and_readable() {
        let _guard = crate::test_env_lock();
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


    /// The page limit counts usable conversations. A bound of zero reads no
    /// heads, and empty or damaged files do not consume the returned page.
    #[test]
    fn limited_listing_fills_the_page_with_conversations() {
        use super::list_managed_sessions_limited;

        let _guard = crate::test_env_lock();
        // Isolate from the developer's real `~/.zo` (global dir) and pin an
        // explicit, empty session root we fully control.
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let prior_root = std::env::var_os("ZO_SESSION_ROOT");
        let home = std::env::temp_dir().join(format!("zo-lmls-home-{}", std::process::id()));
        let root = std::env::temp_dir().join(format!("zo-lmls-root-{}", std::process::id()));
        let dir = root.join("sessions");
        std::fs::create_dir_all(&dir).expect("session dir");
        for index in 0..5 {
            let session = serde_json::json!({"version":1, "session_id":format!("s{index}"),
                "messages":[{"role":"user", "blocks":[{"type":"text", "text":"A real task"}]}]});
            std::fs::write(dir.join(format!("s{index}.json")), session.to_string()).expect("session file");
        }
        for index in 0..25 {
            std::fs::write(dir.join(format!("empty{index}.jsonl")), "{}\n").expect("unusable head");
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

        assert_eq!(capped.len(), 2, "the usable page must be filled");
        assert!(none.is_empty(), "Some(0) reads nothing");
        assert_eq!(all.len(), 5, "only the conversations belong in the list");
    }

    /// l80a (m-77613): each of these points `ZO_CONFIG_HOME` elsewhere for a
    /// moment. Holding only their own lock, one could land inside another
    /// test's no-home trust save and read-back, which then read back `{}`.
    #[test]
    fn tests_that_move_the_config_home_wait_for_the_env_lock() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let movers: [(&'static str, fn()); 4] = [
            (
                "empty_session_root_falls_back_to_scoped_zo_paths",
                empty_session_root_falls_back_to_scoped_zo_paths,
            ),
            (
                "project_sessions_persist_outside_the_working_tree",
                project_sessions_persist_outside_the_working_tree,
            ),
            (
                "global_project_sessions_are_repo_distinct_and_readable",
                global_project_sessions_are_repo_distinct_and_readable,
            ),
            (
                "limited_listing_fills_the_page_with_conversations",
                limited_listing_fills_the_page_with_conversations,
            ),
        ];
        let (report_done, done) = mpsc::channel();
        let held = crate::test_env_lock();
        let threads: Vec<_> = movers
            .into_iter()
            .map(|(name, mover)| {
                let report_done = report_done.clone();
                let thread = std::thread::spawn(move || {
                    mover();
                    let _ = report_done.send(name);
                });
                (name, thread)
            })
            .collect();
        drop(report_done);
        // A mover that does not wait for the env lock is done long before this.
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut ran_under_the_lock = Vec::new();
        while let Ok(name) = done.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            ran_under_the_lock.push(name);
        }
        drop(held);
        for (name, thread) in threads {
            thread.join().unwrap_or_else(|_| panic!("{name} failed"));
        }
        ran_under_the_lock.sort_unstable();
        assert!(
            ran_under_the_lock.is_empty(),
            "moved ZO_CONFIG_HOME while another test held the env lock: {ran_under_the_lock:?}"
        );
    }

    /// t-2947 — the listing reads transcript heads, and every row it shows
    /// is the row the whole-session loader showed: pinned over the synthetic
    /// corpus, capped and uncapped.
    #[test]
    fn the_listing_shows_what_the_whole_session_loader_showed() {
        use super::{list_managed_sessions_limited, recent_session_candidates};
        use runtime::{ContentBlock, MessageRole, Session};

        type Row = (
            String,
            Option<String>,
            u128,
            u128,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let pin = crate::SessionRootPin::new("identity", &session_corpus::CorpusSpec::REAL_MIX);
        let oracle = |limit: Option<usize>| -> Vec<Row> {
            let mut rows: Vec<Row> = recent_session_candidates(None)
                .expect("candidates")
                .into_iter()
                .filter_map(|(path, modified)| {
                    let session = Session::load_from_path(&path).ok()?;
                    let prompt = session.messages.iter().find(|message| {
                        message.role == MessageRole::User && !message.holds_tool_result()
                            && message.blocks.iter().any(|block| match block {
                                ContentBlock::Text { text } => !text.trim().is_empty(),
                                ContentBlock::Image { data, .. } => !data.is_empty(),
                                _ => false,
                            })
                    })?;
                    let preview = prompt.blocks.iter().find_map(|block| match block {
                        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.trim_start().chars().take(60).collect()),
                        _ => None,
                    });
                    Some((session.session_id.clone(), session.name.clone(), modified,
                        u128::from(session.created_at_ms),
                        session.fork.as_ref().map(|fork| fork.parent_session_id.clone()),
                        session.fork.as_ref().and_then(|fork| fork.branch_name.clone()), preview))
                })
                .collect();
            rows.sort_by(|l, r| r.2.cmp(&l.2).then_with(|| r.0.cmp(&l.0)));
            let mut seen = std::collections::HashSet::new();
            rows.retain(|row| seen.insert(row.0.clone()));
            if let Some(limit) = limit { rows.truncate(limit); }
            rows
        };
        for limit in [Some(crate::resume::DEFAULT_LIST_LIMIT), None] {
            let listed: Vec<Row> = list_managed_sessions_limited(limit)
                .expect("listing")
                .into_iter()
                .map(|s| (s.id, s.name, s.modified_epoch_millis, s.created_epoch_millis, s.parent_session_id, s.branch_name, s.first_user_text))
                .collect();
            let expected = oracle(limit);
            assert_eq!(listed.len(), expected.len(), "limit {limit:?}");
            assert_eq!(listed, expected, "limit {limit:?}");
        }
        assert_eq!(recent_session_candidates(None).unwrap().len(), pin.manifest.len());
    }
}
