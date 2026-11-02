//! t-89544: the window ends what a test left behind, and nothing else. Every table
//! and every folder here is a fixture and every signal is written down by a fake:
//! no test reaches a real process.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::orchestration::process_sweep::{Signals, mark};
use crate::orchestration::stray_reap::{
    CPU_STILL_SECS, CPU_STILL_SLACK_CS, CpuProof, ENDS_PER_LOOK_MAX, Holders, Kind, MIN_AGE_SECS,
    Place, Places, Reaped, Row, Stray, Stronger, TERM_GRACE_MS, Watch, end, job_folder,
    job_measures, kind_of, line, parse_columns, parse_cpu_time, parse_elapsed, strays_of, suspects,
};

const UID: u32 = 501;
const STARTED: &str = "Mon Nov 2 10:32:00 2026";
const WINDOW_TEST: &str = "node ui/tests/window.mjs";
const CHROME: &str = "/Users/person/Library/Caches/ms-playwright/chromium_headless_shell-1248/\
                      chrome-headless-shell-mac-arm64/chrome-headless-shell --headless";
const CHROME_RENDERER: &str = "/Users/person/Library/Caches/ms-playwright/\
                               chromium_headless_shell-1248/chrome-headless-shell-mac-arm64/\
                               chrome-headless-shell --type=renderer";
const CHECKOUT: &str = "/w/t-1";
const NOW: u64 = 100_000;

fn row(pid: u32, ppid: u32, args: &str) -> Row {
    Row {
        pid,
        ppid,
        pgid: pid,
        uid: UID,
        tty: None,
        cpu_cs: Some(1_000),
        cpu_idle_now: true,
        age_secs: Some(MIN_AGE_SECS),
        started: STARTED.to_string(),
        args: args.to_string(),
        cwd: Some(CHECKOUT.to_string()),
    }
}

/// One way a row falls short of a stray.
type Change = fn(&mut Row);

fn holders() -> Holders {
    Holders {
        uid: UID,
        window: 500,
        ..Holders::default()
    }
}

/// Checkouts `/w/*` and one scratch folder.
struct Folders;

impl Places for Folders {
    fn place_of(&self, path: &str) -> Option<Place> {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match parts.as_slice() {
            ["w", checkout, ..] => Some(Place::Checkout(PathBuf::from(format!("/w/{checkout}")))),
            ["tmp", "scratchpad", ..] => Some(Place::Scratch(PathBuf::from("/tmp/scratchpad"))),
            _ => None,
        }
    }
}

/// A watch that has seen `rows` stand still for exactly the time asked.
fn watched(rows: &[Row]) -> Watch {
    let mut watch = Watch::default();
    watch.observe(rows, NOW - CPU_STILL_SECS);
    watch.observe(rows, NOW);
    watch
}

fn judged(rows: &[Row], holders: &Holders) -> Vec<u32> {
    let watch = watched(rows);
    let families = suspects(rows, holders, &watch, NOW, CpuProof::Watched);
    strays_of(&families, rows, holders, &Folders)
        .iter()
        .map(|stray| stray.pid)
        .collect()
}

/// The 10:32 scene: five nodes and their Chromes under pid 1 for five hours.
fn the_incident() -> Vec<Row> {
    let mut rows = Vec::new();
    for at in 0..5 {
        let node = 1000 + at * 10;
        let chrome = node + 1;
        rows.push(row(node, 1, WINDOW_TEST));
        rows.push(row(chrome, node, CHROME));
        for helper in 2..5 {
            let mut one = row(node + helper, chrome, CHROME_RENDERER);
            one.pgid = chrome;
            rows.push(one);
        }
    }
    rows
}

#[test]
fn the_morning_scene_is_ended_whole() {
    let rows = the_incident();
    assert_eq!(judged(&rows, &holders()).len(), rows.len());
}

