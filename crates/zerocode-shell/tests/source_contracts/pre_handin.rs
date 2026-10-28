//! `just pre-handin`: what a worker runs before `worker_done`. On 2026-10-28
//! four gate reds (1 h 43 m of the lane) were each caught beforehand by a
//! check of 4 s to 23 min — the formatter, the contracts, a crate's unit
//! tests, the window's suites. The recipe is those checks in two stages, and
//! this holds it to the commands the stages are made of.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The body of the `pre-handin` recipe: the indented lines under its header,
/// up to the first line that starts in column 0.
fn recipe_body(justfile: &str) -> String {
    let mut lines = justfile.lines();
    let header = lines
        .by_ref()
        .find(|line| line.starts_with("pre-handin") && line.contains(':'))
        .expect("a `pre-handin` recipe in the justfile");
    assert!(
        !header.contains(":="),
        "`pre-handin` is a recipe, not a variable: {header}"
    );
    lines
        .take_while(|line| line.is_empty() || line.starts_with(char::is_whitespace))
        .collect::<Vec<_>>()
        .join("\n")
}

fn body() -> String {
    let path = repo_root().join("justfile");
    let justfile =
        std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
    recipe_body(&justfile)
}

/// Stage one is always: the formatter of both workspaces, the private-value
/// gate, the window's source contracts. Each is spelled out in the recipe
/// (the gate's own words), in this order — cheapest first, so the first red
/// comes in seconds.
#[test]
fn stage_one_runs_the_four_checks_in_order() {
    let body = body();
    let mut from = 0;
    for command in [
        "cargo fmt --all -- --check",
        "cd zo-ide && cargo fmt --all -- --check",
        "just pii-check",
        "cargo test -p zerocode-shell --test source_contracts",
    ] {
        let at = body[from..]
            .find(command)
            .unwrap_or_else(|| panic!("`{command}` after byte {from} of the recipe:\n{body}"));
        from += at + command.len();
    }
}

/// The crates to test are the ones the branch changed, chosen from Cargo's
/// own package list by one script — not a list kept in the recipe.
#[test]
fn the_crates_under_test_are_picked_from_the_diff_by_the_script() {
    let body = body();
    assert!(
        body.contains("tools/release/changed_crates.py"),
        "the recipe picks the changed crates with tools/release/changed_crates.py:\n{body}"
    );
    assert!(
        repo_root()
            .join("tools/release/changed_crates.py")
            .is_file(),
        "tools/release/changed_crates.py exists"
    );
}

/// Stage two never runs the whole window gate: it is 23 minutes on a quiet
/// machine, and every worker doing it would starve the build line. Every
/// line that runs `ui/tests/window.mjs` names its suites.
#[test]
fn stage_two_never_runs_the_whole_window_gate() {
    let body = body();
    let runs: Vec<&str> = body
        .lines()
        .filter(|line| line.contains("ui/tests/window.mjs"))
        .collect();
    assert!(
        !runs.is_empty(),
        "stage two runs ui/tests/window.mjs:\n{body}"
    );
    for line in runs {
        assert!(
            line.contains("WINDOW_SUITES"),
            "a window run without WINDOW_SUITES is the whole gate: {line}"
        );
    }
}
