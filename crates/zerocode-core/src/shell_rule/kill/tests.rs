//! The kill guard's rules (t-80602), on a process table written here: the
//! window, the calling pane, another pane and a detached process. Nothing is
//! signalled — the table is letters.

use super::*;

const WINDOW: u32 = 100;
const CALLER: u32 = 200;
const CALLER_CHILD: u32 = 220;
const OTHER_PANE: u32 = 300;
const OTHER_AGENT: u32 = 310;
const DETACHED: u32 = 500;

fn row(pid: u32, ppid: u32, pgid: u32, command: &str) -> ProcessRow {
    ProcessRow {
        pid,
        ppid,
        pgid,
        command: command.to_string(),
    }
}

/// The machine of the 10-28 incident, in miniature: a window with two
/// panes, an agent in each, a dev server under the calling pane's agent,
/// and a process the calling pane started and set loose.
fn table() -> ProcessTable {
    ProcessTable::new(vec![
        row(1, 0, 1, "/sbin/launchd"),
        row(
            WINDOW,
            1,
            WINDOW,
            "/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell",
        ),
        row(CALLER, WINDOW, CALLER, "-zsh"),
        row(
            210,
            CALLER,
            210,
            "claude --permission-mode bypassPermissions",
        ),
        row(
            CALLER_CHILD,
            210,
            CALLER_CHILD,
            "node dev-server.js --port 3000",
        ),
        row(OTHER_PANE, WINDOW, OTHER_PANE, "-zsh"),
        row(
            OTHER_AGENT,
            OTHER_PANE,
            OTHER_AGENT,
            "zo zo-step run --lane foo",
        ),
        row(DETACHED, 1, DETACHED, "python3 detached-worker.py"),
    ])
}

fn protected() -> Vec<ProtectedRoot> {
    vec![
        ProtectedRoot::window(WINDOW),
        ProtectedRoot {
            pid: OTHER_PANE,
            label: "pane term-2".to_string(),
        },
    ]
}

fn judged(command: &str) -> KillVerdict {
    judge_kill(&kill_calls(command), &table(), Some(CALLER), &protected())
}

#[track_caller]
fn denied(command: &str) -> String {
    match judged(command) {
        KillVerdict::Deny { reason } => reason,
        other => panic!("{command:?} should be refused, was {other:?}"),
    }
}

#[track_caller]
fn asked(command: &str) -> String {
    match judged(command) {
        KillVerdict::Ask { reason } => reason,
        other => panic!("{command:?} should be asked of a person, was {other:?}"),
    }
}

#[track_caller]
fn allowed(command: &str) {
    assert_eq!(judged(command), KillVerdict::Allow, "{command:?}");
}

/// The incident itself: an example in backticks inside a double-quoted
/// prompt runs as a command, and every word after `pkill -f` is a pattern.
#[test]
fn a_backtick_example_inside_a_double_quoted_prompt_is_refused() {
    let command = "zerocode-orc worker-start --agent claude --prompt \"고칠 것: `pkill -f zo-step foo` 같은 줄은 쓰지 말 것\"";
    let calls = kill_calls(command);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].written, "pkill -f zo-step foo");
    let pattern = |text: &str| {
        KillTarget::Matching(ProcessPattern {
            text: text.to_string(),
            full: true,
            exact: false,
            ignore_case: false,
            regex: true,
        })
    };
    assert_eq!(calls[0].targets, vec![pattern("zo-step"), pattern("foo")]);
    let reason = denied(command);
    assert!(reason.contains("pkill -f zo-step foo"), "{reason}");
    assert!(
        reason.contains("pid 310") && reason.contains("pane term-2"),
        "{reason}"
    );
}