#[test]
fn a_program_is_a_test_only_by_what_its_words_say() {
    assert_eq!(kind_of(WINDOW_TEST), Some(Kind::NodeTest));
    assert_eq!(
        kind_of("/opt/homebrew/bin/node --test ui/tests/ipc-census.test.mjs"),
        Some(Kind::NodeTest)
    );
    assert_eq!(
        kind_of("node /w/t-1/tools/computer-bench/long-form/run.mjs --out x"),
        Some(Kind::NodeTest)
    );
    assert_eq!(kind_of(CHROME), Some(Kind::HeadlessChrome));
    assert_eq!(
        kind_of("/w/t-1/target/debug/deps/zerocode_shell-0123456789abcdef --nocapture"),
        Some(Kind::TestBinary)
    );
    for not_a_test in [
        "node serve.js",
        "node pw-serve.js",
        "node server/src/main.mjs",
        "node",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --type=renderer",
        "/Users/person/bin/chrome-headless-shell --headless",
        "/w/t-1/target/debug/zerocode",
        "/w/t-1/target/debug/deps/libfoo.dylib",
        "/w/t-1/target/debug/build/x-0123456789abcdef/build-script-build",
        "/w/t-1/deps/zerocode_shell-0123456789abcdef",
        "target/debug/deps/zerocode_shell-0123456789abcdef",
        "cargo test -p zerocode-shell",
        "python3 tools/computer-bench/bench.py",
        "-zsh",
    ] {
        assert_eq!(kind_of(not_a_test), None, "{not_a_test}");
    }
}

/// The person's own runs: their parent lives, so nothing here is under pid 1.
#[test]
fn a_test_a_person_runs_in_a_pane_has_a_living_parent_and_is_never_a_target() {
    let shell = row(800, 1, "-zsh");
    let mut test = row(810, 800, WINDOW_TEST);
    test.tty = Some("ttys003".to_string());
    let chrome = row(811, 810, CHROME);
    assert_eq!(
        judged(&[shell, test, chrome], &holders()),
        Vec::<u32>::new()
    );
}

#[test]
fn a_chrome_counts_only_with_a_target_or_nothing_above_it() {
    // A person's own Playwright: its node is under pid 1 too, but names no test.
    let theirs = row(900, 1, "node pw-serve.js");
    let chrome = row(901, 900, CHROME);
    assert_eq!(judged(&[theirs, chrome], &holders()), Vec::<u32>::new());
    // A chrome whose node is gone.
    assert_eq!(judged(&[row(901, 1, CHROME)], &holders()), vec![901]);
    // A chrome under a person's running shell.
    let shell = row(902, 1, "-zsh");
    assert_eq!(
        judged(&[shell, row(903, 902, CHROME)], &holders()),
        Vec::<u32>::new()
    );
}

#[test]
fn one_condition_short_and_the_process_is_left_alone() {
    let cases: [(&str, Change); 11] = [
        ("younger than the floor", |row| {
            row.age_secs = Some(MIN_AGE_SECS - 1);
        }),
        ("age unreadable", |row| row.age_secs = None),
        ("CPU time unreadable", |row| row.cpu_cs = None),
        ("holds a terminal", |row| {
            row.tty = Some("ttys001".to_string())
        }),
        ("another user's", |row| row.uid = UID + 1),
        ("the place did not read", |row| row.cwd = None),
        ("outside every known folder", |row| {
            row.cwd = Some("/Users/person/Downloads".to_string());
        }),
        ("a measurement's name in the arguments", |row| {
            row.args = "node ui/tests/measure-first-turn.mjs".to_string();
        }),
        ("a measurement's name in the folder", |row| {
            row.cwd = Some("/w/측정-1".to_string());
        }),
        ("a harness in the folder", |row| {
            row.cwd = Some("/w/harness".to_string());
        }),
        ("has a parent", |row| row.ppid = 800),
    ];
    for (why, change) in cases {
        let mut one = row(1000, 1, WINDOW_TEST);
        change(&mut one);
        let watch = watched(std::slice::from_ref(&one));
        let families = suspects(
            std::slice::from_ref(&one),
            &holders(),
            &watch,
            NOW,
            CpuProof::Watched,
        );
        let found = strays_of(&families, std::slice::from_ref(&one), &holders(), &Folders);
        assert!(found.is_empty(), "{why}: {found:?}");
    }
}

