//! Single source of truth for locating a session's todo store.
//!
//! Each session keeps its own plan, `<project state>/todos/<session id>.json`,
//! so two sessions in one folder never read or rewrite each other's. The
//! session id is handed in by whoever runs the conversation rather than read
//! from the process environment: one process runs a session and its helpers
//! at once. The one per-project file every session used to share,
//! `<project state>/.zo-todos.json`, is neither read nor deleted -- it may
//! still hold a person's plan, and it belongs to no session.
//!
//! The `TodoWrite` tool, the mid-turn re-anchor, the compaction re-injection
//! and the task lifecycle hooks all need to agree on *which file* holds the
//! live todo list. Historically each resolved the path on its own, and the
//! writer additionally fell back to a writable `<zo-home>/orphan-todos/` copy
//! when the primary store sat on a read-only filesystem -- but the readers
//! never knew about that fallback. The result was a split brain: in a
//! read-only cwd the writer saved todos to the fallback while the readers read
//! the empty primary, so the live list silently vanished.
//!
//! This module owns the path rule once. Writers persist to [`primary_store`]
//! (degrading to [`fallback_store`] on an unwritable primary); readers call
//! [`resolve_readable_store`], which transparently follows the same fallback.
//! Both halves share [`orphan_todo_key`] so a primary and its fallback always
//! map to each other.

use std::path::{Path, PathBuf};

/// Folder under a project's state holding one plan file per session.
pub const TODO_STORE_DIR: &str = "todos";

/// The primary todo store of session `session_id` in `cwd`. A non-empty
/// `ZO_TODO_STORE` wins for every session (a blank value behaves as unset);
/// otherwise the store lives under Zo's global per-project state directory so
/// todo writes do not dirty the workspace (while `ZO_STATE_DIR` still
/// redirects that state explicitly).
///
/// `None` without an override when there is no session, or its id cannot be a
/// file name: that plan belongs to no session, so it is kept nowhere another
/// session could read it.
#[must_use]
pub fn primary_store(cwd: &Path, session_id: Option<&str>) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("ZO_TODO_STORE") {
        if !explicit.to_string_lossy().trim().is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    let session_id = session_id.filter(|id| names_a_file(id))?;
    Some(
        crate::zo_project_state_dir(cwd)
            .join(TODO_STORE_DIR)
            .join(format!("{session_id}.json")),
    )
}