#[test]
fn a_substitution_in_double_quotes_xargs_and_kill_of_pgrep_are_refused() {
    for command in [
        "echo \"done: $(pkill -f zo-step)\"",
        "pgrep -f zo-step | xargs kill",
        "pgrep -f zo-step | xargs -n 1 kill -9",
        "kill $(pgrep -f zo-step)",
        "kill -9 `pgrep -f zo-step`",
        "ps aux | grep zo-step | grep -v grep | awk '{print $2}' | xargs kill",
        "sudo pkill -9 -f zo-step",
        "/usr/bin/pkill -f zo-step",
        "nohup pkill zo &",
        "killall zo",
        "sh -c 'pkill -f zo-step'",
        "bash -lc \"pkill -f zo-step\"",
        "eval 'pkill -f zo-step'",
        "bash <<'EOF'\npkill -f zo-step\nEOF",
        "cat <<EOF\n`pkill -f zo-step`\nEOF",
        "X=$(pkill -f zo-step) true",
        "if true; then pkill -f zo-step; fi",
        "(pkill -f zo-step)",
        "echo ok && pkill -f zo-step || true",
        "echo 'pkill -f zo-step' | sh",
        "cat <<'EOF' | bash\npkill -f zo-step\nEOF",
        "cat <<EOF | sh -s\npkill -f zo-step\nEOF",
        "cat - <<'EOF' | sudo bash\npkill -f zo-step\nEOF",
        "kill 310",
        "kill -- -310",
        "pkill -P 300",
    ] {
        denied(command);
    }
}

#[test]
fn the_window_itself_is_refused_by_name_and_by_number() {
    let reason = denied("pkill -f zerocode-shell");
    assert!(reason.contains("the ZeroCode window"), "{reason}");
    assert!(denied("kill 100").contains("the ZeroCode window"));
    // Every process: the first kept one met is the window. A lone `-1` is
    // the signal (HUP) with nothing to send it to.
    assert!(denied("kill -9 -1").contains("the ZeroCode window"));
    assert!(denied("kill -- -1").contains("the ZeroCode window"));
    assert_eq!(kill_calls("kill -1")[0].targets, Vec::new());
    denied("pkill -u nobody");
    denied("pkill -v -f nothing-by-this-name");
}

/// Letters that only look like a kill are not a call: a single-quoted
/// example, a commit message, a quoted here-document, a word after `echo`.
#[test]
fn a_kill_that_is_only_letters_is_allowed_and_reads_no_table() {
    for command in [
        "echo 'pkill -f x'",
        "git commit -m 'pkill 설명'",
        "git commit -m \"pkill -f 는 쓰지 말 것\"",
        "git commit -m \"$(cat <<'EOF'\nfix: `pkill -f zo-step` 사고\nEOF\n)\"",
        "printf '%s\\n' \"\\`pkill -f zo-step\\`\"",
        "grep -rn pkill crates/",
        "rg 'kill_calls' src",
        "ls skills/",
        "cat notes.md # pkill -f zo-step",
        "command -v pkill",
        "kill -l",
        "kill -0 310",
        "pkill -0 -f zo-step",
        "killall -s zo",
        "cat <<'EOF' > notes.md\npkill -f zo-step\nEOF",
    ] {
        assert_eq!(kill_calls(command), Vec::new(), "{command:?}");
        allowed(command);
    }
}

#[test]
fn the_callers_own_children_are_its_own_to_end() {
    allowed("kill 220");
    allowed("kill -TERM 220");
    allowed("pkill -f dev-server");
    allowed("kill $(pgrep -f dev-server)");
    allowed("kill -- -220");
    // Nothing matches: nothing is signalled.
    allowed("pkill -f nothing-runs-by-this-name");
    allowed("kill 99999");
}

#[test]
fn the_calling_shells_own_jobs_need_no_table() {
    for command in ["kill %1", "kill $!", "kill ${!}", "kill -9 $$", "kill 0"] {
        let calls = kill_calls(command);
        assert_eq!(calls.len(), 1, "{command:?}");
        assert!(!calls[0].needs_table(), "{command:?}: {calls:?}");
        assert_eq!(
            judge_kill(
                &calls,
                &ProcessTable::unreadable("not read"),
                Some(CALLER),
                &protected()
            ),
            KillVerdict::Allow,
            "{command:?}"
        );
    }
}

#[test]
fn a_process_set_loose_outside_every_pane_is_asked() {
    let reason = asked("kill 500");
    assert!(reason.contains("pid 500"), "{reason}");
    asked("pkill -f detached-worker");
}

