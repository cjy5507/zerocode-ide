//! Single source of truth for locating the per-session todo store.
//!
//! The `TodoWrite` tool, the HUD sidebar, and the compaction re-injection all
//! need to agree on *which file* holds the live todo list. Historically each
//! resolved the path on its own, and the writer additionally fell back to a
//! writable `<zo-home>/orphan-todos/` copy when the primary store sat on a
//! read-only filesystem -- but the readers never knew about that fallback. The
//! result was a split brain: in a read-only cwd the writer saved todos to the
//! fallback while the HUD/compaction read the empty primary, so the live list
//! silently vanished.
//!
//! This module owns the path rule once. Writers persist to [`primary_store`]
//! (degrading to [`fallback_store`] on an unwritable primary); readers call
//! [`resolve_readable_store`], which transparently follows the same fallback.
//! Both halves share [`orphan_todo_key`] so a primary and its fallback always
//! map to each other.

use std::path::{Path, PathBuf};

/// Filename of the per-project todo store under the zo state base.
pub const TODO_STORE_FILE: &str = ".zo-todos.json";

/// The primary todo store for `cwd`, honoring the same overrides every reader
/// and writer must agree on: a non-empty `ZO_TODO_STORE` wins (a blank value
/// behaves as unset); otherwise the store lives under Zo's global per-project
/// state directory so todo writes do not dirty the workspace (while
/// `ZO_STATE_DIR` still redirects that state explicitly).
#[must_use]
pub fn primary_store(cwd: &Path) -> PathBuf {
    if let Some(explicit) = std::env::var_os("ZO_TODO_STORE") {
        if !explicit.to_string_lossy().trim().is_empty() {
            return PathBuf::from(explicit);
        }
    }
    crate::zo_project_state_dir(cwd).join(TODO_STORE_FILE)
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

/// The store a *reader* should load for `cwd`: the primary when it exists,
/// otherwise the writable fallback when *it* exists, otherwise the primary path
/// (so a missing-store reader still gets a stable, sensible path to report
/// empty from). This is the counterpart to the writer's primary->fallback
/// degrade, so a read-only cwd no longer hides the todos the writer saved.
#[must_use]
pub fn resolve_readable_store(cwd: &Path) -> PathBuf {
    let primary = primary_store(cwd);
    if primary.exists() {
        return primary;
    }
    if let Some(fallback) = fallback_store(&primary) {
        if fallback.exists() {
            return fallback;
        }
    }
    primary
}

#[cfg(test)]
mod tests {
    use super::{
        fallback_store, orphan_todo_key, primary_store, resolve_readable_store, TODO_STORE_FILE,
    };
    use std::path::{Path, PathBuf};

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-todo-store-{tag}-{nanos}"))
    }

    #[test]
    fn explicit_non_empty_env_override_wins() {
        let _lock = crate::test_env_lock();
        let prior = std::env::var_os("ZO_TODO_STORE");
        std::env::set_var("ZO_TODO_STORE", "/tmp/explicit-todos.json");
        assert_eq!(
            primary_store(Path::new("/whatever")),
            PathBuf::from("/tmp/explicit-todos.json")
        );
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
        assert!(primary_store(cwd).ends_with(TODO_STORE_FILE));
        assert!(primary_store(cwd).is_absolute());
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

        let store = primary_store(&cwd);

        assert!(store.starts_with(&home));
        assert!(store.ends_with(TODO_STORE_FILE));
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

        let first = primary_store(&root.join("workspace-a"));
        let second = primary_store(&root.join("workspace-b"));

        assert!(first.starts_with(&state_root));
        assert!(second.starts_with(&state_root));
        assert_ne!(
            first, second,
            "state override must still partition per workspace"
        );
        assert!(first.ends_with(TODO_STORE_FILE));
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

        assert_eq!(primary_store(&root.join("workspace")), explicit);
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
        let primary = dir.join(TODO_STORE_FILE);
        let prior = std::env::var_os("ZO_TODO_STORE");
        std::env::set_var("ZO_TODO_STORE", &primary);

        assert_eq!(resolve_readable_store(&dir), primary);

        if let Some(fallback) = fallback_store(&primary) {
            if let Some(parent) = fallback.parent() {
                std::fs::create_dir_all(parent).expect("fallback dir");
            }
            std::fs::write(&fallback, b"[]").expect("write fallback");
            assert_eq!(resolve_readable_store(&dir), fallback);
            let _ = std::fs::remove_file(&fallback);
        }

        std::fs::write(&primary, b"[]").expect("write primary");
        assert_eq!(resolve_readable_store(&dir), primary);

        match prior {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
