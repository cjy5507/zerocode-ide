//! Durable graph checkpoints. Unlike the phase cache, an unreadable checkpoint
//! stops execution: treating it as a cache miss could repeat an external effect.

use std::path::{Path, PathBuf};

use runtime::secure_fs::{self, ExclusiveFileLock};
use serde_json::Value;

const MAX_CHECKPOINT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RUN_ID_BYTES: usize = 128;

/// A graph executor must persist its intent before running a node and its
/// result before advancing. Either failure stops the executor.
pub(crate) trait GraphStore {
    fn load(&self) -> Result<Option<Value>, String>;
    fn save(&self, checkpoint: &Value) -> Result<(), String>;
}

/// Holds one run's exclusive lock for the entire execution or recovery pass.
/// The lock file persists; dropping its descriptor releases the OS lock even
/// after a crash, without an existence-lock cleanup race.
#[derive(Debug)]
pub(crate) struct FileGraphStore {
    root: PathBuf,
    checkpoint: PathBuf,
    _lock: ExclusiveFileLock,
}

impl FileGraphStore {
    pub(crate) fn resolve(run_id: &str) -> Result<Self, String> {
        let root = super::cache::workflow_store_dir()
            .ok_or_else(|| "graph checkpoint directory is unavailable".to_string())?;
        // An explicit directory is caller-owned above its leaf. The default
        // path owns projects/<project>/state/workflows below the config home.
        let owned_suffix = if std::env::var_os("ZO_WORKFLOW_STORE").is_some() {
            1
        } else {
            4
        };
        Self::open(&root, run_id, owned_suffix)
    }

    #[cfg(test)]
    fn at_dir(root: &Path, run_id: &str) -> Result<Self, String> {
        Self::open(root, run_id, 1)
    }

    fn open(root: &Path, run_id: &str, owned_suffix: usize) -> Result<Self, String> {
        if root.as_os_str().is_empty() {
            return Err("graph checkpoint directory must not be empty".to_string());
        }
        if run_id.is_empty()
            || run_id.len() > MAX_RUN_ID_BYTES
            || !run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("graph run id must contain 1–128 ASCII letters, digits, '-' or '_'".to_string());
        }
        let root = if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|_| "graph checkpoint directory is unavailable".to_string())?
                .join(root)
        };
        secure_fs::ensure_private_dir_absolute(&root, owned_suffix)
            .map_err(|_| "graph checkpoint directory is unavailable or unsafe".to_string())?;
        let lock_path = PathBuf::from(format!("{run_id}.graph.lock"));
        let lock = secure_fs::try_lock_owner_only(&root, &lock_path)
            .map_err(|_| "graph run lock is unavailable or unsafe".to_string())?
            .ok_or_else(|| "graph run is already executing in another owner".to_string())?;
        Ok(Self {
            root,
            checkpoint: PathBuf::from(format!("{run_id}.graph.json")),
            _lock: lock,
        })
    }

    fn validate_root(&self) -> Result<(), String> {
        if secure_fs::is_owned_private_dir_absolute(&self.root).unwrap_or(false) {
            Ok(())
        } else {
            Err("graph checkpoint directory changed or is unsafe".to_string())
        }
    }
}

