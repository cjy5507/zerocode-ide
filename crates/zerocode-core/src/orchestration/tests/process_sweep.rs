//! t-78914: `process-sweep` lists by plain text and ends only the pids a list
//! showed, one at a time, each read again before its signal. Every table here
//! is a fixture and every signal is written down by a fake: no test reaches a
//! real process.

use std::cell::RefCell;

use super::*;
use crate::orchestration::process_sweep::{
    Checkout, Ended, Judge, Judgment, Outcome, ProcessRow, ProcessSweepAsk, Signals, TARGETS_MAX,
    Target, VERB, end, end_answer, find, ledger_line, mark, matching_pids,
};

const STARTED: &str = "Wed Oct 28 21:00:00 2026";
const LATER: &str = "Wed Oct 28 23:59:00 2026";

fn row(pid: u32, ppid: u32, args: &str) -> ProcessRow {
    ProcessRow {
        pid,
        ppid,
        started: STARTED.to_string(),
        args: args.to_string(),
        cwd: None,
    }
}

/// The same machine every case reads: a window, a pane shell, an agent, two
/// builds in two checkouts and a person's editor.
fn table() -> Vec<ProcessRow> {
    vec![
        row(
            500,
            1,
            "/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell",
        ),
        row(600, 500, "-zsh"),
        row(700, 600, "claude --model opus"),
        ProcessRow {
            cwd: Some("/w/t-1".to_string()),
            ..row(4100, 700, "cargo build -p zerocode-shell")
        },
        ProcessRow {
            cwd: Some("/w/t-2/crates".to_string()),
            ..row(4200, 700, "cargo test -p zerocode-core")
        },
        row(4300, 1, "/usr/bin/vim notes.txt"),
        row(4400, 700, "sh zerocode-orc process-sweep --find cargo"),
    ]
}

fn target(row: &ProcessRow) -> Target {
    Target {
        pid: row.pid,
        mark: mark(row.pid, &row.started, &row.args),
    }
}

