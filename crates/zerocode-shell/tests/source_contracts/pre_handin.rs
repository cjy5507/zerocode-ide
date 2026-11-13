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
/// private-value gate, the quick Node checks, then the window's source contracts — cheapest
/// first, so the first red comes in seconds. The workspaces after the root are
/// named by `changed_crates.py --workspaces`, one step run per name: the recipe
/// spells none.
#[test]
fn stage_one_runs_quick_checks_before_the_source_contract_build() {
    let body = body();
    let steps = steps(&body);
    let first_three: Vec<(&str, &str)> = steps
        .iter()
        .filter(|(name, _)| !name.contains("lock"))
        .take(3)
        .map(|(name, command)| (name.as_str(), command.as_str()))
        .collect();
    assert_eq!(
        first_three,
        [
            ("fmt", "just fmt-check"),
            (
                "\"fmt-$workspace\"",
                "bash -c 'cd \"$1\" && just fmt-check' _ \"$workspace\""
            ),
            ("pii", "just pii-check"),
        ],
        "formatting and privacy checks precede other stage-one commands:\n{body}"
    );
    let position = |name: &str, command: &str| {
        steps
            .iter()
            .position(|(found, run)| found == name && run == command)
            .unwrap_or_else(|| panic!("no stage {name}: {command}\n{body}"))
    };
    let privacy = position("pii", "just pii-check");
    let contracts = position(
        "contracts",
        "cargo test -p \"$contracts_package\" --test \"$contracts_target\"",
    );
    for (name, command) in [
        ("package-runner", "just package-runner-test"),
        ("quality-runner", "just quality-runner-test"),
        ("window-runner", "just window-runner-test"),
    ] {
        let runner = position(name, command);
        assert!(
            privacy < runner && runner < contracts,
            "{name} must run after privacy checks and before Rust builds:\n{body}"
        );
    }
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

/// A crate under test also gets the gate's own rustdoc, lint and integration
/// tests (a private intra-doc link was the red the lane hit on 2026-10-28; on
/// v1.1.149 nine more reds came up on the stacked tree that these three would
/// have shown first). The commands are the script's `doc`, `clippy` and `tests`
/// lines — the gate's recipes narrowed to the crate — so the recipe runs the
/// line and spells no flag, no crate and no cargo subcommand of its own.
#[test]
fn a_crate_under_test_gets_the_gates_own_doc_lint_and_integration_commands() {
    let body = body();
    let steps = steps(&body);
    assert!(
        steps
            .iter()
            .any(|(name, command)| name == "\"$kind:$package\""
                && command == "bash -c 'cd \"$1\" && eval \"$2\"' _ \"$workspace\" \"$arguments\""),
        "a `$kind:$package` step runs the script's command line in the workspace:\n{body}"
    );
    let path = repo_root().join("tools/release/changed_crates.py");
    let script =
        std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()));
    let kinds = script
        .lines()
        .find(|line| line.starts_with("GATE_KINDS = {"))
        .expect("a `GATE_KINDS = {...}` line in changed_crates.py");
    for kind in ["doc", "clippy", "tests"] {
        assert!(
            kinds.contains(&format!("\"{kind}\"")),
            "the script prints a `{kind}` fact: {kinds}"
        );
    }
    assert!(
        body.lines().any(|line| line.trim() == "doc|clippy|tests)"),
        "the recipe reads the three facts the script prints:\n{body}"
    );
    assert!(
        body.lines()
            .any(|line| line.trim().starts_with("for kind in doc clippy tests;")),
        "a check no crate owed is noted `-`, so \"not owed\" and \"left out\" read differently:\n{body}"
    );
    assert!(
        body.lines()
            .any(|line| line.trim().starts_with("done 3<<< \"$picked\"")),
        "the crates are read from their own descriptor, so a test that reads stdin cannot eat the rest:\n{body}"
    );
    for spelled in [
        "cargo doc",
        "cargo clippy",
        "RUSTDOCFLAGS",
        "-D warnings",
        "doc_flags",
        "doc_skip",
    ] {
        assert!(
            !body.contains(spelled),
            "the recipe spells `{spelled}`; the gate's recipes say it and the script reads them:\n{body}"
        );
    }
}

