//! t-78914: `process-sweep` lists by plain text and ends only the pids a list
//! showed, one at a time, each read again before its signal. Every table here
//! is a fixture and every signal is written down by a fake: no test reaches a
//! real process.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::*;
use crate::orchestration::process_sweep::{
    Checkout, Ended, Judge, Judgment, Outcome, ProcessRow, ProcessSweepAsk, Signals, TARGETS_MAX,
    Target, VERB, end, end_answer, find, ledger_line, mark, matching_pids, outside, owner,
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
/// time), and every signal is written down instead of sent. A signaled pid
/// leaves `now`, as a process that ends on TERM does, unless it `ignores`
/// TERM; every pause is written down instead of slept, and moves the fake's
/// clock as far as a sleep would.
struct Fake {
    now: RefCell<Vec<ProcessRow>>,
    signalled: RefCell<Vec<u32>>,
    fails: Option<u32>,
    ignores: Vec<u32>,
    /// Pids handed to another process (a later start) the moment they go.
    reborn: Vec<u32>,
    paused: RefCell<Vec<u64>>,
    /// One-pid reads, and the pids each many-pid read was asked for.
    single_reads: Cell<usize>,
    many_reads: RefCell<Vec<Vec<u32>>>,
    /// Every many-pid read fails, as a `ps` that timed out does.
    unreadable: bool,
    /// How long each many-pid read takes on the fake's clock.
    read_ms: u64,
    clock: Cell<u64>,
}

impl Fake {
    fn standing(now: Vec<ProcessRow>) -> Self {
        Self {
            now: RefCell::new(now),
            signalled: RefCell::new(Vec::new()),
            fails: None,
            ignores: Vec::new(),
            reborn: Vec::new(),
            paused: RefCell::new(Vec::new()),
            single_reads: Cell::new(0),
            many_reads: RefCell::new(Vec::new()),
            unreadable: false,
            read_ms: 0,
            clock: Cell::new(0),
        }
    }
}

impl Signals for Fake {
    fn reread(&self, pid: u32) -> Option<(String, String)> {
        self.single_reads.set(self.single_reads.get() + 1);
        self.now
            .borrow()
            .iter()
            .find(|row| row.pid == pid)
            .map(|row| (row.started.clone(), row.args.clone()))
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        self.signalled.borrow_mut().push(pid);
        match self.fails {
            Some(failing) if failing == pid => Err("Operation not permitted".to_string()),
            _ => {
                let mut now = self.now.borrow_mut();
                if self.reborn.contains(&pid) {
                    for row in now.iter_mut().filter(|row| row.pid == pid) {
                        row.started = LATER.to_string();
                    }
                } else if !self.ignores.contains(&pid) {
                    now.retain(|row| row.pid != pid);
                }
                Ok(())
            }
        }
    }

    fn reread_many(&self, pids: &[u32]) -> Option<HashMap<u32, (String, String)>> {
        self.many_reads.borrow_mut().push(pids.to_vec());
        self.clock.set(self.clock.get() + self.read_ms);
        if self.unreadable {
            return None;
        }
        Some(
            self.now
                .borrow()
                .iter()
                .filter(|row| pids.contains(&row.pid))
                .map(|row| (row.pid, (row.started.clone(), row.args.clone())))
                .collect(),
        )
    }

    fn pause(&self, ms: u64) {
        self.paused.borrow_mut().push(ms);
        self.clock.set(self.clock.get() + ms);
    }

    fn now_ms(&self) -> u64 {
        self.clock.get()
    }
}

const GONE: Outcome = Outcome::Signaled { gone: true };

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
    // Built at run time, and of letters the table's allow list does not
    // excuse (`fake`, `test`, `example`… are left unmasked on purpose).
    let token = format!("ghp_{}", "Q7mZ".repeat(9));
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
    assert_eq!(outcomes(&ended), vec![(4100, GONE)]);
    assert_eq!(*fake.signalled.borrow(), vec![4100]);
    assert!(
        fake.paused.borrow().is_empty(),
        "a pid gone at once is not waited for"
    );
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
            (4100, GONE),
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

/// The same pid and the very same command line, started again after the
/// list: the mark carries the start, so the new process is not the one shown
/// and is left alone (an arguments-only check could not tell them apart).
#[test]
fn a_pid_restarted_with_the_same_command_line_is_left_alone() {
    let table = table();
    let restarted = ProcessRow {
        started: LATER.to_string(),
        ..table[3].clone()
    };
    assert_eq!(restarted.args, table[3].args);
    let fake = Fake::standing(vec![restarted]);
    let ended = end(&table, &[target(&table[3])], &Protects(Vec::new()), &fake);
    assert_eq!(outcomes(&ended), vec![(4100, Outcome::Changed)]);
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
    assert_eq!(answer["signaled"], 0);
    let line = ledger_line(1, &ended);
    assert!(
        line.starts_with("process-sweep: TERM sent to 0 of 2, 0 gone"),
        "{line}"
    );
    assert!(line.contains("4100 gone"), "{line}");
    assert!(line.contains("4200 failed"), "{line}");
}

/// A sent TERM is not an ended process (r1 P2-1): the pid that left after
/// it is said as signaled and gone, the pid that ignored it as signaled and
/// still running — read again for the whole settle wait, 20 × 100 ms, and
/// never sent anything stronger.
#[test]
fn a_sent_term_is_said_as_signaled_and_whether_it_went() {
    let table = table();
    let mut fake = Fake::standing(table.clone());
    fake.ignores = vec![4200];
    let ended = end(
        &table,
        &[target(&table[3]), target(&table[4])],
        &Protects(Vec::new()),
        &fake,
    );
    assert_eq!(
        outcomes(&ended),
        vec![(4100, GONE), (4200, Outcome::Signaled { gone: false })]
    );
    assert_eq!(*fake.signalled.borrow(), vec![4100, 4200], "one TERM each");
    assert_eq!(
        *fake.paused.borrow(),
        vec![100; 20],
        "2 s in all, then said"
    );
    let answer = end_answer("run-1", &ended);
    assert_eq!(answer["signaled"], 2);
    assert_eq!(answer["gone"], 1);
    assert_eq!(answer["rows"][0]["outcome"], "signaled");
    assert_eq!(answer["rows"][0]["after"], "gone");
    assert_eq!(answer["rows"][1]["after"], "still_running");
    assert!(
        answer["rows"][1]["why"]
            .as_str()
            .is_some_and(|why| why.contains("still runs")),
        "{answer}"
    );
    assert!(answer.get("ended").is_none(), "{answer}");
    let line = ledger_line(4, &ended);
    assert!(
        line.starts_with(
            "process-sweep: TERM sent to 2 of 2, 1 gone, asked by coordinator generation 4"
        ),
        "{line}"
    );
    assert!(line.contains("4100 signaled, gone"), "{line}");
    assert!(line.contains("4200 signaled, still running"), "{line}");
    assert!(!line.contains("ended"), "{line}");
}

/// A pid that went on TERM and was handed to another process before the
/// settle read is gone too: the mark, not the pid, is what was signaled.
#[test]
fn a_signaled_pid_reused_before_the_settle_read_is_gone() {
    let table = table();
    let mut fake = Fake::standing(table.clone());
    fake.reborn = vec![4100];
    let ended = end(&table, &[target(&table[3])], &Protects(Vec::new()), &fake);
    assert_eq!(outcomes(&ended), vec![(4100, GONE)]);
    assert!(fake.paused.borrow().is_empty());
}

/// The settle wait reads every pid still standing in ONE read a step (r2
/// P2-1): 32 pids that ignore TERM are read one at a time once each, right
/// before their signals, and then 21 times all together — not 32 × 21.
#[test]
fn the_settle_wait_reads_all_standing_pids_in_one_read_a_step() {
    let table: Vec<ProcessRow> = (6000..)
        .take(TARGETS_MAX)
        .map(|pid| row(pid, 1, "cargo build -p zerocode-shell"))
        .collect();
    let mut fake = Fake::standing(table.clone());
    fake.ignores = table.iter().map(|row| row.pid).collect();
    let targets: Vec<Target> = table.iter().map(target).collect();
    let ended = end(&table, &targets, &Protects(Vec::new()), &fake);
    assert!(
        ended
            .iter()
            .all(|one| one.outcome == Outcome::Signaled { gone: false }),
        "{ended:?}"
    );
    assert_eq!(
        fake.single_reads.get(),
        TARGETS_MAX,
        "one read before each signal"
    );
    let many = fake.many_reads.borrow();
    assert_eq!(
        many.len(),
        21,
        "one read a step, 20 steps and the last look"
    );
    let all: Vec<u32> = table.iter().map(|row| row.pid).collect();
    assert!(many.iter().all(|asked| *asked == all), "{many:?}");
}

/// A step asks only for the pids still standing, and stops once none does.
#[test]
fn a_settle_step_asks_only_for_the_pids_still_standing() {
    let table = table();
    let mut fake = Fake::standing(table.clone());
    fake.ignores = vec![4200];
    let ended = end(
        &table,
        &[target(&table[3]), target(&table[4])],
        &Protects(Vec::new()),
        &fake,
    );
    assert_eq!(
        outcomes(&ended),
        vec![(4100, GONE), (4200, Outcome::Signaled { gone: false })]
    );
    let many = fake.many_reads.borrow();
    assert_eq!(many[0], vec![4100, 4200]);
    assert!(
        many[1..].iter().all(|asked| *asked == vec![4200]),
        "{many:?}"
    );
}

/// A settle read that failed proves nothing: the pid is not said gone.
#[test]
fn a_failed_settle_read_says_nobody_went() {
    let table = table();
    let mut fake = Fake::standing(table.clone());
    fake.unreadable = true;
    let ended = end(&table, &[target(&table[3])], &Protects(Vec::new()), &fake);
    assert_eq!(*fake.signalled.borrow(), vec![4100]);
    assert_eq!(
        outcomes(&ended),
        vec![(4100, Outcome::Signaled { gone: false })]
    );
}

/// The settle wait stops on the clock too (r3 P2-1): on a machine whose `ps`
/// takes its whole 2 s every time, 21 reads would hold the answer 44 s past
/// the window's 10 s. No read starts after 3 s, so the wait ends by 5 s; the
/// pid that ignored TERM, or that nobody could read, is said still running,
/// and it got one TERM and nothing more.
#[test]
fn a_settle_wait_whose_reads_are_slow_stops_on_time() {
    for unreadable in [false, true] {
        let table = table();
        let mut fake = Fake::standing(table.clone());
        fake.ignores = vec![4100];
        fake.unreadable = unreadable;
        fake.read_ms = 2_000;
        let ended = end(&table, &[target(&table[3])], &Protects(Vec::new()), &fake);
        assert_eq!(
            outcomes(&ended),
            vec![(4100, Outcome::Signaled { gone: false })],
            "unreadable: {unreadable}"
        );
        assert_eq!(*fake.signalled.borrow(), vec![4100], "one TERM");
        assert_eq!(
            fake.many_reads.borrow().len(),
            2,
            "reads at 0 s and 2.1 s, none from 3 s on (unreadable: {unreadable})"
        );
        assert!(
            fake.clock.get() <= 5_000,
            "the wait took {} ms (unreadable: {unreadable})",
            fake.clock.get()
        );
    }
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
    let (worker, pane) = bench.seat(&format!(
        "worker-start --agent claude --task {task} --worktree"
    ));
    assert!(bench.ledger.worker_seated(("team-1", &pane), "/w/t-1"));
    let planned = bench.run(&format!("{VERB} --find cargo"));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let ProcessSweepAsk::Find {
        needle,
        checkouts,
        coordinator_seats,
    } = asked(&planned)
    else {
        panic!("a list was asked for")
    };
    assert_eq!(needle, "cargo");
    assert_eq!(checkouts.len(), 1);
    assert_eq!(checkouts[0].path, "/w/t-1");
    assert_eq!(checkouts[0].worker, worker);
    assert_eq!(checkouts[0].task.as_deref(), Some(task.as_str()));
    let live = bench.ledger.runs()[0]
        .coordinator_live()
        .map(|seat| seat.seat.clone());
    assert_eq!(coordinator_seats, live.into_iter().collect::<Vec<_>>());
}

/// r2 P1-1: a worker opened without a tree of its own (no `--worktree`, or
/// a replacement inheriting such a worker's tree) reports its coordinator's
/// tree — on this machine 191 of 1,499 worker rows named the main checkout,
/// where the build line's runner stands. Only a tree of the worker's own is
/// a worker checkout, and never one holding the run's project root.
#[test]
fn only_a_tree_of_the_workers_own_is_a_worker_checkout() {
    let mut bench = Bench::new();
    bench.json("run-create --name sweep");
    let mut task = || {
        bench.json("task-create --spec build")["taskId"]
            .as_str()
            .expect("a task id")
            .to_string()
    };
    let (own_task, shared_task, rooted_task) = (task(), task(), task());
    let mut seat = |line: String, checkout: &str| {
        let (worker, pane) = bench.seat(&line);
        assert!(bench.ledger.worker_seated(("team-1", &pane), checkout));
        worker
    };
    let own = seat(
        format!("worker-start --agent claude --task {own_task} --worktree"),
        "/w/t-1",
    );
    let shared = seat(
        format!("worker-start --agent claude --task {shared_task}"),
        "/main",
    );
    let rooted = seat(
        format!("worker-start --agent claude --task {rooted_task} --worktree"),
        "/main",
    );
    // Each replaced by a worker inheriting its tree.
    let mut inherit = |ended: &str, task: &str, checkout: &str| {
        let dispatch = bench.ledger.runs()[0]
            .worker(ended)
            .and_then(|held| held.dispatch.clone())
            .expect("the attempt");
        bench.json(&format!("worker-stop --worker {ended} --reason quota-wall"));
        let (worker, pane) = bench.seat(&format!(
            "worker-start --agent claude --task {task} --retry-of {dispatch} --inherit-checkout"
        ));
        assert!(bench.ledger.worker_seated(("team-1", &pane), checkout));
        worker
    };
    let own_heir = inherit(&own, &own_task, "/w/t-1");
    let shared_heir = inherit(&shared, &shared_task, "/main");
    let in_worktree = |worker: &str| {
        bench.ledger.runs()[0]
            .worker(worker)
            .expect("the worker")
            .in_worktree
    };
    assert!(in_worktree(&own) && in_worktree(&own_heir) && in_worktree(&rooted));
    assert!(!in_worktree(&shared) && !in_worktree(&shared_heir));
    let run = bench.ledger.runs()[0].id.clone();
    // The run's root as `git_dir::project_root` gives it: the git common
    // directory inside the main checkout, not the checkout itself.
    bench.ledger.run_mut(&run).expect("the run").project = Some(RunProject {
        root: "/main/.git".to_string(),
        source: ProjectSource::Created,
        at_ms: 1,
        seat: None,
    });
    let planned = bench.run(&format!("{VERB} --find cargo"));
    let ProcessSweepAsk::Find { checkouts, .. } = asked(&planned) else {
        panic!("a list was asked for")
    };
    let workers: Vec<&str> = checkouts.iter().map(|one| one.worker.as_str()).collect();
    assert_eq!(workers, vec![own.as_str(), own_heir.as_str()]);
    assert!(owner("/main", &checkouts).is_none());
    assert!(owner("/main/build-queue", &checkouts).is_none());
}

/// A coordinator's tree, or a project root, held by a checkout — the
/// checkout itself or a folder above it — takes the checkout out; one
/// inside it, or beside it, does not.
#[test]
fn a_checkout_holding_a_coordinators_tree_is_nobodys() {
    let told = |path: &str| Checkout {
        path: path.to_string(),
        run: "run-1".to_string(),
        worker: "w-1".to_string(),
        task: None,
    };
    let held = outside(
        vec![
            told("/main"),
            told("/"),
            told("/main/.worktrees/t-1"),
            told("/w/t-1"),
            told("/main-other"),
        ],
        &["/main/".to_string()],
    );
    let paths: Vec<&str> = held.iter().map(|one| one.path.as_str()).collect();
    assert_eq!(paths, vec!["/main/.worktrees/t-1", "/w/t-1", "/main-other"]);
    // A run's root is its git common directory, `<main checkout>/.git`: the
    // main checkout holds it and goes; a worktree beside it stays.
    let held = outside(
        vec![told("/main"), told("/w/t-1"), told("/main/.git")],
        &["/main/.git".to_string()],
    );
    let paths: Vec<&str> = held.iter().map(|one| one.path.as_str()).collect();
    assert_eq!(paths, vec!["/w/t-1"]);
}

/// The coordinator's end call names its run and the pids, marks and all.
#[test]
fn an_end_call_names_its_run_and_targets() {
    let mut bench = Bench::new();
    bench.json("run-create --name sweep");
    let run = bench.ledger.runs()[0].id.clone();
    let generation = bench.ledger.runs()[0]
        .coordinator_live()
        .map(|seat| seat.generation)
        .expect("the bench's coordinator sits");
    let seats = bench.ledger.runs()[0]
        .coordinator_live()
        .map(|seat| seat.seat.clone())
        .into_iter()
        .collect();
    let planned = bench.run(&format!(
        "{VERB} --kill 4100@0123456789ab,4200@abcdef012345 --retry-request sweep-1"
    ));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    assert_eq!(
        asked(&planned),
        ProcessSweepAsk::End {
            run,
            generation,
            request: "sweep-1".to_string(),
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
            checkouts: Vec::new(),
            coordinator_seats: seats,
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

/// An orphan's pid without the mark a list showed for it is refused before the
/// window is asked: the mark is what makes the coordinator's pick the yes.
#[test]
fn an_orphan_named_without_its_mark_is_refused() {
    let mut bench = Bench::new();
    bench.json("run-create --name sweep");
    let planned = bench.run(&format!("{VERB} --kill 4300"));
    assert_ne!(planned.reply.exit_code, 0);
    assert!(
        planned.reply.stderr.contains("no mark"),
        "{}",
        planned.reply.stderr
    );
    assert!(matches!(planned.effect, Effect::None));
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

/// Ending is the live coordinator seat's, not merely "not this run's worker"
/// (r1 P1-1): a worker naming another run, and a pane nobody seated naming a
/// run, are both refused; the coordinator's own seat is not.
#[test]
fn only_the_runs_coordinator_seat_ends_processes() {
    let mut bench = Bench::new();
    bench.json("run-create --name sweep");
    let first = bench.ledger.runs()[0].id.clone();
    let task = bench.json("task-create --spec build")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    bench.json("run-create --name other");
    let other = bench
        .ledger
        .runs()
        .iter()
        .find(|run| run.id != first)
        .expect("a second run")
        .id
        .clone();
    let kill = "--kill 4100@0123456789ab";
    for (at, line) in [
        (pane.as_str(), format!("{VERB} {kill} --run {other}")),
        (pane.as_str(), format!("{VERB} {kill} --run {first}")),
        ("%9", format!("{VERB} {kill} --run {first}")),
        ("%9", format!("{VERB} {kill} --run {other}")),
    ] {
        let planned = bench.at(at, &line);
        assert_ne!(
            planned.reply.exit_code, 0,
            "`{line}` from {at} was not refused"
        );
        assert!(
            planned.reply.stderr.contains("coordinator"),
            "`{line}` from {at}: {}",
            planned.reply.stderr
        );
        assert!(
            matches!(planned.effect, Effect::None),
            "`{line}` from {at} planned {:?}",
            planned.effect
        );
    }
    let seated = bench.run(&format!(
        "{VERB} {kill} --run {other} --retry-request sweep-1"
    ));
    assert_eq!(seated.reply.exit_code, 0, "{}", seated.reply.stderr);
    assert!(matches!(
        seated.effect,
        Effect::ProcessSweep(ProcessSweepAsk::End { .. })
    ));
}

/// A checkout told as an empty, a relative or the root path holds no place
/// (r1 P2-2): `starts_with` would read every cwd as inside it.
#[test]
fn an_empty_relative_or_root_checkout_holds_nothing() {
    let told = |path: &str| Checkout {
        path: path.to_string(),
        run: "run-1".to_string(),
        worker: "w-1".to_string(),
        task: None,
    };
    for path in ["", "/", "w/t-1"] {
        let checkouts = [told(path)];
        assert!(
            owner("/w/t-1/crates", &checkouts).is_none(),
            "{path:?} held a place"
        );
    }
    let checkouts = [told(""), told("/"), told("/w/t-1")];
    assert_eq!(
        owner("/w/t-1/crates", &checkouts).map(|held| held.path.as_str()),
        Some("/w/t-1")
    );
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
    assert!(about.contains("코디가 목록을 읽고 고른 pid = 물음에 대한 예, 판 안·창은 예외 없음"));
}
