use std::env;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::{Session, SessionError, TranscriptHead};

pub const PRIMARY_SESSION_EXTENSION: &str = "jsonl";
pub const LEGACY_SESSION_EXTENSION: &str = "json";
pub const LATEST_SESSION_REFERENCE: &str = "latest";

const SESSION_REFERENCE_ALIASES: &[&str] = &[LATEST_SESSION_REFERENCE, "last", "recent"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandle {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub modified_epoch_millis: u128,
    pub parent_session_id: Option<String>,
    pub branch_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedManagedSession {
    pub handle: SessionHandle,
    pub session: Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedManagedSession {
    pub parent_session_id: String,
    pub handle: SessionHandle,
    pub session: Session,
    pub branch_name: Option<String>,
}

#[derive(Debug)]
pub enum SessionControlError {
    Io(std::io::Error),
    Session(SessionError),
    Format(String),
}

impl Display for SessionControlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Session(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SessionControlError {}

impl From<std::io::Error> for SessionControlError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<SessionError> for SessionControlError {
    fn from(value: SessionError) -> Self {
        Self::Session(value)
    }
}

pub fn sessions_dir() -> Result<PathBuf, SessionControlError> {
    managed_sessions_dir_for(env::current_dir()?)
}

pub fn managed_sessions_dir_for(
    base_dir: impl AsRef<Path>,
) -> Result<PathBuf, SessionControlError> {
    let path = managed_sessions_dir_path_for(base_dir);
    fs::create_dir_all(&path)?;
    // Sessions hold prompts and file contents; keep the directory owner-only so
    // other local users cannot read another user's transcripts. Best-effort: the
    // directory may live under a root the process does not own (e.g. a shared
    // `ZO_SESSION_ROOT`), and that must not break session creation.
    let _ = core_types::paths::restrict_permissions_owner_only(&path);
    Ok(path)
}

#[must_use]
pub fn managed_sessions_dir_path_for(base_dir: impl AsRef<Path>) -> PathBuf {
    if let Some(root) = env::var_os("ZO_SESSION_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root).join("sessions");
        }
    }
    global_project_sessions_dir_for(base_dir)
}

#[must_use]
pub fn global_project_sessions_dir_for(base_dir: impl AsRef<Path>) -> PathBuf {
    global_project_session_dirs_for(base_dir.as_ref())
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            crate::default_config_home()
                .join("projects")
                .join(crate::config::project_slug(base_dir.as_ref()))
                .join("sessions")
        })
}

fn global_project_session_dirs_for(base_dir: &Path) -> Vec<PathBuf> {
    let slug = crate::config::project_slug(base_dir);
    let mut dirs = Vec::new();
    for root in core_types::paths::zo_global_config_roots() {
        push_unique_path(
            &mut dirs,
            root.join("projects").join(&slug).join("sessions"),
        );
    }
    if dirs.is_empty() {
        push_unique_path(
            &mut dirs,
            crate::default_config_home()
                .join("projects")
                .join(slug)
                .join("sessions"),
        );
    }
    dirs
}

#[must_use]
pub fn workspace_sessions_dir_for(base_dir: impl AsRef<Path>) -> PathBuf {
    base_dir.as_ref().join(".zo").join("sessions")
}

#[must_use]
pub fn managed_session_search_dirs_for(base_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(root) = env::var_os("ZO_SESSION_ROOT") {
        if !root.is_empty() {
            push_unique_path(&mut dirs, PathBuf::from(root).join("sessions"));
        }
    }
    for directory in global_project_session_dirs_for(base_dir) {
        push_unique_path(&mut dirs, directory);
    }
    push_unique_path(&mut dirs, workspace_sessions_dir_for(base_dir));
    dirs
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

/// Default retention for session transcripts by last-activity date, mirroring
/// Claude Code's `cleanupPeriodDays` default.
pub const DEFAULT_SESSION_RETENTION_DAYS: u32 = 30;

/// What one retention sweep removed. Counts only — the sweep is best-effort
/// and silent about individual I/O failures (a locked or vanished file is
/// simply left for the next sweep).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionCleanupReport {
    pub removed_sessions: usize,
    pub removed_prefs: usize,
    /// Orphan `<session>.lock` files reclaimed — see
    /// [`core_types::session::try_reclaim_orphan_lock`].
    pub removed_locks: usize,
    pub removed_dirs: usize,
    pub reclaimed_bytes: u64,
}

impl SessionCleanupReport {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Delete session transcripts older than `retention_days` (by mtime — resume
/// touches a session, so an actively revisited transcript keeps renewing its
/// lease) across every project slug under the global Zo home, along with
/// each removed session's preferences file. Emptied `sessions/`,
/// `session-prefs/`, and slug directories are pruned afterwards, so orphan
/// slugs (e.g. from a tempdir cwd) age out of `~/.zo/projects/` on their
/// own instead of accumulating forever.
///
/// `session_recall` and `/resume` read these transcripts, so an expired
/// session is no longer recallable — the same trade Claude Code's
/// `cleanupPeriodDays` makes. An active session's file was appended moments
/// ago and can never be older than the cutoff.
#[must_use]
pub fn cleanup_expired_sessions(retention_days: u32) -> SessionCleanupReport {
    // checked_sub: an absurd retention (u32::MAX days from a clamped config
    // value) must degrade to "nothing expires", not panic on pre-epoch time.
    let Some(cutoff) = SystemTime::now().checked_sub(std::time::Duration::from_secs(
        u64::from(retention_days) * 24 * 60 * 60,
    )) else {
        return SessionCleanupReport::default();
    };
    cleanup_expired_sessions_under(&crate::default_config_home().join("projects"), cutoff)
}

/// Env-free core of [`cleanup_expired_sessions`]: sweep `projects_root`
/// removing session files last modified before `cutoff`. Split out so tests
/// drive the cutoff directly instead of manipulating file mtimes.
#[must_use]
pub fn cleanup_expired_sessions_under(
    projects_root: &Path,
    cutoff: SystemTime,
) -> SessionCleanupReport {
    let mut report = SessionCleanupReport::default();
    let Ok(slugs) = fs::read_dir(projects_root) else {
        return report;
    };
    for slug in slugs.flatten() {
        let slug_dir = slug.path();
        if !slug_dir.is_dir() {
            continue;
        }
        let sessions_dir = slug_dir.join("sessions");
        let prefs_dir = slug_dir.join("session-prefs");
        for entry in fs::read_dir(&sessions_dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let is_session_file = path.extension().and_then(|ext| ext.to_str()).is_some_and(
                |ext| ext == PRIMARY_SESSION_EXTENSION || ext == LEGACY_SESSION_EXTENSION,
            );
            if !is_session_file {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let expired = metadata
                .modified()
                .is_ok_and(|modified| modified < cutoff);
            if !expired || fs::remove_file(&path).is_err() {
                continue;
            }
            report.removed_sessions += 1;
            report.reclaimed_bytes += metadata.len();
            // The session's preferences ride its file stem (see
            // session_preferences::preferences_path).
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                let prefs = prefs_dir.join(format!("{stem}.json"));
                if let Ok(prefs_metadata) = fs::metadata(&prefs) {
                    if fs::remove_file(&prefs).is_ok() {
                        report.removed_prefs += 1;
                        report.reclaimed_bytes += prefs_metadata.len();
                    }
                }
            }
        }
        // The locks the sweep just orphaned, plus any left by earlier sweeps
        // that only knew how to delete transcripts. A lock whose session file
        // is gone guards nothing — but only the flock can say whether a writer
        // is still holding it, so that is what decides (a live session keeps
        // both its lock and, by extension, its directory).
        for entry in fs::read_dir(&sessions_dir).into_iter().flatten().flatten() {
            let lock_path = entry.path();
            let Some(session_path) = core_types::session::session_path_for_lock(&lock_path)
            else {
                continue;
            };
            if session_path.exists() {
                continue;
            }
            if core_types::session::try_reclaim_orphan_lock(&lock_path) {
                report.removed_locks += 1;
            }
        }

        // A memory store holding nothing but its own lock file is residue: the
        // lock is zero bytes, but it pins the slug directory forever
        // (`remove_dir` refuses non-empty), so slugs from throwaway cwds —
        // tempdir test runs, bench scratchpads — never aged out even after
        // their sessions did. Content is what earns a keep: any entry, journal
        // or marker means state; the lock alone means a store was opened and
        // never written. The flock is the proof of disuse, as with sessions.
        for store in [
            crate::memory::paths::MEMORY_STORE,
            crate::memory::paths::MEMORY_LOCAL_STORE,
        ] {
            let memory_dir = slug_dir.join(store);
            if !memory_store_is_contentless(&memory_dir) {
                continue;
            }
            let lock = memory_dir.join(crate::memory::dreamer::MEMORY_STORE_LOCK_FILE);
            if lock.exists() && core_types::session::try_reclaim_orphan_lock(&lock) {
                report.removed_locks += 1;
            }
            // Both removals refuse non-empty targets, so a writer that raced
            // in keeps everything it created.
            let _ = fs::remove_dir(memory_dir.join(crate::memory::dreamer::DREAMER_OWNED_DIR));
            if fs::remove_dir(&memory_dir).is_ok() {
                report.removed_dirs += 1;
            }
        }

        // Prune what the sweep emptied. `remove_dir` refuses non-empty
        // directories, so a slug still holding state (todos, memory, live
        // sessions) is never touched.
        for dir in [&sessions_dir, &prefs_dir] {
            if fs::remove_dir(dir).is_ok() {
                report.removed_dirs += 1;
            }
        }
        if fs::remove_dir(&slug_dir).is_ok() {
            report.removed_dirs += 1;
        }
    }
    report
}

/// True when a memory store directory holds no state worth keeping: nothing at
/// all, or only its own advisory lock and an empty `.dreamer-owned/`. A
/// journal, a marker, an archive or any entry is state — the sweep never
/// deletes content, only the scaffolding a store leaves behind when it was
/// opened and never used.
fn memory_store_is_contentless(memory_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(memory_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == crate::memory::dreamer::MEMORY_STORE_LOCK_FILE {
            continue;
        }
        if name == crate::memory::dreamer::DREAMER_OWNED_DIR {
            match fs::read_dir(entry.path()) {
                Ok(mut marker_entries) => {
                    if marker_entries.next().is_some() {
                        return false;
                    }
                }
                Err(_) => return false,
            }
            continue;
        }
        return false;
    }
    true
}

pub fn create_managed_session_handle(
    session_id: &str,
) -> Result<SessionHandle, SessionControlError> {
    create_managed_session_handle_for(env::current_dir()?, session_id)
}

pub fn create_managed_session_handle_for(
    base_dir: impl AsRef<Path>,
    session_id: &str,
) -> Result<SessionHandle, SessionControlError> {
    let id = session_id.to_string();
    let path =
        managed_sessions_dir_for(base_dir)?.join(format!("{id}.{PRIMARY_SESSION_EXTENSION}"));
    Ok(SessionHandle { id, path })
}

pub fn resolve_session_reference(reference: &str) -> Result<SessionHandle, SessionControlError> {
    resolve_session_reference_for(env::current_dir()?, reference)
}

pub fn resolve_session_reference_for(
    base_dir: impl AsRef<Path>,
    reference: &str,
) -> Result<SessionHandle, SessionControlError> {
    let base_dir = base_dir.as_ref();
    if is_session_reference_alias(reference) {
        let latest = latest_managed_session_for(base_dir)?;
        return Ok(SessionHandle {
            id: latest.id,
            path: latest.path,
        });
    }

    let direct = PathBuf::from(reference);
    let candidate = if direct.is_absolute() {
        direct.clone()
    } else {
        base_dir.join(&direct)
    };
    let looks_like_path = direct.extension().is_some() || direct.components().count() > 1;
    let path = if candidate.exists() {
        candidate
    } else if looks_like_path {
        return Err(SessionControlError::Format(
            format_missing_session_reference(reference),
        ));
    } else {
        resolve_managed_session_path_for(base_dir, reference)?
    };

    Ok(SessionHandle {
        id: session_id_from_path(&path).unwrap_or_else(|| reference.to_string()),
        path,
    })
}

pub fn resolve_managed_session_path(session_id: &str) -> Result<PathBuf, SessionControlError> {
    resolve_managed_session_path_for(env::current_dir()?, session_id)
}

pub fn resolve_managed_session_path_for(
    base_dir: impl AsRef<Path>,
    session_id: &str,
) -> Result<PathBuf, SessionControlError> {
    let base_dir = base_dir.as_ref();
    for directory in managed_session_search_dirs_for(base_dir) {
        for extension in [PRIMARY_SESSION_EXTENSION, LEGACY_SESSION_EXTENSION] {
            let path = directory.join(format!("{session_id}.{extension}"));
            if path.exists() {
                return Ok(path);
            }
        }
    }
    Err(SessionControlError::Format(
        format_missing_session_reference(session_id),
    ))
}

#[must_use]
pub fn is_managed_session_file(path: &Path) -> bool {
    let has_session_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|extension| {
            extension == PRIMARY_SESSION_EXTENSION || extension == LEGACY_SESSION_EXTENSION
        });
    if !has_session_extension {
        return false;
    }
    // Sidecar files live in the sessions directory and share transcript
    // extensions but are NOT resumable transcripts: the append-only Raw Vault
    // (`<id>.vault.jsonl`), rotated transcript fragments
    // (`<id>.rot-<ts>.jsonl`), per-session todo stores (`<id>.todos.json`), and
    // per-session preference stores (`<id>.prefs.json`). Excluding them keeps
    // them out of session listings and out of `resume latest` (which picks by
    // mtime — sidecars can be touched after their owning transcript and would
    // otherwise win).
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    !name.contains(".vault.")
        && !name.contains(".rot-")
        && !name.ends_with(".todos.json")
        && !name.ends_with(".prefs.json")
}

pub fn list_managed_sessions() -> Result<Vec<ManagedSessionSummary>, SessionControlError> {
    list_managed_sessions_for(env::current_dir()?)
}

pub fn list_managed_sessions_for(
    base_dir: impl AsRef<Path>,
) -> Result<Vec<ManagedSessionSummary>, SessionControlError> {
    let base_dir = base_dir.as_ref();
    let mut sessions = Vec::new();
    for directory in managed_session_search_dirs_for(base_dir) {
        if !directory.exists() {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if !is_managed_session_file(&path) {
                continue;
            }
            let metadata = entry.metadata()?;
            let modified_epoch_millis = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or_default();
            // The head alone: id and fork provenance sit in the header record,
            // so the transcript behind it is never read (t-2947).
            let (id, parent_session_id, branch_name) = match TranscriptHead::read(&path, None) {
                Ok(head) => {
                    let parent_session_id = head
                        .fork
                        .as_ref()
                        .map(|fork| fork.parent_session_id.clone());
                    let branch_name = head.fork.and_then(|fork| fork.branch_name);
                    (head.session_id, parent_session_id, branch_name)
                }
                Err(_) => (
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                    None,
                    None,
                ),
            };
            sessions.push(ManagedSessionSummary {
                id,
                path,
                modified_epoch_millis,
                parent_session_id,
                branch_name,
            });
        }
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

pub fn latest_managed_session() -> Result<ManagedSessionSummary, SessionControlError> {
    latest_managed_session_for(env::current_dir()?)
}

pub fn latest_managed_session_for(
    base_dir: impl AsRef<Path>,
) -> Result<ManagedSessionSummary, SessionControlError> {
    latest_managed_session_for_excluding(base_dir, None)
}

pub fn latest_managed_session_for_excluding(
    base_dir: impl AsRef<Path>,
    exclude_id: Option<&str>,
) -> Result<ManagedSessionSummary, SessionControlError> {
    list_managed_sessions_for(base_dir)?
        .into_iter()
        .find(|summary| exclude_id.is_none_or(|id| summary.id != id))
        .ok_or_else(|| SessionControlError::Format(format_no_managed_sessions()))
}

pub fn load_managed_session_for(
    base_dir: impl AsRef<Path>,
    reference: &str,
) -> Result<LoadedManagedSession, SessionControlError> {
    load_managed_session_for_excluding(base_dir, reference, None)
}

pub fn load_managed_session_for_excluding(
    base_dir: impl AsRef<Path>,
    reference: &str,
    exclude_id: Option<&str>,
) -> Result<LoadedManagedSession, SessionControlError> {
    let base_dir = base_dir.as_ref();
    let handle = if is_session_reference_alias(reference) {
        let latest = latest_managed_session_for_excluding(base_dir, exclude_id)?;
        SessionHandle {
            id: latest.id,
            path: latest.path,
        }
    } else {
        resolve_session_reference_for(base_dir, reference)?
    };
    let session = Session::load_from_path(&handle.path)?;
    Ok(LoadedManagedSession {
        handle: SessionHandle {
            id: session.session_id.clone(),
            path: handle.path,
        },
        session,
    })
}

pub fn fork_managed_session_for(
    base_dir: impl AsRef<Path>,
    session: &Session,
    branch_name: Option<String>,
) -> Result<ForkedManagedSession, SessionControlError> {
    let parent_session_id = session.session_id.clone();
    let forked = session.fork(branch_name);
    let handle = create_managed_session_handle_for(base_dir, &forked.session_id)?;
    let branch_name = forked
        .fork
        .as_ref()
        .and_then(|fork| fork.branch_name.clone());
    let forked = forked.with_persistence_path(handle.path.clone());
    forked.save_to_path(&handle.path)?;
    Ok(ForkedManagedSession {
        parent_session_id,
        handle,
        session: forked,
        branch_name,
    })
}

#[must_use]
pub fn is_session_reference_alias(reference: &str) -> bool {
    SESSION_REFERENCE_ALIASES
        .iter()
        .any(|alias| reference.eq_ignore_ascii_case(alias))
}

fn session_id_from_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .and_then(|name| {
            name.strip_suffix(&format!(".{PRIMARY_SESSION_EXTENSION}"))
                .or_else(|| name.strip_suffix(&format!(".{LEGACY_SESSION_EXTENSION}")))
        })
        .map(ToOwned::to_owned)
}

/// Message for a `--resume <ref>` that matched no managed session.
///
/// Single source of truth for this hint — the CLI re-exports it (see
/// `zo_cli::session_format`) so both surfaces stay in sync. The
/// wording reflects the real layout: managed sessions live in the global
/// per-project store or the workspace `.zo/sessions/` directory.
#[must_use]
pub fn format_missing_session_reference(reference: &str) -> String {
    format!(
        "session not found: {reference}\nHint: managed sessions live in your Zo per-project store (~/.zo/projects/<project>/sessions) or the workspace .zo/sessions/ directory. Try `{LATEST_SESSION_REFERENCE}` for the most recent session or `/session list` in the REPL."
    )
}

/// Message when no managed sessions exist at all. Single source of truth (see
/// [`format_missing_session_reference`]); the CLI re-exports it.
#[must_use]
pub fn format_no_managed_sessions() -> String {
    format!(
        "no managed sessions found in your Zo per-project store (~/.zo/projects/<project>/sessions)\nStart `zo` to create a session, then rerun with `--resume {LATEST_SESSION_REFERENCE}`."
    )
}

#[cfg(test)]
mod tests {
    use super::{
        create_managed_session_handle_for, fork_managed_session_for, is_managed_session_file,
        is_session_reference_alias, list_managed_sessions_for, load_managed_session_for,
        load_managed_session_for_excluding, managed_session_search_dirs_for,
        managed_sessions_dir_path_for, resolve_session_reference_for, ManagedSessionSummary,
        LATEST_SESSION_REFERENCE,
    };
    use crate::session::Session;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "runtime-session-control-{}-{counter}-{nanos}",
            std::process::id()
        ))
    }

    fn with_config_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let _lock = crate::test_env_lock();
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let prior_session_root = std::env::var_os("ZO_SESSION_ROOT");
        std::env::set_var("ZO_CONFIG_HOME", home);
        std::env::remove_var("ZO_SESSION_ROOT");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prior_config_home {
            Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
            None => std::env::remove_var("ZO_CONFIG_HOME"),
        }
        match prior_session_root {
            Some(value) => std::env::set_var("ZO_SESSION_ROOT", value),
            None => std::env::remove_var("ZO_SESSION_ROOT"),
        }
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[test]
    fn search_dirs_include_every_canonical_global_home() {
        let _lock = crate::test_env_lock();
        let root = temp_dir();
        let workspace = root.join("workspace");
        let config_home = root.join("config-home");
        let zo_home = root.join("zo-home");
        let user_home = root.join("user-home");
        let prior_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let prior_zo_home = std::env::var_os("ZO_HOME");
        let prior_home = std::env::var_os("HOME");
        let prior_session_root = std::env::var_os("ZO_SESSION_ROOT");
        std::env::set_var("ZO_CONFIG_HOME", &config_home);
        std::env::set_var("ZO_HOME", &zo_home);
        std::env::set_var("HOME", &user_home);
        std::env::remove_var("ZO_SESSION_ROOT");

        let dirs = managed_session_search_dirs_for(&workspace);

        for (key, value) in [
            ("ZO_CONFIG_HOME", prior_config_home),
            ("ZO_HOME", prior_zo_home),
            ("HOME", prior_home),
            ("ZO_SESSION_ROOT", prior_session_root),
        ] {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        let slug = crate::config::project_slug(&workspace);
        assert_eq!(
            dirs,
            vec![
                config_home.join("projects").join(&slug).join("sessions"),
                zo_home.join("projects").join(&slug).join("sessions"),
                user_home
                    .join(".zo")
                    .join("projects")
                    .join(&slug)
                    .join("sessions"),
                user_home
                    .join(".forge")
                    .join("projects")
                    .join(&slug)
                    .join("sessions"),
                workspace.join(".zo").join("sessions"),
            ]
        );
    }

    #[test]
    fn retention_sweep_removes_expired_keeps_fresh_and_prunes_empty_slugs() {
        use super::cleanup_expired_sessions_under;
        let root = temp_dir();
        // Slug A: one expired session (+ prefs) and one fresh session.
        let a_sessions = root.join("slug-a").join("sessions");
        let a_prefs = root.join("slug-a").join("session-prefs");
        fs::create_dir_all(&a_sessions).expect("a sessions");
        fs::create_dir_all(&a_prefs).expect("a prefs");
        fs::write(a_sessions.join("old.jsonl"), "x".repeat(100)).expect("old session");
        fs::write(a_prefs.join("old.json"), "{}").expect("old prefs");
        fs::write(a_sessions.join("new.jsonl"), "y").expect("new session");
        // Slug B: only an expired session — the whole slug should vanish.
        let b_sessions = root.join("slug-b").join("sessions");
        fs::create_dir_all(&b_sessions).expect("b sessions");
        fs::write(b_sessions.join("done.jsonl"), "z").expect("b session");
        // Slug C: no sessions at all, but live state — must never be touched.
        let c_state = root.join("slug-c").join("state");
        fs::create_dir_all(&c_state).expect("c state");
        fs::write(c_state.join("todos.json"), "[]").expect("c todos");

        // Cutoff in the future relative to `old`/`done`, in the past for `new`.
        let now = SystemTime::now();
        let old_time = now - std::time::Duration::from_secs(60);
        // Backdate nothing: instead pick a cutoff between file creation times.
        // All files were just written, so make `new` fresh by rewriting it
        // after choosing the cutoff boundary of "now".
        std::thread::sleep(std::time::Duration::from_millis(20));
        let cutoff = SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(a_sessions.join("new.jsonl"), "y2").expect("touch new");
        let _ = old_time;

        let report = cleanup_expired_sessions_under(&root, cutoff);

        assert_eq!(report.removed_sessions, 2, "old + done expire");
        assert_eq!(report.removed_prefs, 1, "old's prefs follow it");
        assert!(!a_sessions.join("old.jsonl").exists());
        assert!(a_sessions.join("new.jsonl").exists(), "fresh session survives");
        assert!(!a_prefs.exists(), "emptied prefs dir is pruned");
        assert!(!root.join("slug-b").exists(), "fully-emptied slug is pruned");
        assert!(
            c_state.join("todos.json").exists(),
            "slugs holding live state are untouched"
        );
        assert!(report.reclaimed_bytes >= 100);

        // Idempotent: nothing left to expire.
        let second = cleanup_expired_sessions_under(&root, cutoff);
        assert!(second.is_empty(), "second sweep finds nothing: {second:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// **락 누수 회귀 핀** — 보존 스윕은 트랜스크립트만 지우고 형제
    /// `<file>.lock` 은 놔뒀다(한 머신에 6,680개). 0바이트라 용량은 아니지만
    /// `remove_dir` 가 영원히 실패해서 **"고아 슬러그가 알아서 age out 된다"는
    /// 약속이 거짓**이 됐다. 지워도 되는지의 증거는 **flock 자체**다.
    #[test]
    fn retention_sweep_reclaims_orphan_locks_and_then_prunes_the_slug() {
        use super::cleanup_expired_sessions_under;
        let root = temp_dir();
        let sessions = root.join("slug").join("sessions");
        fs::create_dir_all(&sessions).expect("sessions dir");
        fs::write(sessions.join("gone.jsonl"), "x").expect("session");
        fs::write(sessions.join("gone.jsonl.lock"), "").expect("its lock");
        // A lock left behind by an *earlier* sweep, its session already gone.
        fs::write(sessions.join("ancient.jsonl.lock"), "").expect("stale lock");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let cutoff = SystemTime::now();

        let report = cleanup_expired_sessions_under(&root, cutoff);

        assert_eq!(report.removed_sessions, 1);
        assert_eq!(report.removed_locks, 2, "both locks guard nothing: {report:?}");
        assert!(
            !root.join("slug").exists(),
            "with the locks gone the emptied slug finally prunes"
        );
    }

    /// 살아있는 세션의 락은 **세션 파일이 사라져도** 남는다 — 그 프로세스가
    /// flock 을 쥐고 있으므로 회수 시도가 실패한다. 이게 무너지면 두 writer 가
    /// 서로 다른 inode 를 잠그고 **둘 다 리스를 가졌다고 믿는다**.
    #[test]
    fn a_held_lock_is_never_reclaimed_even_with_its_session_deleted() {
        use super::cleanup_expired_sessions_under;
        let root = temp_dir();
        let sessions = root.join("slug").join("sessions");
        fs::create_dir_all(&sessions).expect("sessions dir");
        let session_path = sessions.join("live.jsonl");

        // A live writer: binding + one write takes the advisory lease.
        let mut live = Session::new().with_persistence_path(&session_path);
        live.push_user_text("held").expect("write takes the lease");
        let lock_path = core_types::session::lock_file_for(&session_path);
        assert!(lock_path.exists(), "the write created the lock");

        // Its transcript disappears out from under it (a manual delete, or a
        // sweep that ran first).
        fs::remove_file(&session_path).expect("remove the transcript");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let report = cleanup_expired_sessions_under(&root, SystemTime::now());

        assert_eq!(
            report.removed_locks, 0,
            "a lease still held must survive the sweep: {report:?}"
        );
        assert!(lock_path.exists(), "the live writer keeps its lock file");

        // Once that writer is gone the same sweep reclaims it.
        drop(live);
        let second = cleanup_expired_sessions_under(&root, SystemTime::now());
        assert_eq!(second.removed_locks, 1, "{second:?}");
        assert!(!lock_path.exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// **memory 잔해 회귀 핀** — 락 파일 하나만 남은 memory store 는 슬러그를
    /// 영원히 고정한다(`remove_dir` 가 ENOTEMPTY 로 거부). 실측(개발 머신
    /// 8,936 슬러그 · 세션 보유 41개)에서 이런 빈 store 는 16개였다. 수는
    /// 적지만 age out 이 **영구히** 막히는 경로라 예방적으로 회수한다.
    /// 내용(.md·journal·marker)이 있으면 절대 건드리지 않는다.
    #[test]
    fn retention_sweep_reclaims_a_lock_only_memory_store() {
        use super::cleanup_expired_sessions_under;
        let root = temp_dir();
        let slug = root.join("slug");
        let memory = slug.join("memory");
        fs::create_dir_all(memory.join(".dreamer-owned")).expect("store scaffolding");
        fs::write(memory.join(".memory-store.lock"), "").expect("orphan store lock");
        // A sessions dir whose transcript expires in the same sweep.
        let sessions = slug.join("sessions");
        fs::create_dir_all(&sessions).expect("sessions dir");
        fs::write(sessions.join("gone.jsonl"), "x").expect("session");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let report = cleanup_expired_sessions_under(&root, SystemTime::now());

        assert!(!slug.exists(), "the whole slug ages out: {report:?}");

        // A store with any content is never touched, lock or no lock.
        let kept = root.join("kept");
        let kept_memory = kept.join("memory");
        fs::create_dir_all(&kept_memory).expect("kept store");
        fs::write(kept_memory.join(".memory-store.lock"), "").expect("lock");
        fs::write(kept_memory.join("lesson.md"), "real memory").expect("entry");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = cleanup_expired_sessions_under(&root, SystemTime::now());
        assert!(
            kept_memory.join("lesson.md").exists() && kept_memory.join(".memory-store.lock").exists(),
            "content keeps both the store and its lock: {second:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    fn persist_session(root: &Path, text: &str) -> Session {
        let mut session = Session::new();
        session
            .push_user_text(text)
            .expect("session message should save");
        let handle = create_managed_session_handle_for(root, &session.session_id)
            .expect("managed session handle should build");
        let session = session.with_persistence_path(handle.path.clone());
        session
            .save_to_path(&handle.path)
            .expect("session should persist");
        session
    }

    fn wait_for_next_millisecond() {
        let start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_millis();
        while SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_millis()
            <= start
        {}
    }

    fn summary_by_id<'a>(
        summaries: &'a [ManagedSessionSummary],
        id: &str,
    ) -> &'a ManagedSessionSummary {
        summaries
            .iter()
            .find(|summary| summary.id == id)
            .expect("session summary should exist")
    }

    #[test]
    fn creates_and_lists_managed_sessions() {
        // given
        let root = temp_dir();
        fs::create_dir_all(&root).expect("root dir should exist");
        let config_home = root.join("home").join(".zo");
        with_config_home(&config_home, || {
            let older = persist_session(&root, "older session");
            wait_for_next_millisecond();
            let newer = persist_session(&root, "newer session");

            // when
            let sessions = list_managed_sessions_for(&root).expect("managed sessions should list");

            // then
            assert_eq!(sessions.len(), 2);
            assert_eq!(sessions[0].id, newer.session_id);
            assert_eq!(summary_by_id(&sessions, &older.session_id).id, older.session_id);
            assert_eq!(summary_by_id(&sessions, &newer.session_id).id, newer.session_id);
            assert!(managed_sessions_dir_path_for(&root).starts_with(&config_home));
        });
        fs::remove_dir_all(root).expect("temp dir should clean up");
    }

    #[test]
    fn lists_skip_vault_rotation_and_todo_sidecars() {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("root dir should exist");
        let config_home = root.join("home").join(".zo");
        with_config_home(&config_home, || {
            let real = persist_session(&root, "real session");
            let dir = managed_sessions_dir_path_for(&root);

            // Sidecars next to the real transcript: a Raw Vault and a rotated
            // fragment. Both share the `.jsonl` extension but must never surface
            // as resumable sessions (a vault is touched on every compaction, so
            // it would otherwise win `resume latest` by mtime).
            fs::write(
                dir.join(format!("{}.vault.jsonl", real.session_id)),
                "{\"type\":\"vault\",\"vault_seq\":0,\"message\":{}}\n",
            )
            .expect("vault sidecar should write");
            fs::write(
                dir.join(format!("{}.rot-123.jsonl", real.session_id)),
                "not a session\n",
            )
            .expect("rotation sidecar should write");
            fs::write(dir.join(format!("{}.todos.json", real.session_id)), "[]")
                .expect("todo sidecar should write");

            let sessions =
                list_managed_sessions_for(&root).expect("managed sessions should list");

            assert_eq!(
                sessions.len(),
                1,
                "only the real transcript lists; vault/rotation/todo sidecars are skipped"
            );
            assert_eq!(sessions[0].id, real.session_id);
        });
        fs::remove_dir_all(root).expect("temp dir should clean up");
    }

    #[test]
    fn sidecars_are_not_managed_session_files() {
        assert!(is_managed_session_file(Path::new("session-1-2.jsonl")));
        assert!(is_managed_session_file(Path::new("session-1-2.json")));
        assert!(!is_managed_session_file(Path::new(
            "session-1-2.vault.jsonl"
        )));
        assert!(!is_managed_session_file(Path::new(
            "session-1-2.rot-123.jsonl"
        )));
        assert!(!is_managed_session_file(Path::new(
            "session-1-2.todos.json"
        )));
        assert!(!is_managed_session_file(Path::new(
            "session-1-2.prefs.json"
        )));
        assert!(!is_managed_session_file(Path::new("notes.txt")));
    }

    #[test]
    fn resolves_latest_alias_and_loads_session_from_workspace_root() {
        // given
        let root = temp_dir();
        fs::create_dir_all(&root).expect("root dir should exist");
        let config_home = root.join("home").join(".zo");
        with_config_home(&config_home, || {
            let older = persist_session(&root, "older session");
            wait_for_next_millisecond();
            let newer = persist_session(&root, "newer session");

            // when
            let handle = resolve_session_reference_for(&root, LATEST_SESSION_REFERENCE)
                .expect("latest alias should resolve");
            let loaded = load_managed_session_for(&root, "recent")
                .expect("recent alias should load the latest session");

            // then
            assert_eq!(handle.id, newer.session_id);
            assert_eq!(loaded.handle.id, newer.session_id);
            assert_eq!(loaded.session.messages.len(), 1);
            assert_ne!(loaded.handle.id, older.session_id);
            assert!(is_session_reference_alias("last"));
        });
        fs::remove_dir_all(root).expect("temp dir should clean up");
    }

    #[test]
    fn latest_alias_ignores_todo_sidecar_when_excluding_current_session() {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("root dir should exist");
        let config_home = root.join("home").join(".zo");
        with_config_home(&config_home, || {
            let prior = persist_session(&root, "prior session");
            wait_for_next_millisecond();
            let current = persist_session(&root, "current session");
            wait_for_next_millisecond();
            let dir = managed_sessions_dir_path_for(&root);
            fs::write(dir.join(format!("{}.todos.json", current.session_id)), "[]")
                .expect("todo sidecar should write");

            let loaded = load_managed_session_for_excluding(
                &root,
                LATEST_SESSION_REFERENCE,
                Some(&current.session_id),
            )
            .expect("latest should skip current session and its sidecars");

            assert_eq!(loaded.handle.id, prior.session_id);
            assert_eq!(loaded.session.messages.len(), 1);
        });
        fs::remove_dir_all(root).expect("temp dir should clean up");
    }

    #[test]
    fn forks_session_into_managed_storage_with_lineage() {
        // given
        let root = temp_dir();
        fs::create_dir_all(&root).expect("root dir should exist");
        let config_home = root.join("home").join(".zo");
        with_config_home(&config_home, || {
            let source = persist_session(&root, "parent session");

            // when
            let forked =
                fork_managed_session_for(&root, &source, Some("incident-review".to_string()))
                    .expect("session should fork");
            let sessions = list_managed_sessions_for(&root).expect("managed sessions should list");
            let summary = summary_by_id(&sessions, &forked.handle.id);

            // then
            assert_eq!(forked.parent_session_id, source.session_id);
            assert_eq!(forked.branch_name.as_deref(), Some("incident-review"));
            assert_eq!(
                summary.parent_session_id.as_deref(),
                Some(source.session_id.as_str())
            );
            assert_eq!(summary.branch_name.as_deref(), Some("incident-review"));
            assert_eq!(
                forked.session.persistence_path(),
                Some(forked.handle.path.as_path())
            );
        });
        fs::remove_dir_all(root).expect("temp dir should clean up");
    }

    /// t-2947 — the listing reads transcript heads, and every row it gives is
    /// the row the whole-session loader gave: pinned over the synthetic corpus.
    #[test]
    fn the_listing_shows_what_the_whole_session_loader_showed() {
        let root = temp_dir();
        let base = root.join("workspace");
        let manifest = session_corpus::generate(
            &base.join(".zo").join("sessions"),
            &session_corpus::CorpusSpec::REAL_MIX,
        )
        .expect("synthetic corpus");
        with_config_home(&root.join("config-home"), || {
            type Row = (String, PathBuf, u128, Option<String>, Option<String>);
            let mut expected: Vec<Row> = Vec::new();
            for directory in super::managed_session_search_dirs_for(&base) {
                let Ok(entries) = fs::read_dir(&directory) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !super::is_managed_session_file(&path) {
                        continue;
                    }
                    let modified = entry
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .expect("mtime")
                        .duration_since(UNIX_EPOCH)
                        .expect("epoch")
                        .as_millis();
                    let (id, parent, branch) = match Session::load_from_path(&path) {
                        Ok(session) => (
                            session.session_id.clone(),
                            session.fork.as_ref().map(|fork| fork.parent_session_id.clone()),
                            session.fork.as_ref().and_then(|fork| fork.branch_name.clone()),
                        ),
                        Err(_) => (
                            path.file_stem()
                                .and_then(|stem| stem.to_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            None,
                            None,
                        ),
                    };
                    expected.push((id, path, modified, parent, branch));
                }
            }
            expected.sort_by(|l, r| r.2.cmp(&l.2).then_with(|| r.0.cmp(&l.0)));
            expected.dedup_by(|l, r| l.0 == r.0);

            let listed: Vec<Row> = super::list_managed_sessions_for(&base)
                .expect("listing")
                .into_iter()
                .map(|s| (s.id, s.path, s.modified_epoch_millis, s.parent_session_id, s.branch_name))
                .collect();
            assert_eq!(listed.len(), manifest.len());
            assert_eq!(listed, expected);
        });
        let _ = fs::remove_dir_all(&root);
    }
}