#[test]
fn pids_known_only_at_run_time_are_asked() {
    for command in [
        "kill $SERVER_PID",
        "kill $(cat server.pid)",
        "lsof -ti :3000 | xargs kill",
        "pkill -F server.pid",
        "sh -c \"pkill -f $NAME\"",
        "kill ${!ref}",
    ] {
        asked(command);
    }
}

#[test]
fn a_table_that_did_not_read_is_asked_never_allowed() {
    let calls = kill_calls("pkill -f zo-step");
    let verdict = judge_kill(
        &calls,
        &ProcessTable::unreadable("ps timed out"),
        Some(CALLER),
        &protected(),
    );
    match verdict {
        KillVerdict::Ask { reason } => assert!(reason.contains("ps timed out"), "{reason}"),
        other => panic!("an unread table should ask, was {other:?}"),
    }
}

/// A pane the window cannot place owns nothing: what it would signal inside
/// a kept tree is still refused.
#[test]
fn an_unknown_caller_cannot_claim_the_window_as_its_own() {
    let calls = kill_calls("kill 220");
    let verdict = judge_kill(&calls, &table(), None, &protected());
    assert!(matches!(verdict, KillVerdict::Deny { .. }), "{verdict:?}");
}

/// A command without the letters is never read — the guard costs it
/// nothing, and the hook's answer for it stays what it was.
#[test]
fn a_command_without_kill_is_not_read() {
    for command in [
        "cargo test -p zerocode-core shell_rule",
        "git status && git log --oneline -5",
        "echo \"$(date)\"",
    ] {
        assert!(!might_signal(command), "{command:?}");
        assert_eq!(kill_calls(command), Vec::new(), "{command:?}");
    }
    assert!(might_signal("PKILL -f x"));
}

/// `pgrep` and `killall` match a process's name, and a login shell's name
/// is its program's: the `-` its command line opens with is not part of it.
#[test]
fn a_login_shells_name_is_its_program_without_the_dash() {
    assert_eq!(row(1, 0, 1, "-zsh").name(), "zsh");
    assert_eq!(row(1, 0, 1, "/bin/zsh -l").name(), "zsh");
    for command in ["killall zsh", "pkill -x zsh", "killall -9 zsh"] {
        let reason = denied(command);
        assert!(
            reason.contains("pid 300") && reason.contains("pane term-2"),
            "{command:?}: {reason}"
        );
    }
}

#[test]
fn a_pattern_that_does_not_read_is_asked() {
    asked("pkill -f 'zo-step('");
}

#[test]
fn killall_matches_whole_names_and_regex_only_when_asked() {
    // `zo` is a whole name; `z` is not.
    allowed("killall z");
    denied("killall -m '^z'");
    denied("killall -9 zo");
}

/// The table a host reads: `ps` text, the listing's own `ps` left out, and
/// the two trees a host fills its kept roots from.
#[test]
fn a_listing_reads_into_a_table_and_its_trees() {
    let listing = "\
    1     0     1 /sbin/launchd
  100     1   100 /Applications/ZeroCode.app/Contents/MacOS/zerocode-shell
  200   100   200 -zsh
  210   200   210 claude --resume abc
  220   210   220 node dev-server.js
  300   100   300 -zsh
  999   210   999 ps -ww -eo pid=,ppid=,pgid=,command=
not a row
";
    let read = ProcessTable::from_listing(listing, Some(999));
    assert_eq!(
        read,
        ProcessTable::new(vec![
            row(1, 0, 1, "/sbin/launchd"),
            row(
                100,
                1,
                100,
                "/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell"
            ),
            row(200, 100, 200, "-zsh"),
            row(210, 200, 210, "claude --resume abc"),
            row(220, 210, 220, "node dev-server.js"),
            row(300, 100, 300, "-zsh"),
        ])
    );
    assert_eq!(read.ancestors(220), vec![210, 200, 100, 1]);
    assert_eq!(read.tree_of(200), vec![200, 210, 220]);
    assert_eq!(read.tree_of(100).len(), 5);
    let unread = ProcessTable::unreadable("no ps");
    assert!(unread.ancestors(220).is_empty() && unread.tree_of(200).is_empty());
}