/// Whether `session_id` can name a plan file: the rule the background task
/// store holds its per-session files to (`TaskRegistry::for_session`).
fn names_a_file(session_id: &str) -> bool {
    !session_id.is_empty()
        && !matches!(session_id, "." | "..")
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

/// The writable per-user fallback for `primary` -- `<zo home>/orphan-todos/
/// <key>.json` -- used when the primary store's directory is read-only. Reuses
/// the same zo-home chain (`ZO_CONFIG_HOME`/`ZO_HOME`/`HOME`) sessions
/// already persist under. `None` only when no zo home can be resolved.
#[must_use]
pub fn fallback_store(primary: &Path) -> Option<PathBuf> {
    let home = crate::zo_global_config_roots().into_iter().next()?;
    Some(
        home.join("orphan-todos")
            .join(format!("{}.json", orphan_todo_key(primary))),
    )
}

/// A stable, collision-resistant filename stem for a store path, so distinct
/// projects never clobber each other's fallback store. A naive
/// non-alphanumeric->`_` mapping is **not injective** (`/a/b` and `/a-b` would
/// collide) and can be empty, so hash the full path string instead. The hasher
/// is fixed-seeded, so the same path always maps to the same file across runs.
#[must_use]
pub fn orphan_todo_key(store_path: &Path) -> String {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    store_path.to_string_lossy().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The store a *reader* should load for session `session_id` in `cwd`: the
/// primary when it exists, otherwise the writable fallback when *it* exists,
/// otherwise the primary path (so a missing-store reader still gets a stable,
/// sensible path to report empty from). `None` when the session has no store
/// ([`primary_store`]). This is the counterpart to the writer's
/// primary->fallback degrade, so a read-only cwd no longer hides the todos the
/// writer saved.
#[must_use]
pub fn resolve_readable_store(cwd: &Path, session_id: Option<&str>) -> Option<PathBuf> {
    let primary = primary_store(cwd, session_id)?;
    if primary.exists() {
        return Some(primary);
    }
    if let Some(fallback) = fallback_store(&primary) {
        if fallback.exists() {
            return Some(fallback);
        }
    }
    Some(primary)
}

#[cfg(test)]
mod tests {
    use super::{fallback_store, orphan_todo_key, primary_store, resolve_readable_store};
    use std::path::{Path, PathBuf};

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-todo-store-{tag}-{nanos}"))
    }

    /// The tail every store of session `session_id` ends with.
    fn session_file(session_id: &str) -> PathBuf {
        Path::new("todos").join(format!("{session_id}.json"))
    }

    #[test]
    fn explicit_non_empty_env_override_wins() {
        let _lock = crate::test_env_lock();
        let prior = std::env::var_os("ZO_TODO_STORE");
        std::env::set_var("ZO_TODO_STORE", "/tmp/explicit-todos.json");
        // An explicit store is every session's, and a session-less caller's.
        for session in [Some("session-a"), Some("session-b"), None] {
            assert_eq!(
                primary_store(Path::new("/whatever"), session),
                Some(PathBuf::from("/tmp/explicit-todos.json")),
                "{session:?}"
            );
        }
        match prior {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
    }

    #[test]
    fn blank_env_behaves_as_unset() {
        let _lock = crate::test_env_lock();
        let prior = std::env::var_os("ZO_TODO_STORE");
        std::env::set_var("ZO_TODO_STORE", "   ");
        let cwd = Path::new("/proj");
        let store = primary_store(cwd, Some("session-a")).expect("a session's store");
        assert!(store.ends_with(session_file("session-a")), "{}", store.display());
        assert!(store.is_absolute());
        match prior {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
    }

    #[test]
    fn default_store_uses_global_project_state_dir() {
        let _lock = crate::test_env_lock();
        let prior_todo = std::env::var_os("ZO_TODO_STORE");
        let prior_state = std::env::var_os("ZO_STATE_DIR");
        let prior_home = std::env::var_os("ZO_CONFIG_HOME");
        let root = unique_dir("global-state");
        let cwd = root.join("workspace");
        let home = root.join("home").join(".zo");
        std::env::remove_var("ZO_TODO_STORE");
        std::env::remove_var("ZO_STATE_DIR");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let store = primary_store(&cwd, Some("session-a")).expect("a session's store");

        assert!(store.starts_with(&home));
        assert!(store.ends_with(session_file("session-a")), "{}", store.display());
        assert!(!store.starts_with(&cwd));

        match prior_todo {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        match prior_state {
            Some(value) => std::env::set_var("ZO_STATE_DIR", value),
            None => std::env::remove_var("ZO_STATE_DIR"),
        }
        match prior_home {
            Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
            None => std::env::remove_var("ZO_CONFIG_HOME"),
        }
    }

    #[test]
    fn zo_state_dir_partitions_default_store_by_workspace() {
        let _lock = crate::test_env_lock();
        let prior_todo = std::env::var_os("ZO_TODO_STORE");
        let prior_state = std::env::var_os("ZO_STATE_DIR");
        let root = unique_dir("state-override");
        let state_root = root.join("state-root");
        std::env::remove_var("ZO_TODO_STORE");
        std::env::set_var("ZO_STATE_DIR", &state_root);

        let first = primary_store(&root.join("workspace-a"), Some("session-a")).expect("a store");
        let second = primary_store(&root.join("workspace-b"), Some("session-a")).expect("a store");

        assert!(first.starts_with(&state_root));
        assert!(second.starts_with(&state_root));
        assert_ne!(
            first, second,
            "state override must still partition per workspace"
        );
        assert!(first.ends_with(session_file("session-a")), "{}", first.display());
        match prior_todo {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        match prior_state {
            Some(value) => std::env::set_var("ZO_STATE_DIR", value),
            None => std::env::remove_var("ZO_STATE_DIR"),
        }
    }

    #[test]
    fn explicit_todo_store_wins_over_zo_state_dir() {
        let _lock = crate::test_env_lock();
        let prior_todo = std::env::var_os("ZO_TODO_STORE");
        let prior_state = std::env::var_os("ZO_STATE_DIR");
        let root = unique_dir("state-vs-explicit");
        let explicit = root.join("explicit.json");
        std::env::set_var("ZO_TODO_STORE", &explicit);
        std::env::set_var("ZO_STATE_DIR", root.join("state-root"));

        assert_eq!(primary_store(&root.join("workspace"), Some("session-a")), Some(explicit));
        match prior_todo {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        match prior_state {
            Some(value) => std::env::set_var("ZO_STATE_DIR", value),
            None => std::env::remove_var("ZO_STATE_DIR"),
        }
    }

    /// Two sessions in one folder keep two plans (t-68408): their stores, and
    /// the read-only fallbacks keyed off them, never meet, and neither is the
    /// file every session shared before.
    #[test]
    fn two_sessions_in_one_folder_keep_two_stores() {
        let _lock = crate::test_env_lock();
        let prior_todo = std::env::var_os("ZO_TODO_STORE");
        let prior_state = std::env::var_os("ZO_STATE_DIR");
        let root = unique_dir("two-sessions");
        std::env::remove_var("ZO_TODO_STORE");
        std::env::set_var("ZO_STATE_DIR", root.join("state-root"));
        let cwd = root.join("workspace");

        let first = primary_store(&cwd, Some("session-a")).expect("a session's store");
        let second = primary_store(&cwd, Some("session-b")).expect("a session's store");
        let shared = crate::zo_project_state_dir(&cwd).join(".zo-todos.json");

        assert_ne!(first, second, "two sessions in one folder must not share a plan");
        assert_ne!(first, shared, "a session never takes the old shared file");
        assert_ne!(second, shared, "a session never takes the old shared file");
        assert_eq!(first.parent(), second.parent(), "one plan folder per project");
        assert_ne!(
            fallback_store(&first),
            fallback_store(&second),
            "their read-only fallbacks stay apart too"
        );
        match prior_todo {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        match prior_state {
            Some(value) => std::env::set_var("ZO_STATE_DIR", value),
            None => std::env::remove_var("ZO_STATE_DIR"),
        }
    }

    /// A plan that belongs to no session has no store: no session, or an id
    /// that cannot be a file name, never falls back to a file another session
    /// reads.
    #[test]
    fn a_store_needs_a_session_that_names_a_file() {
        let _lock = crate::test_env_lock();
        let prior_todo = std::env::var_os("ZO_TODO_STORE");
        let prior_state = std::env::var_os("ZO_STATE_DIR");
        let root = unique_dir("no-session");
        std::env::remove_var("ZO_TODO_STORE");
        std::env::set_var("ZO_STATE_DIR", root.join("state-root"));
        let cwd = root.join("workspace");

        for session in [None, Some(""), Some("."), Some(".."), Some("../escape"), Some("a/b"), Some("two words")] {
            assert_eq!(primary_store(&cwd, session), None, "{session:?}");
            assert_eq!(resolve_readable_store(&cwd, session), None, "{session:?}");
        }
        match prior_todo {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        match prior_state {
            Some(value) => std::env::set_var("ZO_STATE_DIR", value),
            None => std::env::remove_var("ZO_STATE_DIR"),
        }
    }

    #[test]
    fn orphan_key_is_injective_stable_and_nonempty() {
        let a = orphan_todo_key(Path::new("/a/b"));
        let b = orphan_todo_key(Path::new("/a-b"));
        assert_ne!(a, b, "distinct paths must not collide");
        assert_eq!(a, orphan_todo_key(Path::new("/a/b")), "stable across calls");
        assert!(!a.is_empty());
        assert!(!orphan_todo_key(Path::new("")).is_empty());
    }

    #[test]
    fn fallback_lives_under_orphan_todos_dir() {
        let _lock = crate::test_env_lock();
        let primary = Path::new("/proj/.zo-todos.json");
        if let Some(fallback) = fallback_store(primary) {
            assert!(fallback.to_string_lossy().contains("orphan-todos"));
            assert!(fallback.extension().is_some_and(|ext| ext == "json"));
        }
    }

    #[test]
    fn resolve_prefers_primary_then_fallback_then_primary_path() {
        let _lock = crate::test_env_lock();
        let dir = unique_dir("resolve");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let primary = dir.join("todos.json");
        let prior = std::env::var_os("ZO_TODO_STORE");
        std::env::set_var("ZO_TODO_STORE", &primary);

        assert_eq!(resolve_readable_store(&dir, Some("session-a")), Some(primary.clone()));

        if let Some(fallback) = fallback_store(&primary) {
            if let Some(parent) = fallback.parent() {
                std::fs::create_dir_all(parent).expect("fallback dir");
            }
            std::fs::write(&fallback, b"[]").expect("write fallback");
            assert_eq!(resolve_readable_store(&dir, Some("session-a")), Some(fallback.clone()));
            let _ = std::fs::remove_file(&fallback);
        }

        std::fs::write(&primary, b"[]").expect("write primary");
        assert_eq!(resolve_readable_store(&dir, Some("session-a")), Some(primary));

        match prior {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