#[test]
fn the_script_must_be_the_repositorys_own_and_stand_in_a_known_place() {
    let judged_at = |cwd: &str, args: &str| {
        let mut one = row(1000, 1, args);
        one.cwd = Some(cwd.to_string());
        judged(std::slice::from_ref(&one), &holders())
    };
    assert_eq!(judged_at("/w/t-1", "node ui/tests/window.mjs"), vec![1000]);
    assert_eq!(judged_at("/w/t-1/ui", "node tests/window.mjs"), vec![1000]);
    assert_eq!(
        judged_at("/tmp/scratchpad/run", "node /w/t-1/ui/tests/window.mjs"),
        vec![1000],
        "an agent's scratch folder, the checkout's own test"
    );
    assert_eq!(
        judged_at("/tmp/scratchpad/copy", "node ui/tests/window.mjs"),
        vec![1000],
        "an archive of a commit in a scratch folder"
    );
    assert_eq!(
        judged_at("/w/t-1", "node tools/computer-bench/long-form/run.mjs"),
        vec![1000]
    );
    for (cwd, args, why) in [
        ("/w/t-1", "node other/tests/window.mjs", "not ui/tests"),
        ("/w/t-1", "node ui/tests/deep/window.mjs", "below ui/tests"),
        ("/w/t-1", "node ui/tests/window.js", "not .mjs"),
        ("/w/t-1", "node ../t-2/ui/tests/window.mjs", "a climb out"),
        (
            "/w/t-1",
            "node --require x ui/tests/window.mjs",
            "an option's value",
        ),
        (
            "/w/t-1",
            "node /elsewhere/ui/tests/window.mjs",
            "another folder",
        ),
        (
            "/Users/person",
            "node /w/t-1/ui/tests/window.mjs",
            "a person's cwd",
        ),
    ] {
        assert_eq!(judged_at(cwd, args), Vec::<u32>::new(), "{why}");
    }
}

#[test]
fn a_test_binary_counts_only_in_a_checkouts_own_target() {
    let binary = |path: &str| {
        let args = format!("{path} --nocapture");
        judged(&[row(1000, 1, &args)], &holders())
    };
    assert_eq!(
        binary("/w/t-1/target/debug/deps/zerocode_shell-0123456789abcdef"),
        vec![1000]
    );
    assert_eq!(
        binary("/elsewhere/target/debug/deps/zerocode_shell-0123456789abcdef"),
        Vec::<u32>::new()
    );
    assert_eq!(
        binary("/w/t-1/sub/target/debug/deps/zerocode_shell-0123456789abcdef"),
        Vec::<u32>::new(),
        "a target below the checkout's top is not the checkout's"
    );
}

#[test]
fn cpu_that_moves_keeps_it_and_stillness_has_to_last() {
    // Seen once: nobody knows yet.
    let rows = vec![row(1000, 1, WINDOW_TEST)];
    let mut watch = Watch::default();
    watch.observe(&rows, NOW);
    assert!(
        suspects(&rows, &holders(), &watch, NOW, CpuProof::Watched).is_empty(),
        "first sight proves nothing"
    );
    // Short of the span by a second.
    assert!(
        suspects(
            &rows,
            &holders(),
            &watch,
            NOW + CPU_STILL_SECS - 1,
            CpuProof::Watched
        )
        .is_empty()
    );
    assert_eq!(
        suspects(
            &rows,
            &holders(),
            &watch,
            NOW + CPU_STILL_SECS,
            CpuProof::Watched
        )
        .len(),
        1
    );
    // Work between two looks starts the span over.
    let mut working = rows.clone();
    working[0].cpu_cs = Some(50_000);
    watch.observe(&working, NOW + CPU_STILL_SECS);
    assert!(
        suspects(
            &working,
            &holders(),
            &watch,
            NOW + CPU_STILL_SECS,
            CpuProof::Watched
        )
        .is_empty(),
        "it was using CPU"
    );
    // An idle browser's own timers do not.
    let mut ticking = rows.clone();
    ticking[0].cpu_cs = Some(1_010);
    let mut watch = Watch::default();
    watch.observe(&rows, NOW);
    watch.observe(&ticking, NOW + CPU_STILL_SECS);
    assert_eq!(
        suspects(
            &ticking,
            &holders(),
            &watch,
            NOW + CPU_STILL_SECS,
            CpuProof::Watched
        )
        .len(),
        1,
        "0.1 s in ten minutes is an idle timer"
    );
    // A pid reused by a new start is a new process with no history.
    let mut reused = rows.clone();
    reused[0].started = "Mon Nov 2 16:00:00 2026".to_string();
    let mut watch = Watch::default();
    watch.observe(&rows, NOW);
    watch.observe(&reused, NOW + CPU_STILL_SECS);
    assert!(
        suspects(
            &reused,
            &holders(),
            &watch,
            NOW + CPU_STILL_SECS,
            CpuProof::Watched
        )
        .is_empty()
    );
}