/// Stage one runs the window's source contracts, which are an integration test
/// target of the window's own crate. A branch that changes that crate would owe
/// the same target again as its integration tests (71 s of a measured 100 s run
/// on 2026-11-14), so the recipe says which target ran and the script leaves it
/// out. The two names are the recipe's own variables, and they name a real target.
#[test]
fn the_contracts_stage_is_not_run_a_second_time_as_a_crates_integration_tests() {
    let body = body();
    let value = |name: &str| -> String {
        body.lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no `{name}=` line:\n{body}"))
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let (package, target) = (value("contracts_package"), value("contracts_target"));
    assert!(
        body.lines().any(|line| line.trim().starts_with("picked=")
            && line.contains("--ran-test \"$contracts_package:$contracts_target\"")),
        "the script is told which target stage one ran:\n{body}"
    );
    let manifest = repo_root().join("crates").join(&package).join("Cargo.toml");
    assert!(manifest.is_file(), "`{package}` is a crate here");
    let tests = repo_root().join("crates").join(&package).join("tests");
    assert!(
        tests.join(format!("{target}.rs")).is_file()
            || tests.join(&target).join("main.rs").is_file(),
        "`{target}` is an integration test target of `{package}`"
    );
}

/// What the crate stages do not run is said on the last lines, not left for a
/// worker to find out: the crates that depend on a changed crate, and the doc
/// tests.
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

/// A dependency the root's crate takes changes the lock of a workspace that
/// reads that crate by path, with no path of that workspace in the diff: on
/// 2026-11-02 v1.1.115's gate stopped on zo-ide's lock (t-89036, t-84719). So
/// every workspace's lock is asked, whichever paths the branch changed. It takes
/// under a second and builds nothing; the answer it prints is not wanted.
#[test]
fn every_workspaces_lock_is_checked_whichever_paths_changed() {
    let body = body();
    let at = |line: &str| {
        body.find(line)
            .unwrap_or_else(|| panic!("no line {line}:\n{body}"))
    };
    assert!(
        at("lock_step lock .") < at("step contracts"),
        "the lock is asked before the steps that build:\n{body}"
    );
    at("lock_step \"lock-$workspace\" \"$workspace\"");
}

/// t-84719 r1 P2-1: a crate this machine never fetched is not a lock behind its manifest. Only
/// `lock_answer.py`, which holds prebatch's own verdict, runs the command, and what it could
/// not answer is a stage that did not apply, said aloud: the line then asks nobody to commit a
/// lock that did not change.
#[test]
fn the_lock_stage_takes_its_verdict_from_prebatch_and_a_no_answer_is_not_red() {
    let body = body();
    assert!(
        !body.contains("cargo metadata --locked"),
        "the recipe holds no second verdict on that command:\n{body}"
    );
    let stage = body
        .split("lock_step() {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }").next())
        .unwrap_or_else(|| panic!("a lock_step function:\n{body}"));
    assert!(
        stage.contains("python3 tools/release/lock_answer.py"),
        "{stage}"
    );
    assert!(stage.contains("no answer here"), "{stage}");
    assert!(stage.contains("code=-"), "{stage}");
}

/// t-89474 r1 P3-2: the words the ledger refuses a moved result with are what the window
/// recognises to say it to a person, and both roads to 통과 go through that reading.
#[test]
fn the_window_reads_the_ledgers_words_for_a_result_that_moved() {
    let words = zerocode_core::orchestration::RESULT_MOVED_WORDS;
    let now = std::fs::read_to_string(repo_root().join("ui/shell-board-now.js")).expect("board");
    assert!(
        now.contains(&format!("BOARD_RESULT_MOVED = \"{words}\"")),
        "ui/shell-board-now.js spells the ledger's words"
    );
    assert!(
        now.contains("press.error = boardRefusalWords(error)"),
        "the board's press"
    );
    let inbox = std::fs::read_to_string(repo_root().join("ui/shell-inbox.js")).expect("inbox");
    assert!(
        inbox.contains("held.error = boardRefusalWords(error)"),
        "the inbox's press"
    );
}
