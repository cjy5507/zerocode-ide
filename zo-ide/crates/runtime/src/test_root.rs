//! A test's own folder under the temporary folder, removed when the test ends
//! however it ends (2026-11-10). The tests' helpers handed out a bare path, and
//! every test that did not remove its own, or panicked before it did, left it
//! behind: 13,872 `runtime-config-*`, 2,377 `runtime-prompt-import-*` and 395
//! `runtime-session-control-*` folders in one Mac's temporary folder.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A path under the temporary folder that no other test of any process holds —
/// the prefix, the pid, the clock and a counter — removed with all it holds when
/// it is dropped. Making the folder is the test's.
pub(crate) struct TestRoot(PathBuf);

impl TestRoot {
    pub(crate) fn new(prefix: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("{prefix}-{}-{nanos}-{counter}", std::process::id());
        Self(std::env::temp_dir().join(name))
    }
}

impl std::ops::Deref for TestRoot {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TestRoot {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::TestRoot;

    #[test]
    fn the_folder_goes_when_its_test_ends_and_even_when_it_panics() {
        let kept = {
            let root = TestRoot::new("runtime-test-root");
            std::fs::create_dir_all(root.join("a/b")).expect("a folder in the root");
            std::fs::write(root.join("a/b/c.txt"), "c").expect("a file in it");
            root.to_path_buf()
        };
        assert!(!kept.exists(), "{} stayed", kept.display());

        let path = std::sync::Mutex::new(None);
        let panicked = std::panic::catch_unwind(|| {
            let root = TestRoot::new("runtime-test-root");
            std::fs::create_dir_all(&root).expect("the root");
            *path.lock().expect("the path") = Some(root.to_path_buf());
            panic!("a test that fails before its own cleanup");
        });
        assert!(panicked.is_err());
        let held = path.lock().expect("the path").clone().expect("the root was made");
        assert!(!held.exists(), "{} stayed after a panic", held.display());
    }

    #[test]
    fn two_roots_of_one_prefix_are_two_folders() {
        let (a, b) = (TestRoot::new("runtime-test-root"), TestRoot::new("runtime-test-root"));
        assert_ne!(a.to_path_buf(), b.to_path_buf());
        let name = a.file_name().map(|name| name.to_string_lossy().into_owned());
        assert!(name.is_some_and(|name| name.starts_with("runtime-test-root-")));
    }
}