/// The slack is the edge: one hundredth past it is work, and so is CPU time that
/// reads less than before, which only a new process under the same start can show.
#[test]
fn the_slack_is_an_edge_and_a_fall_starts_the_watch_over() {
    let rows = vec![row(1000, 1, WINDOW_TEST)];
    let base = rows[0].cpu_cs.expect("cpu");
    let after = |cpu_cs: u64| {
        let mut later = rows.clone();
        later[0].cpu_cs = Some(cpu_cs);
        let mut watch = Watch::default();
        watch.observe(&rows, NOW);
        watch.observe(&later, NOW + CPU_STILL_SECS);
        suspects(
            &later,
            &holders(),
            &watch,
            NOW + CPU_STILL_SECS,
            CpuProof::Watched,
        )
        .len()
    };
    assert_eq!(
        after(base + CPU_STILL_SLACK_CS),
        1,
        "the slack itself is still"
    );
    assert_eq!(after(base + CPU_STILL_SLACK_CS + 1), 0, "past it is work");
    assert_eq!(after(base - 1), 0, "a fall is not stillness");
}

#[test]
fn one_busy_child_keeps_the_whole_family() {
    let mut rows = vec![
        row(1000, 1, WINDOW_TEST),
        row(1001, 1000, CHROME),
        row(1002, 1001, CHROME_RENDERER),
    ];
    let mut watch = Watch::default();
    watch.observe(&rows, NOW - CPU_STILL_SECS);
    rows[2].cpu_cs = Some(60_000);
    watch.observe(&rows, NOW);
    assert!(suspects(&rows, &holders(), &watch, NOW, CpuProof::Watched).is_empty());
}

#[test]
fn a_child_this_cannot_name_keeps_the_whole_family() {
    let rows = [
        row(1000, 1, WINDOW_TEST),
        row(1001, 1000, "/w/t-1/target/debug/zerocode --serve"),
    ];
    assert_eq!(judged(&rows, &holders()), Vec::<u32>::new());
}

#[test]
fn work_that_is_running_is_never_touched() {
    let test = || row(1000, 1, WINDOW_TEST);

    // The window, a pane.
    let mut held = holders();
    held.window = 1000;
    assert!(judged(&[test()], &held).is_empty(), "the window itself");
    let mut held = holders();
    held.panes = vec![1000];
    assert!(judged(&[test()], &held).is_empty(), "a pane's own");

    // A build line job on record, and a child that outlived its parent in the job's
    // group.
    let mut held = holders();
    held.recorded = vec![1000];
    assert!(judged(&[test()], &held).is_empty(), "recorded");
    let job = row(700, 650, "zsh /q/run/gate.job");
    let mut child = test();
    child.pgid = 700;
    let mut held = holders();
    held.recorded = vec![700];
    assert!(
        judged(&[job.clone(), child.clone()], &held).is_empty(),
        "in a recorded job's group"
    );
    // The record's shell is gone; its group still stands.
    let mut gone = child.clone();
    gone.pgid = 700;
    assert!(
        judged(&[gone], &held).is_empty(),
        "a group whose recorded leader has left"
    );
    // No record at all: the group rule alone keeps it.
    assert!(
        judged(&[job, child], &holders()).is_empty(),
        "someone else is alive in its group"
    );

    // A job standing in the same checkout.
    let mut held = holders();
    held.busy_folders = vec![CHECKOUT.to_string()];
    assert!(judged(&[test()], &held).is_empty(), "beside a running job");

    // A lane, or a measuring build line.
    let mut held = holders();
    held.stand_down = true;
    assert!(judged(&[test()], &held).is_empty(), "the look stands down");
}

#[test]
fn a_group_shared_by_two_strays_goes_together_and_one_foreigner_keeps_both() {
    let mut first = row(1000, 1, WINDOW_TEST);
    let mut second = row(1010, 1, WINDOW_TEST);
    first.pgid = 990;
    second.pgid = 990;
    assert_eq!(
        judged(&[first.clone(), second.clone()], &holders()),
        vec![1000, 1010]
    );
    let mut foreigner = row(1020, 800, "zsh batch.sh");
    foreigner.pgid = 990;
    assert!(judged(&[first, second, foreigner], &holders()).is_empty());
}

