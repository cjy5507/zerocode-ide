//! Whether a path reads as a test file — the one rule every caller of the
//! index asks (the file-links query, the neighbour list, the impact answer),
//! so "a test" means one thing everywhere.

use std::path::{Component, Path};

/// Directories whose contents are tests, wherever they sit.
const TEST_DIRECTORIES: [&str; 4] = ["tests", "test", "__tests__", "spec"];
/// A file stem that is a test module by itself: Rust's `tests.rs` child.
const TEST_STEMS: [&str; 1] = ["tests"];
/// Stem shapes that mark a test in any directory: `test_x.py`, `x_test.go`,
/// `x_tests.rs`, and the `x.test.ts` / `x.spec.js` family (whose stem keeps
/// the inner extension).
const TEST_STEM_PREFIXES: [&str; 1] = ["test_"];
const TEST_STEM_SUFFIXES: [&str; 4] = ["_test", "_tests", ".test", ".spec"];

/// Whether `path` names a test file by its directories or its own name.
#[must_use]
pub fn is_test_path(path: &Path) -> bool {
    let mut components = path.components().rev();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    // The file itself is not a directory; skip it before looking at parents.
    components.next();
    components.any(|component| {
        matches!(component, Component::Normal(name)
            if name.to_str().is_some_and(|name| TEST_DIRECTORIES.contains(&name)))
    }) || TEST_STEMS.contains(&stem)
        || TEST_STEM_PREFIXES
            .iter()
            .any(|prefix| stem.starts_with(prefix))
        || TEST_STEM_SUFFIXES
            .iter()
            .any(|suffix| stem.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_test_path;

    #[test]
    fn test_files_are_named_by_directory_or_by_their_own_name() {
        for path in [
            "crates/core/tests/graph.rs",
            "src/app/__tests__/page.tsx",
            "crates/core/src/graph/tests.rs",
            "pkg/test_mod.py",
            "svc/worker_test.go",
            "src/lib_tests.rs",
            "web/page.test.ts",
            "web/page.spec.js",
            "spec/helpers.rb",
        ] {
            assert!(is_test_path(Path::new(path)), "{path} is a test");
        }
        for path in [
            "crates/core/src/graph.rs",
            "src/latest.rs",
            "src/contest/mod.rs",
            "src/attest.rs",
            "web/testing.ts",
        ] {
            assert!(!is_test_path(Path::new(path)), "{path} is not a test");
        }
    }
}