/// A judge that protects the pids it is given, saying why, and allows the rest.
struct Protects(Vec<(u32, &'static str)>);

impl Judge for Protects {
    fn judge(&self, row: &ProcessRow, _table: &[ProcessRow]) -> Judgment {
        match self.0.iter().find(|(pid, _)| *pid == row.pid) {
            Some((_, why)) => Judgment::Deny {
                why: (*why).to_string(),
            },
            None => Judgment::Allow,
        }
    }
}

/// The system as a fake: rereads answer from `now` (what stands at signal
/// time), and every signal is written down instead of sent.
struct Fake {
    now: Vec<ProcessRow>,
    signalled: RefCell<Vec<u32>>,
    fails: Option<u32>,
}

impl Fake {
    fn standing(now: Vec<ProcessRow>) -> Self {
        Self {
            now,
            signalled: RefCell::new(Vec::new()),
            fails: None,
        }
    }
}

impl Signals for Fake {
    fn reread(&self, pid: u32) -> Option<(String, String)> {
        self.now
            .iter()
            .find(|row| row.pid == pid)
            .map(|row| (row.started.clone(), row.args.clone()))
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        self.signalled.borrow_mut().push(pid);
        match self.fails {
            Some(failing) if failing == pid => Err("Operation not permitted".to_string()),
            _ => Ok(()),
        }
    }
}

fn outcomes(ended: &[Ended]) -> Vec<(u32, Outcome)> {
    ended
        .iter()
        .map(|one| (one.target.pid, one.outcome.clone()))
        .collect()
}

/* ---- the list ---------------------------------------------------------- */

/// The needle is plain text: a regex's dot, star or anchor is a character to
/// find, so `cargo.*` matches nothing in a table that has no such text.
#[test]
fn the_needle_is_text_never_a_pattern() {
    let table = table();
    assert_eq!(matching_pids(&table, "cargo"), vec![4100, 4200]);
    assert!(matching_pids(&table, "cargo.*").is_empty());
    assert!(matching_pids(&table, "^cargo").is_empty());
}

/// The asking command holds its own needle; it is not listed.
#[test]
fn the_asking_command_is_not_its_own_candidate() {
    assert!(!matching_pids(&table(), "cargo").contains(&4400));
}

/// A row says its target, parent, start, place, owner and the judge's word;
/// a protected row says why. Nothing is signalled by a list.
#[test]
fn a_list_says_each_rows_target_place_owner_and_verdict() {
    let table = table();
    let checkouts = [
        Checkout {
            path: "/w/t-1".to_string(),
            run: "run-1".to_string(),
            worker: "w-1".to_string(),
            task: Some("t-1".to_string()),
        },
        Checkout {
            path: "/w/t-2".to_string(),
            run: "run-2".to_string(),
            worker: "w-2".to_string(),
            task: None,
        },
    ];
    let judge = Protects(vec![(4200, "another run's build")]);
    let said = find(&table, "cargo", &checkouts, &judge);
    assert_eq!(said["matched"], 2);
    let rows = said["rows"].as_array().expect("rows");
    let first = &rows[0];
    assert_eq!(first["pid"], 4100);
    assert_eq!(first["ppid"], 700);
    assert_eq!(first["started"], STARTED);
    assert_eq!(first["cwd"], "/w/t-1");
    assert_eq!(first["owner"]["worker"], "w-1");
    assert_eq!(first["owner"]["task"], "t-1");
    assert_eq!(first["verdict"], "may_end");
    assert_eq!(
        first["target"],
        format!(
            "4100@{}",
            mark(4100, STARTED, "cargo build -p zerocode-shell")
        )
    );
    let second = &rows[1];
    assert_eq!(
        second["owner"]["run"], "run-2",
        "a place under a checkout is its"
    );
    assert_eq!(second["verdict"], "protected");
    assert_eq!(second["why"], "another run's build");
    assert!(
        said["never"]
            .as_str()
            .is_some_and(|line| line.contains("패턴"))
    );
}

/// A command line's private value is masked in what the list says.
#[test]
fn a_listed_command_line_is_masked() {
    let token = format!("ghp_{}", "FAKE".repeat(9));
    let table = [row(4100, 1, &format!("cargo run -- {token}"))];
    let said = find(&table, "cargo", &[], &Protects(Vec::new()));
    let args = said["rows"][0]["args"].as_str().expect("args");
    assert!(!args.contains(&token), "{args}");
    assert!(args.starts_with("cargo run"), "{args}");
}

/* ---- the end road ------------------------------------------------------ */

/// An allowed pid whose mark still holds is signalled, and only it.
#[test]
fn an_allowed_pid_that_still_matches_is_ended_alone() {
    let table = table();
    let fake = Fake::standing(table.clone());
    let ended = end(&table, &[target(&table[3])], &Protects(Vec::new()), &fake);
    assert_eq!(outcomes(&ended), vec![(4100, Outcome::Ended)]);
    assert_eq!(*fake.signalled.borrow(), vec![4100]);
}

/// Refusal 1: the judge protects the pid — refused with the judge's reason,
/// and the pids after it are still carried one by one.
#[test]
fn a_protected_pid_is_refused_with_its_reason_and_the_rest_go_on() {
    let table = table();
    let fake = Fake::standing(table.clone());
    let judge = Protects(vec![(700, "a worker CLI the ledger knows")]);
    let ended = end(
        &table,
        &[target(&table[2]), target(&table[3])],
        &judge,
        &fake,
    );
    assert_eq!(
        outcomes(&ended),
        vec![
            (
                700,
                Outcome::Refused {
                    why: "a worker CLI the ledger knows".to_string()
                }
            ),
            (4100, Outcome::Ended),
        ]
    );
    assert_eq!(
        *fake.signalled.borrow(),
        vec![4100],
        "the protected pid was signalled"
    );
}

/// Refusals 2–4: the window, a pane shell and a person's editor each refused,
/// each saying its own reason, and none signalled.
#[test]
fn every_protected_kind_is_refused_and_none_is_signalled() {
    let table = table();
    let fake = Fake::standing(table.clone());
    let judge = Protects(vec![
        (500, "the ZeroCode window"),
        (600, "a pane's shell"),
        (4300, "a person's tab process"),
    ]);
    let ended = end(
        &table,
        &[target(&table[0]), target(&table[1]), target(&table[5])],
        &judge,
        &fake,
    );
    let whys: Vec<String> = ended
        .iter()
        .map(|one| match &one.outcome {
            Outcome::Refused { why } => why.clone(),
            other => panic!("pid {} was not refused: {other:?}", one.target.pid),
        })
        .collect();
    assert_eq!(
        whys,
        vec![
            "the ZeroCode window",
            "a pane's shell",
            "a person's tab process"
        ]
    );
    assert!(fake.signalled.borrow().is_empty());
}

/// The reread right before the signal finds another process under the pid
/// (same pid, a later start and other arguments): left alone, said as
/// changed, never signalled.
#[test]
fn a_pid_reused_since_the_list_is_left_alone() {
    let table = table();
    let reused = ProcessRow {
        started: LATER.to_string(),
        args: "/usr/bin/ssh-agent".to_string(),
        ..table[3].clone()
    };
    let fake = Fake::standing(vec![reused]);
    let ended = end(&table, &[target(&table[3])], &Protects(Vec::new()), &fake);
    assert_eq!(outcomes(&ended), vec![(4100, Outcome::Changed)]);
    assert_eq!(
        ended[0].args, "/usr/bin/ssh-agent",
        "it says what stands now"
    );
    assert!(fake.signalled.borrow().is_empty());
}

/// A mark the table no longer agrees with (the arguments changed between the
/// list and this call) is changed before anybody is asked.
#[test]
fn a_mark_the_table_disagrees_with_is_changed() {
    let table = table();
    let stale = Target {
        pid: 4100,
        mark: mark(4100, STARTED, "cargo build -p something-else"),
    };
    let fake = Fake::standing(table.clone());
    let ended = end(&table, &[stale], &Protects(Vec::new()), &fake);
    assert_eq!(outcomes(&ended), vec![(4100, Outcome::Changed)]);
    assert!(fake.signalled.borrow().is_empty());
}

/// A pid gone by the reread is gone, and a signal the system refused is a
/// failure with its reason — neither is said as ended.
#[test]
fn gone_and_failed_are_said_as_they_are() {
    let table = table();
    let mut fake = Fake::standing(vec![table[4].clone()]);
    fake.fails = Some(4200);
    let ended = end(
        &table,
        &[target(&table[3]), target(&table[4])],
        &Protects(Vec::new()),
        &fake,
    );
    assert_eq!(
        outcomes(&ended),
        vec![
            (4100, Outcome::Gone),
            (
                4200,
                Outcome::Failed {
                    why: "Operation not permitted".to_string()
                }
            ),
        ]
    );
    let answer = end_answer("run-1", &ended);
    assert_eq!(answer["ended"], 0);
    let line = ledger_line(&ended);
    assert!(line.starts_with("process-sweep: ended 0 of 2"), "{line}");
    assert!(line.contains("4100 gone"), "{line}");
    assert!(line.contains("4200 failed"), "{line}");
}

/* ---- the verb ---------------------------------------------------------- */

fn asked(planned: &Decided) -> ProcessSweepAsk {
    match planned.effect.clone() {
        Effect::ProcessSweep(ask) => ask,
        other => panic!("{VERB} planned no sweep: {other:?}"),
    }
}

/// A list is any pane's and needs no run; it carries the checkouts the
/// ledger was told, with each worker's task.
#[test]
fn a_list_carries_the_needle_and_the_ledgers_checkouts() {
    let mut bench = Bench::new();
    bench.json("run-create --name sweep");
    let task = bench.json("task-create --spec build")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), "/w/t-1"));
    let planned = bench.run(&format!("{VERB} --find cargo"));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let ProcessSweepAsk::Find { needle, checkouts } = asked(&planned) else {
        panic!("a list was asked for")
    };
    assert_eq!(needle, "cargo");
    assert_eq!(checkouts.len(), 1);
    assert_eq!(checkouts[0].path, "/w/t-1");
    assert_eq!(checkouts[0].worker, worker);
    assert_eq!(checkouts[0].task.as_deref(), Some(task.as_str()));
}