/// The coordinator's own act stands in only for the watch, and for nothing else.
#[test]
fn a_named_pid_answers_for_the_watch_and_not_for_anything_else() {
    let one = row(1000, 1, WINDOW_TEST);
    let mut watch = Watch::default();
    watch.observe(std::slice::from_ref(&one), NOW);
    let named = |rows: &[Row], watch: &Watch| {
        let families = suspects(rows, &holders(), watch, NOW, CpuProof::NamedByCoordinator);
        strays_of(&families, rows, &holders(), &Folders).len()
    };
    assert_eq!(named(std::slice::from_ref(&one), &watch), 1);

    let mut busy = one.clone();
    busy.cpu_idle_now = false;
    assert_eq!(named(std::slice::from_ref(&busy), &watch), 0, "CPU now");

    let mut moved = one.clone();
    moved.cpu_cs = Some(90_000);
    let mut seen_moving = watch.clone();
    seen_moving.observe(std::slice::from_ref(&moved), NOW);
    assert_eq!(
        named(std::slice::from_ref(&moved), &seen_moving),
        0,
        "the window saw it move inside the span"
    );

    for change in [
        |row: &mut Row| row.age_secs = Some(MIN_AGE_SECS - 1),
        |row: &mut Row| row.tty = Some("ttys001".to_string()),
        |row: &mut Row| row.cwd = Some("/Users/person".to_string()),
        |row: &mut Row| row.ppid = 800,
        |row: &mut Row| row.args = "node serve.js".to_string(),
    ] {
        let mut changed = one.clone();
        change(&mut changed);
        assert_eq!(
            named(std::slice::from_ref(&changed), &watch),
            0,
            "{changed:?}"
        );
    }
}

/// Writes down each signal instead of sending it. `dies_on_term` are the pids a TERM
/// ends; everything else ignores it.
struct Fake {
    table: RefCell<HashMap<u32, (String, String)>>,
    dies_on_term: Vec<u32>,
    log: RefCell<Vec<String>>,
    clock: RefCell<u64>,
    killable: bool,
}

impl Fake {
    fn over(rows: &[Row], dies_on_term: &[u32]) -> Self {
        Self {
            table: RefCell::new(
                rows.iter()
                    .map(|row| (row.pid, (row.started.clone(), row.args.clone())))
                    .collect(),
            ),
            dies_on_term: dies_on_term.to_vec(),
            log: RefCell::new(Vec::new()),
            clock: RefCell::new(0),
            killable: true,
        }
    }
}

impl Signals for Fake {
    fn reread(&self, pid: u32) -> Option<(String, String)> {
        self.table.borrow().get(&pid).cloned()
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        self.log.borrow_mut().push(format!("TERM {pid}"));
        if self.dies_on_term.contains(&pid) {
            self.table.borrow_mut().remove(&pid);
        }
        Ok(())
    }

    fn pause(&self, ms: u64) {
        *self.clock.borrow_mut() += ms;
    }

    fn now_ms(&self) -> u64 {
        *self.clock.borrow()
    }
}

impl Stronger for Fake {
    fn kill(&self, pid: u32) -> Result<(), String> {
        self.log
            .borrow_mut()
            .push(format!("KILL {pid} at {}", *self.clock.borrow()));
        if !self.killable {
            return Err("no".to_string());
        }
        self.table.borrow_mut().remove(&pid);
        Ok(())
    }
}

fn strays(rows: &[Row]) -> Vec<Stray> {
    let families: Vec<Vec<u32>> = rows.iter().map(|row| vec![row.pid]).collect();
    strays_of(&families, rows, &holders(), &Folders)
}

#[test]
fn term_first_and_kill_only_what_still_stands_after_the_grace() {
    let rows = vec![row(1000, 1, WINDOW_TEST), row(1010, 1, WINDOW_TEST)];
    let fake = Fake::over(&rows, &[1000]);
    let reaped = end(&strays(&rows), &rows, &fake, &fake);
    assert_eq!(reaped.by_term, 1, "{reaped:?}");
    assert_eq!(reaped.by_kill, 1, "{reaped:?}");
    assert_eq!(reaped.left, 0);
    let log = fake.log.borrow();
    assert_eq!(log[0], "TERM 1000");
    assert_eq!(log[1], "TERM 1010");
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(log[2].starts_with("KILL 1010"), "{log:?}");
    let at: u64 = log[2].rsplit(' ').next().unwrap().parse().unwrap();
    assert!(at >= TERM_GRACE_MS, "the grace was given: {at}");
}