impl GraphStore for FileGraphStore {
    fn load(&self) -> Result<Option<Value>, String> {
        self.validate_root()?;
        let Some(text) = secure_fs::read_regular_file_absolute_no_follow_bounded(
            &self.root.join(&self.checkpoint),
            MAX_CHECKPOINT_BYTES,
        )
        .map_err(|_| "graph checkpoint is unreadable, unsafe, or exceeds 32 MiB".to_string())?
        else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| "graph checkpoint is corrupt; execution cannot safely restart".to_string())
    }

    fn save(&self, checkpoint: &Value) -> Result<(), String> {
        self.validate_root()?;
        let bytes = serde_json::to_vec(checkpoint)
            .map_err(|_| "graph checkpoint could not be encoded".to_string())?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CHECKPOINT_BYTES {
            return Err("graph checkpoint exceeds 32 MiB".to_string());
        }
        // The helper fsyncs the temporary file and parent directory around its
        // atomic rename. A successful save is a durable execution boundary.
        secure_fs::write_atomic_owner_only(&self.root, &self.checkpoint, &bytes)
            .map_err(|_| "graph checkpoint could not be durably saved".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{FileGraphStore, GraphStore, MAX_CHECKPOINT_BYTES};
    use serde_json::json;

    #[test]
    fn graph_checkpoint_round_trips_and_replaces_atomically() {
        let root = tempfile::tempdir().unwrap();
        let store = FileGraphStore::at_dir(root.path(), "run-1").unwrap();
        assert_eq!(store.load().unwrap(), None);
        let first = json!({"cursor":"verify", "in_flight":"build", "steps":1});
        store.save(&first).unwrap();
        assert_eq!(store.load().unwrap(), Some(first));
        let second = json!({"cursor":"end", "in_flight":null, "steps":2});
        store.save(&second).unwrap();
        assert_eq!(store.load().unwrap(), Some(second));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(root.path().join("run-1.graph.json")).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn graph_checkpoint_corruption_is_not_a_cache_miss() {
        let root = tempfile::tempdir().unwrap();
        let store = FileGraphStore::at_dir(root.path(), "run-1").unwrap();
        std::fs::write(root.path().join("run-1.graph.json"), "{broken").unwrap();
        assert!(store.load().unwrap_err().contains("corrupt"));
    }

    #[test]
    fn graph_checkpoint_read_is_bounded() {
        let root = tempfile::tempdir().unwrap();
        let store = FileGraphStore::at_dir(root.path(), "run-1").unwrap();
        let file = std::fs::File::create(root.path().join("run-1.graph.json")).unwrap();
        file.set_len(MAX_CHECKPOINT_BYTES + 1).unwrap();
        assert!(store.load().unwrap_err().contains("32 MiB"));
    }

    #[test]
    fn graph_run_lock_is_exclusive_and_released_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let first = FileGraphStore::at_dir(root.path(), "run-1").unwrap();
        assert!(FileGraphStore::at_dir(root.path(), "run-1")
            .unwrap_err()
            .contains("already executing"));
        let other = FileGraphStore::at_dir(root.path(), "run-2").unwrap();
        drop(first);
        assert!(FileGraphStore::at_dir(root.path(), "run-1").is_ok());
        drop(other);
    }

    #[test]
    fn graph_run_ids_cannot_escape_the_store() {
        let root = tempfile::tempdir().unwrap();
        let invalid = ["", "../run", "/run", "run/name", "a.b", "two words", "실행", "run\\name"];
        for id in invalid {
            assert!(FileGraphStore::at_dir(root.path(), id).is_err(), "{id}");
        }
        assert!(FileGraphStore::at_dir(root.path(), &"x".repeat(129)).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn graph_store_creates_only_its_owned_leaf() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("workflows");
        assert!(FileGraphStore::at_dir(&child, "run-1").is_ok());
        let missing_parent = root.path().join("missing");
        assert!(FileGraphStore::at_dir(&missing_parent.join("workflows"), "run-1").is_err());
        assert!(!missing_parent.exists());
        assert!(FileGraphStore::at_dir(std::path::Path::new(""), "run-1").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn graph_checkpoint_and_lock_symlinks_are_refused() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("untouched");
        std::fs::write(&victim, "sentinel").unwrap();
        symlink(&victim, root.path().join("run-1.graph.json")).unwrap();
        let store = FileGraphStore::at_dir(root.path(), "run-1").unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&json!({"cursor":"end"})).is_err());
        symlink(&victim, root.path().join("run-2.graph.lock")).unwrap();
        assert!(FileGraphStore::at_dir(root.path(), "run-2").is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn graph_store_refuses_symlink_root_and_ancestors() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("linked");
        symlink(outside.path(), &link).unwrap();
        assert!(FileGraphStore::at_dir(&link, "run-1").is_err());
        assert!(FileGraphStore::at_dir(&link.join("nested"), "run-1").is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