/// The coordinator's end call names its run and the pids, marks and all.
#[test]
fn an_end_call_names_its_run_and_targets() {
    let mut bench = Bench::new();
    bench.json("run-create --name sweep");
    let run = bench.ledger.runs()[0].id.clone();
    let planned = bench.run(&format!(
        "{VERB} --kill 4100@0123456789ab,4200@abcdef012345"
    ));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    assert_eq!(
        asked(&planned),
        ProcessSweepAsk::End {
            run,
            targets: vec![
                Target {
                    pid: 4100,
                    mark: "0123456789ab".to_string()
                },
                Target {
                    pid: 4200,
                    mark: "abcdef012345".to_string()
                },
            ],
        }
    );
}

/// Every call the verb refuses before the window is asked: no effect planned,
/// and the reason names what to do instead.
#[test]
fn malformed_calls_are_refused_before_the_window_is_asked() {
    let many: Vec<String> = (0..=TARGETS_MAX)
        .map(|at| format!("{}@0123456789ab", 5000 + at))
        .collect();
    let cases: [(String, &str); 9] = [
        (VERB.to_string(), "--find"),
        (format!("{VERB} cargo"), "no bare words"),
        (
            format!("{VERB} --find cargo --kill 4100@0123456789ab"),
            "never both",
        ),
        (format!("{VERB} --find c"), "at least"),
        (format!("{VERB} --kill 4100"), "no mark"),
        (format!("{VERB} --kill 1@0123456789ab"), "above 1"),
        (format!("{VERB} --kill 4100@XYZ"), "lowercase hex"),
        (
            format!("{VERB} --kill 4100@0123456789ab,4100@0123456789ab"),
            "twice",
        ),
        (format!("{VERB} --kill {}", many.join(",")), "at most"),
    ];
    for (line, says) in cases {
        let mut bench = Bench::new();
        bench.json("run-create --name sweep");
        let planned = bench.run(&line);
        assert_ne!(planned.reply.exit_code, 0, "`{line}` was not refused");
        assert!(
            planned.reply.stderr.contains(says),
            "`{line}` was refused without saying {says:?}: {}",
            planned.reply.stderr
        );
        assert!(
            matches!(planned.effect, Effect::None),
            "`{line}` planned {:?}",
            planned.effect
        );
    }
}

