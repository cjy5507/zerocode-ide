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

/// The commands the recipe runs, as `(stage name, command)` in order: the
/// lines that start with `step <name> `. A comment, a message or a hint that
/// spells a command is not a step.
fn steps(body: &str) -> Vec<(String, String)> {
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("step "))
        .filter_map(|rest| rest.split_once(' '))
        .map(|(name, command)| (name.to_string(), command.to_string()))
        .collect()
}

/// Stage one is always: the formatter of every Cargo workspace (the gate's own
/// `fmt-check` recipes, so the stage and the gate cannot disagree), the
/// private-value gate, the window's source contracts. In this order — cheapest
/// first, so the first red comes in seconds. The workspaces after the root are
/// named by `changed_crates.py --workspaces`, one step run per name: the recipe
/// spells none.
#[test]
fn stage_one_runs_the_four_checks_in_order() {
    let body = body();
    let steps = steps(&body);
    let first_four: Vec<(&str, &str)> = steps
        .iter()
        .take(4)
        .map(|(name, command)| (name.as_str(), command.as_str()))
        .collect();
    assert_eq!(
        first_four,
        [
            ("fmt", "just fmt-check"),
            (
                "\"fmt-$workspace\"",
                "bash -c 'cd \"$1\" && just fmt-check' _ \"$workspace\""
            ),
            ("pii", "just pii-check"),
            (
                "contracts",
                "cargo test -p zerocode-shell --test source_contracts"
            ),
        ],
        "stage one's first four steps:\n{body}"
    );
    assert!(
        body.lines()
            .any(|line| line.contains("changed_crates.py --workspaces")),
        "the workspaces come from the script:\n{body}"
    );
    assert!(
        !body.contains("zo-ide"),
        "the second workspace's name lives in the script's WORKSPACES, not in the recipe:\n{body}"
    );
}

/// The workspaces the script names (its `WORKSPACES = (...)` line, the one
/// place that knows them): the root and the others.
fn script_workspaces() -> Vec<String> {
    let path = repo_root().join("tools/release/changed_crates.py");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
    let line = text
        .lines()
        .find(|line| line.starts_with("WORKSPACES = ("))
        .expect("a `WORKSPACES = (...)` line in changed_crates.py");
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The gate's fmt recipes are the ones the stage runs: they exist, in every
/// workspace the script names.
#[test]
fn the_fmt_steps_run_recipes_that_exist() {
    let workspaces = script_workspaces();
    assert!(
        workspaces.len() > 1 && workspaces[0] == ".",
        "the root and at least one more workspace: {workspaces:?}"
    );
    for workspace in workspaces {
        let justfile = if workspace == "." {
            "justfile".to_string()
        } else {
            format!("{workspace}/justfile")
        };
        let path = repo_root().join(&justfile);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|why| panic!("{}: {why}", path.display()));
        assert!(
            text.lines().any(|line| line.starts_with("fmt-check:")),
            "{justfile} has the `fmt-check` recipe the gate and `pre-handin` both run"
        );
    }
}

/// A crate under test also gets its rustdoc built with the gate's own flags
/// (a private intra-doc link was the red the lane hit on 2026-10-28), except
/// the one crate the gate leaves out of `just doc`. Flags and that name are
/// the `doc` recipe's variables, interpolated, so the step and the gate agree.
#[test]
fn a_crate_under_test_gets_its_docs_built_like_the_gate_builds_them() {
    let body = body();
    let steps = steps(&body);
    assert!(
        steps
            .iter()
            .any(|(name, command)| name == "\"doc:$package\""
                && command
                    == "env RUSTDOCFLAGS=\"$doc_flags\" cargo doc -p \"$package\" --no-deps"),
        "a `doc:$package` step runs cargo doc with the flags the gate uses:\n{body}"
    );
    assert!(
        body.lines()
            .any(|line| line.trim() == "doc_flags={{quote(doc_flags)}}")
            && body
                .lines()
                .any(|line| line.trim() == "doc_skip={{quote(doc_skip)}}"),
        "the recipe takes the doc flags and the skipped crate from the justfile's variables:\n{body}"
    );
    let path = repo_root().join("justfile");
    let justfile =
        std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
    assert!(
        justfile.contains(
            "RUSTDOCFLAGS=\"{{doc_flags}}\" cargo doc --workspace --exclude {{doc_skip}} --no-deps"
        ),
        "the gate's `doc` recipe runs on the same variables"
    );
}

