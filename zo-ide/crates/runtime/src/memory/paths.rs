use std::path::{Path, PathBuf};

pub const MEMORY_STORE: &str = "memory";
pub const MEMORY_LOCAL_STORE: &str = "memory.local";
pub const MEMORY_INDEX_FILE: &str = "MEMORY.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryReadRoot {
    pub dir: PathBuf,
    pub display_prefix: String,
}

#[must_use]
pub fn memory_write_dir(cwd: &Path, local: bool) -> PathBuf {
    global_project_memory_dir(cwd, memory_store_name(local))
}

#[must_use]
pub fn global_project_memory_dir(cwd: &Path, store: &str) -> PathBuf {
    project_memory_dir_under(&crate::default_config_home(), cwd, store)
}

/// The per-project memory store directory under a specific global config root.
/// Writes always target the primary root via [`global_project_memory_dir`];
/// this exists so reads can enumerate every canonical root.
fn project_memory_dir_under(config_home: &Path, cwd: &Path, store: &str) -> PathBuf {
    let root = memory_project_root(cwd);
    config_home
        .join("projects")
        .join(crate::config::project_slug(&root))
        .join(store)
}

/// Stable logical project root for global memory. In git repositories we key
/// memory by the worktree root so sessions launched from nested directories
/// share the same durable memories. Outside git, the provided cwd remains the
/// project boundary.
#[must_use]
pub fn memory_project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map_or_else(|| cwd.to_path_buf(), Path::to_path_buf)
}

#[must_use]
pub const fn memory_store_name(local: bool) -> &'static str {
    if local {
        MEMORY_LOCAL_STORE
    } else {
        MEMORY_STORE
    }
}

/// Read roots for merged global memory, spanning every canonical global config
/// root (`core_types::paths::zo_global_config_roots()`).
///
/// Ordering encodes conflict precedence for the last-wins merge in
/// [`crate::memory::recall`]: lower-priority roots are emitted first and the
/// primary (highest-priority) root last, so on a slug collision the primary
/// root wins while every unique lower-root entry is still merged in. Within a
/// single root, `memory` precedes `memory.local` so the machine-local overlay
/// keeps overriding the durable store.
#[must_use]
pub fn global_memory_read_roots(cwd: &Path) -> Vec<MemoryReadRoot> {
    // `zo_global_config_roots()` is highest-priority first; reverse so the
    // primary root's entries are inserted last (and therefore win).
    crate::zo_global_config_roots()
        .into_iter()
        .rev()
        .flat_map(|config_home| {
            [MEMORY_STORE, MEMORY_LOCAL_STORE]
                .into_iter()
                .map(move |store| {
                    let dir = project_memory_dir_under(&config_home, cwd, store);
                    MemoryReadRoot {
                        display_prefix: dir.display().to_string(),
                        dir,
                    }
                })
        })
        .collect()
}

#[must_use]
pub fn memory_index_candidates(cwd: &Path) -> Vec<PathBuf> {
    global_memory_read_roots(cwd)
        .into_iter()
        .map(|root| root.dir.join(MEMORY_INDEX_FILE))
        .collect()
}
