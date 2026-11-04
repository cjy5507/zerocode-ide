//! The recipes folder is the person's (t-93871).
//!
//! Three recipe documents (a bench's fixture Flow and two end-to-end probes) were
//! found in a person's real recipes folder, saved there by runs against the
//! installed window. No test of this repository wrote them — and these hold that
//! none can: the one place that points the recipe store (the evidence root, whose
//! `computer-use/recipes` folder it is) at the real data root is the window's
//! own runtime, and every test that sets it names a folder it just made.
//!
//! What the card does about the files that are already there is its own tests'
//! (`computer_use::recipes::register`) and the window suite's: the person deletes
//! them, one question each, and nothing deletes them for the person.

use std::path::{Path, PathBuf};

use super::quiet_children::code_only;
use super::support::strip_rust_comments;

/// The call this contract is about, as every caller spells it: the definition is
/// `pub fn set_root(`, which is not a call and does not match.
const CALL: &str = "evidence::set_root(";

/// A file's shipped code, and what is test: a module that stands last in a file
/// (the rule of the repository) and the files that are only tests.
fn halves(path: &Path, text: &str) -> (String, String) {
    let only_tests = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("tests"))
        || path.components().any(|part| part.as_os_str() == "tests");
    if only_tests {
        return (String::new(), strip_rust_comments(text));
    }
    let cut = text.rfind("#[cfg(test)]\nmod ").unwrap_or(text.len());
    let (shipped, tests) = text.split_at(cut);
    (code_only(shipped), strip_rust_comments(tests))
}

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("src reads").flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn only_the_windows_own_runtime_points_the_recipe_store_at_the_real_data_root() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 100, "the backend's sources were not found");
    let mut shipped_callers: Vec<String> = Vec::new();
    let mut test_calls = 0usize;
    for file in &files {
        let text = std::fs::read_to_string(file).expect("a source reads");
        let name = file
            .strip_prefix(&src)
            .unwrap_or(file)
            .display()
            .to_string();
        let (shipped, tests) = halves(file, &text);
        if shipped.contains(CALL) {
            shipped_callers.push(name.clone());
        }
        for (at, _) in tests.match_indices(CALL) {
            test_calls += 1;
            let rest = &tests[at + CALL.len()..];
            let argument = &rest[..rest.find(';').unwrap_or(rest.len())];
            assert!(
                argument.contains(".path()"),
                "{name}: a test points the recipe store at `{argument}` — a test names a folder it made (a tempdir's `.path()`), never a path that could be the person's data root"
            );
        }
    }
    assert_eq!(
        shipped_callers,
        ["agent_tools_runtime.rs"],
        "the recipe store's root is set by the window's own runtime and nowhere else that ships"
    );
    assert!(
        test_calls >= 3,
        "the tests that set the root were not found"
    );
}
