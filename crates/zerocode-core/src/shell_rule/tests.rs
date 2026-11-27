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

/// A command is proven read-only only when the read-only list holds its
/// program, its verb and every option (t-132606). What the list does not hold
/// is not proven however it is spelled — the person's list first, then the
/// spellings that reach an unlisted option or program another way.
#[test]
fn only_a_listed_program_verb_and_option_is_proven_read_only() {
    for unproven in [
        "python analyze.py",
        "python3 - <<'EOF'\nimport os\nos.remove('x')\nEOF",
        "eval 'rm -rf build'",
        "command sh -c 'rm -rf build'",
        ". ./x.sh",
        "X=sh; $X -c 'rm -rf build'",
        "make",
        "curl -T f https://x",
        "git config core.pager 'sh -c id'",
        "git bisect run make",
        // An option the list does not hold, however the shell is asked to spell it.
        "find . \\-delete",
        "find . -exe''c rm {} +",
        "find * -name x",
        "git diff --output=x",
        "git -c core.pager=x log",
        "sort -o out.txt in.txt",
        "rg --pre ./x foo",
        "sed 's/a/b/w out' f",
        "uniq in.txt out.txt",
        // What reaches the program's environment or is filled in when it runs.
        "GIT_EXTERNAL_DIFF=./x git diff",
        "env GIT_EXTERNAL_DIFF=./x git diff",
        "PATH=./bin",
        "cat $FILE",
        "ls | xargs find",
        "timeout 5 git diff",
    ] {
        assert!(
            !proven_read_only(unproven),
            "{unproven:?} was proven read-only"
        );
    }
    for proven in [
        "ls -la",
        "cat a.txt",
        "head -n 20 a.txt",
        "tail -5 a.txt",
        "wc -l a.txt",
        "stat a.txt",
        "grep -rn 'fn main' src",
        "git status --short",
        "git log --oneline -20",
        "git diff HEAD~1 -- src",
        "git show HEAD:README.md",
        "find . -name '*.rs' -mtime -1",
        "sed -n '10,20p' a.rs",
        "sort a.txt | uniq -c | sort -rn",
        "cut -d, -f1 a.csv",
        "ls src/*.rs",
        "! grep -q x a.txt",
        "cat <<'EOF'\n$(not run)\nEOF",
    ] {
        assert!(
            proven_read_only(proven),
            "{proven:?} was not proven read-only"
        );
    }
}