#[test]
fn a_reused_pid_is_read_again_before_each_signal_and_left_alone() {
    let rows = vec![row(1000, 1, WINDOW_TEST)];
    // Before the TERM: another process under the pid.
    let fake = Fake::over(&rows, &[]);
    fake.table.borrow_mut().insert(
        1000,
        ("later".to_string(), "/Applications/Other".to_string()),
    );
    let reaped = end(&strays(&rows), &rows, &fake, &fake);
    assert_eq!(reaped.changed, 1);
    assert!(fake.log.borrow().is_empty(), "{:?}", fake.log.borrow());

    // Between the TERM and the KILL.
    struct ReusedAfterTerm(Fake);
    impl Signals for ReusedAfterTerm {
        fn reread(&self, pid: u32) -> Option<(String, String)> {
            self.0.reread(pid)
        }
        fn terminate(&self, pid: u32) -> Result<(), String> {
            self.0.terminate(pid)
        }
        fn pause(&self, ms: u64) {
            if ms > 1_000 {
                self.0.table.borrow_mut().insert(
                    1000,
                    ("later".to_string(), "/Applications/Other".to_string()),
                );
            }
            self.0.pause(ms);
        }
        fn now_ms(&self) -> u64 {
            self.0.now_ms()
        }
    }
    let reused = ReusedAfterTerm(Fake::over(&rows, &[]));
    let reaped = end(&strays(&rows), &rows, &reused, &reused.0);
    assert_eq!(reaped.by_kill, 0, "{reaped:?}");
    assert!(
        reused
            .0
            .log
            .borrow()
            .iter()
            .all(|one| !one.starts_with("KILL")),
        "{:?}",
        reused.0.log.borrow()
    );
}

#[test]
fn a_kill_that_cannot_land_is_said_as_left() {
    let rows = vec![row(1000, 1, WINDOW_TEST)];
    let mut fake = Fake::over(&rows, &[]);
    fake.killable = false;
    let reaped = end(&strays(&rows), &rows, &fake, &fake);
    assert_eq!(reaped.left, 1, "{reaped:?}");
}

/// The line names and counts only what this look ended: a pid already gone when
/// its TERM was due, or another process by then, is neither.
#[test]
fn only_what_was_ended_is_named_and_counted() {
    const OTHER_TEST: &str = "node ui/tests/other.mjs";
    const THIRD_TEST: &str = "node ui/tests/third.mjs";
    let rows = vec![
        row(1000, 1, WINDOW_TEST),
        row(1010, 1, OTHER_TEST),
        row(1020, 1, THIRD_TEST),
    ];
    let fake = Fake::over(&rows, &[1000]);
    fake.table.borrow_mut().remove(&1010);
    fake.table.borrow_mut().insert(
        1020,
        ("later".to_string(), "/Applications/Other".to_string()),
    );
    let reaped = end(&strays(&rows), &rows, &fake, &fake);
    assert_eq!(reaped.said, vec![WINDOW_TEST.to_string()], "{reaped:?}");
    assert_eq!(
        (reaped.by_term, reaped.gone_before, reaped.changed),
        (1, 1, 1)
    );
    let said = line(&reaped);
    assert!(
        said.starts_with("stray-reap: ended 1 left by tests (1 node ui/tests/window.mjs)"),
        "{said}"
    );
    assert!(
        !said.contains("other.mjs") && !said.contains("third.mjs"),
        "{said}"
    );

    let all_gone = Fake::over(&[], &[]);
    let reaped = end(&strays(&rows[..1]), &rows[..1], &all_gone, &all_gone);
    assert_eq!(reaped.gone_before, 1);
    assert!(
        !reaped.did_anything(),
        "a look that found them gone says nothing"
    );
}