/// A host inside a pane (zo) is told the window's pid by its environment and
/// keeps the window and every process the window started but its own pane.
#[test]
fn a_host_inside_a_pane_keeps_the_window_and_the_other_panes() {
    let env = |name: &str| (name == WINDOW_PID_ENV).then(|| "100".to_string());
    let kept = ProtectedRoot::from_window_env(env, &table(), 210);
    assert_eq!(
        kept.iter().map(|root| root.pid).collect::<Vec<_>>(),
        vec![WINDOW, OTHER_PANE],
        "{kept:?}"
    );
    assert_eq!(kept[0], ProtectedRoot::window(WINDOW));
    let calls = kill_calls("pkill -f zo-step");
    assert!(matches!(
        judge_kill(&calls, &table(), Some(210), &kept),
        KillVerdict::Deny { .. }
    ));
    assert_eq!(
        judge_kill(&kill_calls("kill 220"), &table(), Some(210), &kept),
        KillVerdict::Allow
    );
    // Outside a window, or with a value that is not a pid, nothing is kept.
    assert!(ProtectedRoot::from_window_env(|_| None, &table(), 210).is_empty());
    let garbled = |_: &str| Some("not-a-pid".to_string());
    assert!(ProtectedRoot::from_window_env(garbled, &table(), 210).is_empty());
    // A table that did not read still keeps the window itself.
    assert_eq!(
        ProtectedRoot::from_window_env(env, &ProcessTable::unreadable("no ps"), 210),
        vec![ProtectedRoot::window(WINDOW)]
    );
}

/* ---- a job the pane set loose (t-81714) -------------------------------------- */

const SET_LOOSE: u32 = 600;
const SET_LOOSE_BY_THE_OTHER_PANE: u32 = 610;
const SET_LOOSE_IN_ANOTHER_WINDOW: u32 = 620;
const LENT_TO_THE_OTHER_PANE: u32 = 320;
const ANOTHER_WINDOW: u32 = 900;

fn mark(window: u32, pane: &str) -> PaneMark {
    PaneMark {
        window,
        pane: pane.to_string(),
    }
}

/// [`table`] as the window reads it on 10-29: every pane's process names its
/// pane in its environment, and the calling pane's `… &` job, its tool's shell
/// gone, hangs under pid 1 still naming the calling pane. Beside it, the
/// other pane's loose job, one from a pane numbered alike in another window,
/// and a process in the other pane's tree that was started with the calling
/// pane's mark.
fn marked_table() -> ProcessTable {
    let mut rows = match table().rows {
        Ok(rows) => rows,
        Err(why) => panic!("the fixture table reads: {why}"),
    };
    rows.extend([
        row(SET_LOOSE, 1, SET_LOOSE, "python3 -m http.server 8000"),
        row(
            SET_LOOSE_BY_THE_OTHER_PANE,
            1,
            SET_LOOSE_BY_THE_OTHER_PANE,
            "python3 -m http.server 8001",
        ),
        row(
            SET_LOOSE_IN_ANOTHER_WINDOW,
            1,
            SET_LOOSE_IN_ANOTHER_WINDOW,
            "python3 -m http.server 8002",
        ),
        row(
            LENT_TO_THE_OTHER_PANE,
            OTHER_AGENT,
            LENT_TO_THE_OTHER_PANE,
            "python3 -m http.server 8003",
        ),
    ]);
    ProcessTable::new(rows)
        .with_pane_mark(CALLER, mark(WINDOW, "term-1"))
        .with_pane_mark(210, mark(WINDOW, "term-1"))
        .with_pane_mark(CALLER_CHILD, mark(WINDOW, "term-1"))
        .with_pane_mark(SET_LOOSE, mark(WINDOW, "term-1"))
        .with_pane_mark(OTHER_PANE, mark(WINDOW, "term-2"))
        .with_pane_mark(OTHER_AGENT, mark(WINDOW, "term-2"))
        .with_pane_mark(SET_LOOSE_BY_THE_OTHER_PANE, mark(WINDOW, "term-2"))
        .with_pane_mark(SET_LOOSE_IN_ANOTHER_WINDOW, mark(ANOTHER_WINDOW, "term-1"))
        .with_pane_mark(LENT_TO_THE_OTHER_PANE, mark(WINDOW, "term-1"))
}

