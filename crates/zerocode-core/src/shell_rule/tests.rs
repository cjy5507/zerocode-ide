//! The rules on the words people's commands hold (t-10916): the window reads
//! every shell command an agent pane runs with them, so a command no test
//! spelled is one they meet.

use super::*;

/// A value after `KEY=` is read where it stands in the command: whitespace
/// before it is skipped in the command itself, not only in a trimmed copy —
/// so the command after it is the first one, and a word of letters wider
/// than a byte after the space is never cut in two (a transcript on this
/// machine held `…= (규제 준수 현황 cards, …)`, and the rules panicked on it).
#[test]
fn an_assignments_value_is_read_where_it_stands_in_the_command() {
    assert_eq!(extract_first_command("KEY=value ls"), "ls");
    assert_eq!(extract_first_command("KEY=\"a b\" ls -la"), "ls");
    assert_eq!(extract_first_command("KEY= value ls"), "ls");
    assert_eq!(extract_first_command("KEY= \"a b\" ls"), "ls");
    let wide = "SUMMARY= (규제 준수 현황 cards, 망분리 · 개인정보, KPI bands). cat notes.md";
    assert_eq!(extract_first_command(wide), "준수");
    // Read through whole by the reader the window's pane guard asks — its
    // verdict is the table's (a name it does not list is not a write).
    let _ = proven_read_only(wide);
}

/// A run is a check and names no project gate; a gate names itself however
/// the command around it is written, and a command that checks nothing —
/// the `sleep 1` the dreamer once filed as this project's gate — names none.
#[test]
fn a_command_names_the_project_gates_it_runs_and_a_run_names_none() {
    let chained = "cargo fmt --all --check && C=/tmp/c.sh; $C cargo clippy -p x -- -D warnings \
                   && cargo test -p x --lib > /tmp/t.log 2>&1";
    assert_eq!(
        project_checks_in(chained).collect::<Vec<_>>(),
        ["cargo clippy", "cargo test"]
    );
    assert_eq!(
        project_checks_in("cargo build --tests").collect::<Vec<_>>(),
        ["cargo build"]
    );
    assert_eq!(
        project_checks_in("npx tsc --noEmit").collect::<Vec<_>>(),
        ["tsc"]
    );
    assert_eq!(project_checks_in("cat tsconfig.json").count(), 0);
    for run in [
        "cargo run -p zo",
        "python3 -m json.tool < a.json",
        "npm run dev",
    ] {
        assert!(command_is_check_shaped(run), "{run} stopped being a check");
        assert_eq!(project_checks_in(run).count(), 0, "{run} named a gate");
    }
    assert!(!command_is_check_shaped("sleep 1"));
    assert_eq!(project_checks_in("sleep 1").count(), 0);
}