/// What the unit stage does not run is said on the last lines, not left for a
/// worker to find out: the crates that depend on a changed crate, and the
/// integration tests other than the contracts.
#[test]
fn the_recipe_says_what_the_unit_stage_does_not_run() {
    let body = body();
    assert!(
        body.lines().any(|line| {
            let line = line.trim();
            line.starts_with("echo ")
                && line.contains("not the crates that depend on them")
                && line.contains("the gate")
        }),
        "a line says dependents and integration tests are the gate's:\n{body}"
    );
}

/// A pick that failed is a window stage that stopped (`2`), with the message
/// above it — not a `-` that reads as "nothing to do".
#[test]
fn a_failed_pick_stops_the_window_stage_instead_of_skipping_it() {
    let body = body();
    assert!(
        body.lines()
            .any(|line| line.trim() == "if [ \"$pick\" -ne 0 ]; then"),
        "the window stage checks the pick first:\n{body}"
    );
}

/// A hint of more than a quarter of the suites says so and carries its count.
#[test]
fn a_broad_suite_hint_is_said_to_be_broad() {
    let body = body();
    assert!(
        body.lines()
            .any(|line| line.trim().starts_with("broad_over=")),
        "the share that makes a hint broad is one named value:\n{body}"
    );
    assert!(
        body.contains("$hinted of $total"),
        "the hint names how many of how many suites:\n{body}"
    );
}

/// The crates to test are the ones the branch changed, chosen from Cargo's
/// own package list by one script that the recipe runs and whose answer it
/// reads — not a list kept in the recipe. A red stage does not stop the
/// others (no `set -e`), and a branch point can be named.
#[test]
fn the_crates_under_test_are_picked_from_the_diff_by_the_script() {
    let body = body();
    assert!(
        body.lines().any(|line| {
            let line = line.trim();
            line.starts_with("picked=$(python3 tools/release/changed_crates.py")
                && line.contains("PRE_HANDIN_BASE")
        }),
        "the recipe runs tools/release/changed_crates.py and passes PRE_HANDIN_BASE as --base:\n{body}"
    );
    assert!(
        body.lines()
            .any(|line| line.contains("read -r kind workspace package arguments")),
        "the recipe reads the script's `crate` lines:\n{body}"
    );
    assert!(
        repo_root()
            .join("tools/release/changed_crates.py")
            .is_file(),
        "tools/release/changed_crates.py exists"
    );
    assert!(
        !body.lines().any(|line| line.trim().starts_with("set -e")),
        "a red stage must not stop the others:\n{body}"
    );
}

/// Stage two never runs the whole window gate: it is 23 minutes on a quiet
/// machine, and every worker doing it would starve the build line. The one
/// step that runs `ui/tests/window.mjs` names its suites; any other line that
/// spells it is a message.
#[test]
fn stage_two_never_runs_the_whole_window_gate() {
    let body = body();
    let runs: Vec<(String, String)> = steps(&body)
        .into_iter()
        .filter(|(_, command)| command.contains("ui/tests/window.mjs"))
        .collect();
    assert!(
        !runs.is_empty(),
        "stage two runs ui/tests/window.mjs:\n{body}"
    );
    for (name, command) in &runs {
        assert!(
            command.contains("WINDOW_SUITES="),
            "step `{name}` runs the window without WINDOW_SUITES, which is the whole gate: {command}"
        );
    }
    for line in body.lines().map(str::trim) {
        if line.contains("node ui/tests/window.mjs") && !line.starts_with("step ") {
            assert!(
                line.starts_with("hint=") || line.starts_with("echo "),
                "a line that is not a step spells the window run: {line}"
            );
        }
    }
}