fn judged_marked(command: &str, table: &ProcessTable, caller: Option<u32>) -> KillVerdict {
    judge_kill(&kill_calls(command), table, caller, &protected())
}

/// The 10-29 06:3x measurement: a pane's `… &` job, moved under pid 1 when
/// its tool's shell ended, was refused to the pane that started it — by
/// number too, the way the refusal itself told it to (`echo $!`, then
/// `kill 4242`). It names the calling pane, so it is the pane's own.
#[test]
fn a_job_the_pane_set_loose_is_its_own_to_end() {
    let table = marked_table();
    for command in [
        "kill 600",
        "kill -9 600",
        "kill -- -600",
        "pkill -f 'http.server 8000'",
        "kill $(pgrep -f 'http.server 8000')",
    ] {
        assert_eq!(
            judged_marked(command, &table, Some(CALLER)),
            KillVerdict::Allow,
            "{command:?}"
        );
    }
}

/// What the calling pane did not start stays asked — another pane's loose
/// job, one from a pane of the same number in another window, one that names
/// no pane — and what hangs in a kept tree stays refused whatever its
/// environment says.
#[test]
fn a_loose_job_of_another_pane_or_window_or_none_is_still_asked() {
    let table = marked_table();
    for (command, pid) in [
        ("kill 610", SET_LOOSE_BY_THE_OTHER_PANE),
        ("kill 620", SET_LOOSE_IN_ANOTHER_WINDOW),
        ("kill 500", DETACHED),
        (
            "pkill -f 'http.server 800[012]'",
            SET_LOOSE_BY_THE_OTHER_PANE,
        ),
    ] {
        match judged_marked(command, &table, Some(CALLER)) {
            KillVerdict::Ask { reason } => {
                assert!(
                    reason.contains(&format!("pid {pid}")),
                    "{command:?}: {reason}"
                );
            }
            other => panic!("{command:?} should be asked of a person, was {other:?}"),
        }
    }
    for command in [
        "kill 320",
        "kill 310",
        "kill 100",
        "pkill -f 'http.server 800'",
    ] {
        assert!(
            matches!(
                judged_marked(command, &table, Some(CALLER)),
                KillVerdict::Deny { .. }
            ),
            "{command:?}"
        );
    }
}

/// The mark is the caller's own only when the caller's own process names it:
/// a caller the window cannot place, or whose environment did not read, owns
/// no loose job.
#[test]
fn a_caller_without_a_mark_owns_no_loose_job() {
    let unmarked_caller = {
        let mut table = marked_table();
        table.marks.remove(&CALLER);
        table
    };
    for (table, caller) in [(&unmarked_caller, Some(CALLER)), (&marked_table(), None)] {
        assert!(
            matches!(
                judged_marked("kill 600", table, caller),
                KillVerdict::Ask { .. }
            ),
            "{caller:?}"
        );
    }
}

/// A mark names a window and a pane, or nothing.
#[test]
fn a_mark_needs_both_the_window_and_the_pane() {
    let env = |window: Option<&'static str>, pane: Option<&'static str>| {
        move |name: &str| match name {
            WINDOW_PID_ENV => window.map(str::to_string),
            PANE_KEY_ENV => pane.map(str::to_string),
            _ => None,
        }
    };
    assert_eq!(
        PaneMark::from_env(env(Some("100"), Some("term-1"))),
        Some(mark(WINDOW, "term-1"))
    );
    for (window, pane) in [
        (None, Some("term-1")),
        (Some("100"), None),
        (Some("100"), Some("")),
        (Some("not-a-pid"), Some("term-1")),
        (Some("0"), Some("term-1")),
    ] {
        assert_eq!(
            PaneMark::from_env(env(window, pane)),
            None,
            "{window:?} {pane:?}"
        );
    }
}