#[test]
fn a_look_ends_only_so_many_and_takes_the_rest_next_time() {
    let rows: Vec<Row> = (0..u32::try_from(ENDS_PER_LOOK_MAX).unwrap() + 3)
        .map(|at| row(1000 + at, 1, WINDOW_TEST))
        .collect();
    let all: Vec<u32> = rows.iter().map(|row| row.pid).collect();
    let fake = Fake::over(&rows, &all);
    let reaped = end(&strays(&rows), &rows, &fake, &fake);
    assert_eq!(fake.log.borrow().len(), ENDS_PER_LOOK_MAX);
    assert_eq!(reaped.by_term, ENDS_PER_LOOK_MAX);
}

#[test]
fn the_marks_are_process_sweeps_own() {
    let rows = vec![row(1000, 1, WINDOW_TEST)];
    let found = strays(&rows);
    assert_eq!(found[0].target.mark, mark(1000, STARTED, WINDOW_TEST));
}

#[test]
fn the_line_says_what_and_how_many_and_no_folder_of_anybodys() {
    let rows = the_incident();
    let all: Vec<u32> = rows.iter().map(|row| row.pid).collect();
    let fake = Fake::over(&rows, &all);
    let found = strays_of(
        &suspects(&rows, &holders(), &watched(&rows), NOW, CpuProof::Watched),
        &rows,
        &holders(),
        &Folders,
    );
    let said = line(&end(&found, &rows, &fake, &fake));
    assert!(
        said.starts_with("stray-reap: ended 25 left by tests"),
        "{said}"
    );
    assert!(said.contains("20 chrome-headless-shell"), "{said}");
    assert!(said.contains("5 node ui/tests/window.mjs"), "{said}");
    assert!(said.contains("25 by TERM, 0 by KILL"), "{said}");
    assert!(!said.contains("/Users"), "{said}");
    assert!(!said.contains(CHECKOUT), "{said}");
    assert_eq!(said.lines().count(), 1);
    assert!(
        !Reaped::default().did_anything(),
        "a quiet look says nothing"
    );
}

#[test]
fn the_tables_words_read_or_say_nothing() {
    assert_eq!(parse_cpu_time("0:00.15"), Some(15));
    assert_eq!(parse_cpu_time("132:01.50"), Some(792_150));
    assert_eq!(parse_cpu_time("01:02:03"), Some(372_300));
    assert_eq!(parse_cpu_time("garbage"), None);
    assert_eq!(parse_cpu_time(""), None);
    assert_eq!(parse_elapsed("45:00"), Some(2_700));
    assert_eq!(parse_elapsed("05:02:11"), Some(18_131));
    assert_eq!(parse_elapsed("19-02:44:57"), Some(1_651_497));
    assert_eq!(parse_elapsed("12"), None);

    let good = ["9", "1", "9", "501", "??", "0:00.10", "45:00", "0.0"];
    let read = parse_columns(&good).expect("a row");
    assert_eq!(read.tty, None);
    assert!(read.cpu_idle_now);
    let on_terminal = ["9", "1", "9", "501", "ttys001", "0:00.10", "45:00", "0.3"];
    let read = parse_columns(&on_terminal).expect("a row");
    assert_eq!(read.tty.as_deref(), Some("ttys001"));
    assert!(!read.cpu_idle_now);
    assert_eq!(parse_columns(&good[..7]), None);
    let bad_pid = ["x", "1", "9", "501", "??", "0:00.10", "45:00", "0.0"];
    assert_eq!(parse_columns(&bad_pid), None);
}

#[test]
fn a_jobs_folder_comes_from_its_header_only() {
    assert_eq!(
        job_folder("# who: w-1\n# cwd: /w/t-1\necho hi\n# cwd: /other\n").as_deref(),
        Some("/w/t-1")
    );
    assert_eq!(job_folder("echo hi\n# cwd: /other\n"), None);
    assert_eq!(job_folder("# cwd:\n"), None);
}

#[test]
fn a_job_that_measures_says_so_in_its_what_line() {
    assert!(job_measures(
        "# who: c\n# what: 1.1.115 low-spec 실측 r3\necho\n"
    ));
    assert!(job_measures("# what: Harness run\n"));
    assert!(!job_measures("# what: 1.1.115 — g2c-shell-tests\n"));
    assert!(
        !job_measures("# what: build\necho measure\n"),
        "the header only, never the job's commands"
    );
    assert!(!job_measures("# whatever: measure\n"), "the key is whole");
}