/// Ending is the coordinator's: a worker's pane is refused and told to ask.
#[test]
fn a_workers_pane_cannot_end_processes() {
    let mut bench = Bench::new();
    bench.json("run-create --name sweep");
    let task = bench.json("task-create --spec build")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let planned = bench.at(&pane, &format!("{VERB} --kill 4100@0123456789ab"));
    assert_ne!(planned.reply.exit_code, 0);
    assert!(
        planned.reply.stderr.contains(&worker),
        "{}",
        planned.reply.stderr
    );
    assert!(matches!(planned.effect, Effect::None));
    // A worker may still look.
    let looked = bench.at(&pane, &format!("{VERB} --find cargo"));
    assert_eq!(looked.reply.exit_code, 0, "{}", looked.reply.stderr);
}

/// A list changes nothing and needs no retry name; an end call does.
#[test]
fn only_the_end_call_changes_anything() {
    let find_words = split_words(&words("--find cargo"), BOOL_FLAGS);
    let kill_words = split_words(&words("--kill 4100@0123456789ab"), BOOL_FLAGS);
    assert!(!changes_the_ledger(VERB, &find_words));
    assert!(changes_the_ledger(VERB, &kill_words));
}

/// The help says the rule the verb exists for.
#[test]
fn the_help_says_never_to_kill_by_pattern() {
    let (_, about, _) = VERBS
        .iter()
        .find(|(name, _, _)| *name == VERB)
        .expect("the verb is in the table");
    assert!(about.contains("패턴으로 죽이는 명령은 쓰지 않는다 — 인자가 패턴이 된다"));
}
