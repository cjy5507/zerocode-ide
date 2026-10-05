use std::time::{Duration, SystemTime};

use zerocode_core::artifact::Origin;
use zerocode_core::evidence_digest::{STEP_PANE_KEY, STEP_TASK_KEY};

use super::*;

/// A day in 2026, in epoch milliseconds: the fixtures' files are dated from it
/// so that "newest first" is theirs to decide.
const DAY_MS: i64 = 1_790_000_000_000;

/// A store over a temporary folder, and the folder the fixtures' files lie in.
struct Bench {
    _dir: tempfile::TempDir,
    files: PathBuf,
    store: Store,
}

impl Bench {
    fn new(limits: Limits) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let files = dir.path().join("files");
        let store = Store::open(&dir.path().join("data"), limits);
        Self {
            _dir: dir,
            files,
            store,
        }
    }

    /// Write a file dated `minutes` after the bench's day and catalog it where
    /// it lies. The catalog reads a row's time off its file.
    fn place(
        &self,
        name: &str,
        text: &str,
        minutes: i64,
        source: Source,
        origin: Origin,
    ) -> String {
        let path = self.files.join(name);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
        let at = SystemTime::UNIX_EPOCH
            + Duration::from_millis((DAY_MS + minutes * 60_000).unsigned_abs());
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(at))
            .expect("dated");
        self.store
            .register_in_place(&path, source, origin, DAY_MS)
            .expect("registers")
            .id
    }
}

/// What the ledger vouches for of a worker's row.
fn of(worker: &str, task: &str) -> Origin {
    Origin {
        run: Some("run-1".into()),
        worker: Some(worker.into()),
        task: Some(task.into()),
        work_summary: Some(format!("the work of {task}")),
        agent: Some("claude".into()),
        ..Origin::default()
    }
}

/// A Computer Use session's origin: the automation, and no task.
fn session() -> Origin {
    Origin {
        automation: Some(crate::agent_tools_runtime::EVIDENCE_AUTOMATION_ID.into()),
        ..Origin::default()
    }
}

/// One line of a step log as the window's recorder writes it, taken for `task`
/// when one is given.
fn step(n: usize, task: Option<&str>) -> String {
    let observation = task.map(
        |task| serde_json::json!({ STEP_TASK_KEY: task, STEP_PANE_KEY: "term-3", "act_ms": 4 }),
    );
    let line = crate::run_evidence::Step {
        n,
        at_epoch_ms: DAY_MS + n as i64,
        tool: "computer".into(),
        verb: "click".into(),
        argv: vec!["click".into(), "--index".into(), n.to_string()],
        ok: true,
        observation,
        error: None,
        code: None,
        acts: true,
        shot: None,
        frame: None,
        frame_skipped: None,
    };
    serde_json::to_string(&line).expect("serialises") + "\n"
}

const PASSING_LOG: &str =
    "test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n";
const FAILING_LOG: &str = "test a::one ... FAILED\n\
     test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";

/// The parts of a line always add up: the files are the pictures, the logs and
/// the rest.
fn the_parts_add_up(parts: &Parts) {
    assert_eq!(
        parts.files,
        parts.pictures + parts.logs + parts.other,
        "files = pictures + logs + other: {parts:?}"
    );
}

/// A session's step log is tagged with the tasks its steps were taken for —
/// from what was appended since the row was last read, so a long log is not
/// read again for every new step; a log written anew is read from its start,
/// and the tags are in the catalog a reopened window reads.
#[test]
fn a_step_logs_row_is_tagged_with_the_tasks_its_steps_name() {
    let bench = Bench::new(Limits::default());
    let log = "sessions/one/steps.jsonl";
    let tags = |id: &str| bench.store.get(id).expect("the row").tags;
    let first = step(1, Some("t-10")) + &step(2, None);
    let id = bench.place(log, &first, 1, Source::Evidence, session());
    assert_eq!(tags(&id), ["task:t-10"]);

    let longer = first.clone() + &step(3, Some("t-20")) + &step(4, Some("t-10"));
    let again = bench.place(log, &longer, 2, Source::Evidence, session());
    assert_eq!(again, id, "the same file is the same row");
    assert_eq!(tags(&id), ["task:t-10", "task:t-20"]);

    // The first steps are not read again: a log whose old lines were changed
    // in place, with the same length before the new line, keeps its old tags.
    let changed_in_place = longer.replace("t-20", "t-21") + &step(5, None);
    bench.place(log, &changed_in_place, 3, Source::Evidence, session());
    assert_eq!(
        tags(&id),
        ["task:t-10", "task:t-20"],
        "only the appended bytes were read"
    );

    // A shorter log was written anew.
    bench.place(log, &step(1, Some("t-30")), 4, Source::Evidence, session());
    assert_eq!(tags(&id), ["task:t-30"]);

    // A frame of the same session is not a step log.
    let frame = bench.place(
        "sessions/one/001-computer-click.png",
        "png",
        4,
        Source::Evidence,
        session(),
    );
    assert_eq!(tags(&frame), Vec::<String>::new());

    let data = bench
        .store
        .root()
        .parent()
        .expect("the data root")
        .to_path_buf();
    let reopened = Store::open(&data, Limits::default());
    assert_eq!(
        reopened.get(&id).expect("the row").tags,
        ["task:t-30"],
        "the tags are written with the row"
    );
}

/// The fixture of the two listing tests: task `t-2` (the newer) with two
/// workers' reports, a page, pictures, logs and a file that is gone; task
/// `t-1` with one page; and rows no task is linked to.
fn two_tasks(bench: &Bench) {
    let report = Source::WorkerReport;
    let kept = Source::Evidence;
    bench.place(
        "t2/first.md",
        "# The first attempt\n",
        10,
        report,
        of("w-1", "t-2"),
    );
    bench.place(
        "t2/final.md",
        "# The second attempt\n",
        30,
        report,
        of("w-2", "t-2"),
    );
    bench.place(
        "t2/page.html",
        "<title>A page</title>",
        20,
        Source::AgentPage,
        of("w-2", "t-2"),
    );
    bench.place(
        "t2/shots/dark-before.png",
        "png",
        21,
        kept,
        of("w-2", "t-2"),
    );
    bench.place("t2/shots/dark-after.png", "png", 22, kept, of("w-2", "t-2"));
    bench.place("t2/logs/green.log", PASSING_LOG, 23, kept, of("w-2", "t-2"));
    bench.place("t2/logs/red.log", FAILING_LOG, 24, kept, of("w-2", "t-2"));
    bench.place("t2/logs/red.rc", "0\n", 27, kept, of("w-2", "t-2"));
    bench.place(
        "t2/numbers.json",
        r#"{"first_paint_ms":41}"#,
        25,
        kept,
        of("w-2", "t-2"),
    );
    bench.place("t2/gone.log", PASSING_LOG, 26, kept, of("w-2", "t-2"));
    std::fs::remove_file(bench.files.join("t2/gone.log")).expect("removed");
    bench.place(
        "t1/notes.md",
        "# Notes\n",
        5,
        Source::AgentPage,
        of("w-0", "t-1"),
    );

    bench.place(
        "loose/empty-origin.md",
        "# A report of nobody\n",
        1,
        report,
        Origin::default(),
    );
    bench.place(
        "loose/worker-only.md",
        "# A report with a worker and no task\n",
        2,
        report,
        Origin {
            worker: Some("w-9".into()),
            ..Origin::default()
        },
    );
    bench.place(
        "loose/page.html",
        "<title>Loose</title>",
        3,
        Source::AgentPage,
        Origin::default(),
    );
    for (folder, name) in [
        ("a", "steps.jsonl"),
        ("a", "001-computer-click.png"),
        ("b", "state.json"),
    ] {
        let text = if name == "steps.jsonl" {
            step(1, None)
        } else {
            "{}".to_string()
        };
        bench.place(
            &format!("sessions/{folder}/{name}"),
            &text,
            4,
            kept,
            session(),
        );
    }
}

/// One line for each task, newest first; what each holds is counted in parts
/// that add up, a file that is gone is counted as gone and as nothing else,
/// and the rows no task is linked to are counted the same way — a session's
/// files as files and as sessions.
#[test]
fn a_task_line_counts_what_the_task_holds_in_parts_that_add_up() {
    let bench = Bench::new(Limits::default());
    two_tasks(&bench);
    let listing = bench.store.tasks(&Filter::default());
    let names: Vec<&str> = listing
        .tasks
        .iter()
        .map(|line| line.task.as_str())
        .collect();
    assert_eq!(names, ["t-2", "t-1"], "newest first");
    assert_eq!((listing.total, listing.truncated), (2, false));

    let line = &listing.tasks[0];
    assert_eq!(
        line.parts,
        Parts {
            reports: 2,
            pages: 1,
            files: 6,
            pictures: 2,
            logs: 2,
            other: 2,
            missing: 1,
        }
    );
    the_parts_add_up(&line.parts);
    assert_eq!(line.attempts, 2, "two workers handed a report in");
    assert_eq!(line.title, "The second attempt", "the newest report leads");
    assert_eq!(line.work.as_deref(), Some("the work of t-2"));
    assert_eq!(
        bench
            .store
            .get(line.lead.as_deref().expect("a lead"))
            .map(|row| row.kind),
        Some(ArtifactKind::Report)
    );
    assert_eq!(line.modified_ms, DAY_MS + 30 * 60_000);

    let page_only = &listing.tasks[1];
    assert_eq!(
        (
            page_only.parts.pages,
            page_only.parts.reports,
            page_only.attempts
        ),
        (1, 0, 0)
    );
    assert_eq!(page_only.title, "Notes", "without a report, its page leads");

    let unlinked = listing.unlinked;
    assert_eq!(
        (
            unlinked.parts.reports,
            unlinked.reports_without_origin,
            unlinked.parts.pages
        ),
        (2, 1, 1)
    );
    assert_eq!(
        (
            unlinked.parts.files,
            unlinked.session_files,
            unlinked.sessions
        ),
        (3, 3, 2),
        "three files of two sessions"
    );
    the_parts_add_up(&unlinked.parts);
}

/// A search on this tab is for a task: its words are asked of the line — the
/// task's id, its words, its title — while the gallery's other filters narrow
/// the rows a line is counted from. Past the table's count the listing says so.
#[test]
fn a_task_search_asks_the_line_and_the_table_bounds_the_listing() {
    let bench = Bench::new(Limits::default());
    two_tasks(&bench);
    let found = |query: &str| {
        let listing = bench.store.tasks(&Filter {
            query: query.into(),
            ..Filter::default()
        });
        listing
            .tasks
            .into_iter()
            .map(|line| line.task)
            .collect::<Vec<_>>()
    };
    assert_eq!(found("t-1"), ["t-1"]);
    assert_eq!(
        found("second ATTEMPT"),
        ["t-2"],
        "the lead's title, in any case"
    );
    assert_eq!(found("work of"), ["t-2", "t-1"], "the task's words");
    assert_eq!(
        found("first attempt"),
        Vec::<String>::new(),
        "not every row's words"
    );

    let of_w1 = bench.store.tasks(&Filter {
        worker: Some("w-1".into()),
        ..Filter::default()
    });
    assert_eq!(of_w1.tasks.len(), 1);
    assert_eq!(
        of_w1.tasks[0].parts.reports, 1,
        "counted from the rows the filter admits"
    );

    let bounded = Bench::new(Limits {
        task_lines_max: 1,
        ..Limits::default()
    });
    two_tasks(&bounded);
    let listing = bounded.store.tasks(&Filter::default());
    assert_eq!(
        (listing.tasks.len(), listing.total, listing.truncated),
        (1, 2, true)
    );
    assert_eq!(listing.tasks[0].task, "t-2", "the newest is the one kept");
}

/// Everything one task holds: its reports with the attempt each worker was, its
/// pictures paired by their names, each log with what it says and how its run
/// ended, its pages — and the sum of the runs, beside its parts.
#[test]
fn a_bundle_holds_everything_one_task_has_with_what_each_log_says() {
    let bench = Bench::new(Limits::default());
    two_tasks(&bench);
    // The first worker reported again after the second: still the first attempt.
    bench.place(
        "t2/addendum.md",
        "# An addendum\n",
        40,
        Source::WorkerReport,
        of("w-1", "t-2"),
    );
    assert_eq!(bench.store.bundle("t-404"), None, "a task no row names");
    let found = bench.store.bundle("t-2");
    assert!(found.is_some(), "a task's rows are its bundle");
    let bundle = found.unwrap_or_default();
    // A row by the name of its file: a log is told apart by what it is called.
    let name = |id: &str| {
        bundle
            .rows
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| row.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };

    let reports: Vec<(String, usize)> = bundle
        .reports
        .iter()
        .map(|held| (name(&held.id), held.attempt))
        .collect();
    assert_eq!(
        reports,
        [
            ("addendum.md".to_string(), 1),
            ("final.md".to_string(), 2),
            ("first.md".to_string(), 1),
        ],
        "newest first, each with its worker's place among the attempts"
    );
    assert_eq!(
        bundle.line.title, "An addendum",
        "the final report's own heading"
    );

    assert_eq!(bundle.pictures.len(), 1, "{:?}", bundle.pictures);
    let pair = &bundle.pictures[0];
    assert_eq!(pair.name, "dark");
    assert_eq!(
        (
            pair.before.as_deref().map(name),
            pair.after.as_deref().map(name)
        ),
        (
            Some("dark-before.png".to_string()),
            Some("dark-after.png".to_string())
        )
    );

    /// What one evidence line says of its test run: passed, failed, the verdict, the exit code.
    type RunSaid = Option<(usize, usize, Verdict, Option<i32>)>;
    let said: Vec<(String, RunSaid)> = bundle
        .evidence
        .iter()
        .map(|line| {
            (
                name(&line.id),
                line.tests
                    .as_ref()
                    .map(|tests| (tests.passed, tests.failed, tests.verdict, tests.rc)),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            ("red.rc".to_string(), Some((0, 0, Verdict::Pass, Some(0)))),
            ("numbers.json".to_string(), None),
            // The failures under an exit code of 0: what the run was for.
            (
                "red.log".to_string(),
                Some((3, 1, Verdict::Intended, Some(0)))
            ),
            ("green.log".to_string(), Some((12, 0, Verdict::Pass, None))),
        ],
        "every file is its own line, newest first"
    );
    assert_eq!(
        bundle.tally,
        Tally {
            runs: 3,
            passed: 15,
            failed: 1,
            ignored: 1,
            pass: 2,
            fail: 0,
            intended: 1,
            unknown: 0,
        },
        "the sum of the lines above, and of nothing else"
    );
    assert_eq!(bundle.unread, 0);
    assert_eq!(
        bundle.pages.iter().map(|id| name(id)).collect::<Vec<_>>(),
        ["page.html"]
    );
    assert_eq!(bundle.missing.len(), 1);
    assert_eq!(
        name(&bundle.missing[0]),
        "gone.log",
        "the gone file is still named"
    );

    // What the bundle lists is what its line counts.
    let parts = bundle.line.parts;
    the_parts_add_up(&parts);
    assert_eq!(bundle.reports.len(), parts.reports);
    assert_eq!(bundle.evidence.len(), parts.logs + parts.other);
    let pictured: usize = bundle
        .pictures
        .iter()
        .map(|held| {
            [&held.before, &held.after, &held.single]
                .into_iter()
                .flatten()
                .count()
        })
        .sum();
    assert_eq!(pictured, parts.pictures);
    assert_eq!(bundle.pages.len(), parts.pages);
    assert_eq!(
        bundle.rows.len(),
        parts.reports + parts.pages + parts.files + parts.missing
    );

    // Past the table's count a file is listed and not read.
    let bounded = Bench::new(Limits {
        bundle_digests_max: 1,
        ..Limits::default()
    });
    two_tasks(&bounded);
    let bundle = bounded.store.bundle("t-2").expect("the bundle");
    assert_eq!((bundle.evidence.len(), bundle.unread), (4, 3));
    assert_eq!(bundle.tally.runs, 1, "only the first file was read");
    assert!(bundle.evidence[1..].iter().all(|line| line.tests.is_none()));
}

/// A Computer Use session joins the tasks its steps were taken for: its step
/// log is one of each task's files and says how many of its steps were that
/// task's; the frames beside it are linked to no task.
#[test]
fn a_session_joins_the_tasks_its_steps_name() {
    let bench = Bench::new(Limits::default());
    bench.place(
        "t1/report.md",
        "# A report\n",
        1,
        Source::WorkerReport,
        of("w-1", "t-1"),
    );
    let log = step(1, Some("t-1")) + &step(2, Some("t-2")) + &step(3, Some("t-1")) + &step(4, None);
    let steps = bench.place(
        "sessions/a/steps.jsonl",
        &log,
        2,
        Source::Evidence,
        session(),
    );
    bench.place(
        "sessions/a/001-computer-click.png",
        "png",
        2,
        Source::Evidence,
        session(),
    );

    let listing = bench.store.tasks(&Filter::default());
    assert_eq!(listing.tasks.len(), 2, "both tasks have a line");
    let line = |task: &str| {
        listing
            .tasks
            .iter()
            .find(|line| line.task == task)
            .cloned()
            .expect("the line")
    };
    assert_eq!((line("t-1").sessions, line("t-1").parts.other), (1, 1));
    assert_eq!((line("t-2").sessions, line("t-2").parts.other), (1, 1));
    assert_eq!(line("t-2").lead.as_deref(), Some(steps.as_str()));
    the_parts_add_up(&line("t-1").parts);
    assert_eq!(
        (listing.unlinked.session_files, listing.unlinked.sessions),
        (1, 1),
        "the frame names no task"
    );

    let bundle = bench.store.bundle("t-1").expect("the bundle");
    assert_eq!(bundle.evidence.len(), 1);
    assert_eq!(bundle.evidence[0].id, steps);
    assert_eq!(
        bundle.evidence[0].steps,
        Some(SessionSteps {
            of_task: 2,
            total: 4,
            failed: 0,
        })
    );
    assert_eq!(
        bench
            .store
            .bundle("t-2")
            .and_then(|held| held.evidence[0].steps)
            .map(|held| held.of_task),
        Some(1)
    );
}

/// Pictures pair by the table of names: the word must stand whole in the name,
/// the rest of the name must be the same, and the newest of each side is taken.
#[test]
fn pictures_pair_by_the_table_of_names() {
    let side = |name: &str| side_of(Path::new(name));
    assert_eq!(
        side("dark-before.png"),
        Some((Side::Before, "dark".to_string()))
    );
    assert_eq!(
        side("After_Dark.PNG"),
        Some((Side::After, "dark".to_string()))
    );
    assert_eq!(side("before.png"), Some((Side::Before, String::new())));
    assert_eq!(
        side("전-목록.png"),
        Some((Side::Before, "목록".to_string()))
    );
    assert_eq!(side("목록-후.png"), Some((Side::After, "목록".to_string())));
    assert_eq!(side("aftermath.png"), None, "the word must stand whole");
    assert_eq!(side("dark.png"), None);

    let picture = |id: &str, name: &str, at: i64| Artifact {
        id: id.into(),
        path: PathBuf::from(name),
        modified_ms: at,
        ..blank()
    };
    let rows = [
        picture("a", "list-after.png", 5),
        picture("b", "list-before.png", 4),
        picture("c", "list-after.png", 9),
        picture("d", "drawer-after.png", 3),
        picture("e", "overview.png", 2),
    ];
    let held: Vec<&Artifact> = rows.iter().collect();
    let pictures = pictures_of(&held);
    assert_eq!(
        pictures,
        [
            Picture {
                name: "list".into(),
                before: Some("b".into()),
                after: Some("c".into()),
                single: None,
            },
            // The older "after" of the same name, a side without its other
            // side, and a picture that names no side: each stands alone.
            Picture {
                single: Some("a".into()),
                ..Picture::default()
            },
            Picture {
                single: Some("d".into()),
                ..Picture::default()
            },
            Picture {
                single: Some("e".into()),
                ..Picture::default()
            },
        ]
    );
}

/// A row with nothing in it, for the tests that only need a name and a time.
fn blank() -> Artifact {
    Artifact {
        id: String::new(),
        kind: ArtifactKind::Screenshot,
        subtype: None,
        title: String::new(),
        path: PathBuf::new(),
        bytes: 0,
        created_ms: 0,
        modified_ms: 0,
        url: None,
        favicon: None,
        description: None,
        version: None,
        source_path: None,
        feedback_count: None,
        feedback_version: None,
        origin: Origin::default(),
        tags: Vec::new(),
        preview: zerocode_core::artifact::Preview::None,
        source: Source::Evidence,
    }
}

/// The exit code of a log's run is the `.rc` of the same name: the file beside
/// it, or the one the same worker handed in for the same task into a folder of
/// its own. Another worker's, another task's and another name's are not it —
/// and the drawer's digest of the log is told the same.
#[test]
fn the_exit_code_of_a_run_is_the_rc_of_the_same_name() {
    let bench = Bench::new(Limits::default());
    let kept = Source::Evidence;
    // Each handed-in file in a folder of its own, as the store keeps them.
    let log = bench.place("kept/1/red.log", FAILING_LOG, 1, kept, of("w-1", "t-1"));
    let row = |id: &str| bench.store.get(id).expect("the row");
    assert_eq!(
        bench.store.told_about(&row(&log)).rc,
        None,
        "no exit code yet"
    );
    bench.place("kept/2/red.rc", "7\n", 1, kept, of("w-2", "t-1"));
    bench.place("kept/3/red.rc", "8\n", 1, kept, of("w-1", "t-9"));
    bench.place("kept/4/green.rc", "9\n", 1, kept, of("w-1", "t-1"));
    assert_eq!(bench.store.told_about(&row(&log)).rc, None);
    bench.place("kept/5/red.rc", "0\n", 1, kept, of("w-1", "t-1"));
    assert_eq!(bench.store.told_about(&row(&log)).rc, Some(0));
    let preview = bench.store.preview(&log).expect("the preview");
    assert!(
        matches!(
            &preview.digest,
            Some(Digest::Tests(tests)) if tests.verdict == Verdict::Intended && tests.rc == Some(0)
        ),
        "the drawer's digest is told the exit code: {:?}",
        preview.digest
    );

    // In a folder that holds both, the file beside the log answers first.
    let beside = bench.place("run/job.log", PASSING_LOG, 1, kept, Origin::default());
    bench.place("run/job.rc", "3\n", 1, kept, Origin::default());
    assert_eq!(bench.store.told_about(&row(&beside)).rc, Some(3));
    // A file that is not a log is told nothing.
    let other = bench.place("run/job.json", "{}", 1, kept, Origin::default());
    assert_eq!(bench.store.told_about(&row(&other)), Told::default());
}

/// A failure the worker said to expect is what the run was for, whatever the
/// run's exit code — and it is the hand-in's word that says so, never the
/// file's name: the log of a red run named `green.log` is intended once its
/// worker said `fail`, and a log named `red.log` nobody said anything of has
/// failed. The newest hand-in that names a file decides, and a verdict drawn
/// before the record came — or before the run's exit code did — is drawn again.
#[test]
fn a_failure_the_hand_in_said_to_expect_is_intended_and_a_name_says_nothing() {
    use zerocode_core::hand_in::{Entry, HandIn, Manifest, Named, Outcome, Role};
    let bench = Bench::new(Limits::default());
    let kept = Source::Evidence;
    let said = bench.place("kept/1/green.log", FAILING_LOG, 1, kept, of("w-1", "t-1"));
    let unsaid = bench.place("kept/2/red.log", FAILING_LOG, 1, kept, of("w-1", "t-1"));
    let verdict = |id: &str| match &bench.store.preview(id).expect("the preview").digest {
        Some(Digest::Tests(tests)) => Some((tests.verdict, tests.rc)),
        _ => None,
    };
    assert_eq!(verdict(&said), Some((Verdict::Fail, None)));
    assert_eq!(verdict(&unsaid), Some((Verdict::Fail, None)));
    let hand_in = |message: &str, at_ms: i64, expect: Expect| {
        let named = HandIn {
            run: "run-1".into(),
            message: message.into(),
            task: Some("t-1".into()),
            dispatch: None,
            worker: "w-1".into(),
            at_ms,
            commit: None,
            named: Named::default(),
        };
        let mut manifest = Manifest::begin(&named, format!("print-{message}"), at_ms);
        manifest.push(Entry {
            name: "green.log".into(),
            role: Role::Evidence,
            outcome: Outcome::Kept,
            artifact: Some(said.clone()),
            expect: Some(expect),
            ..Entry::default()
        });
        bench.store.note_hand_in(manifest).expect("noted");
    };
    hand_in("m-1", DAY_MS, Expect::Fail);
    assert_eq!(
        verdict(&said),
        Some((Verdict::Intended, None)),
        "the worker said the run was meant to fail, and the verdict drawn before is drawn again"
    );
    assert_eq!(
        verdict(&unsaid),
        Some((Verdict::Fail, None)),
        "a name says nothing"
    );
    // The run's exit code is catalogued after the log was read: it joins the verdict.
    bench.place("kept/3/green.rc", "101\n", 2, kept, of("w-1", "t-1"));
    assert_eq!(verdict(&said), Some((Verdict::Intended, Some(101))));
    // A newer hand-in that says the run should pass decides.
    hand_in("m-2", DAY_MS + 1, Expect::Pass);
    assert_eq!(verdict(&said), Some((Verdict::Fail, Some(101))));
    // The task's bundle says the same of the same file.
    let bundle = bench.store.bundle("t-1").expect("the bundle");
    let line = bundle
        .evidence
        .iter()
        .find(|line| line.id == said)
        .expect("its line");
    assert_eq!(
        line.tests.as_ref().map(|tests| (tests.verdict, tests.rc)),
        Some((Verdict::Fail, Some(101)))
    );
}

/// What reading the catalog by task costs (t-36910), on a catalog the size of
/// the measured one: 1,500 rows over 400 tasks. It times the listing of the
/// tasks with the bytes of its answer, the bundle of one task with its logs
/// read, and the summary of a test log of 8 MiB — beside the listing of rows
/// the window asked for before, so the two can be read against each other —
/// and reads the process's memory before and after the nine summaries of the
/// long log: a log is read a line at a time, so its size is not held.
///
/// Run on purpose, normally and under `taskpolicy -b`:
/// `cargo test -p zerocode-shell --bin zerocode-shell artifact_runtime::tasks::tests::measure_ -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
fn measure_the_listing_by_task_a_bundle_and_the_summary_of_a_long_log() {
    use std::time::Instant;
    const ROWS: usize = 1_500;
    const TASKS: usize = 400;
    const ROUNDS: usize = 9;
    const LOG_BYTES: usize = 8 * 1024 * 1024;
    // Of every ten rows: four reports, three logs, two pictures, one page.
    const KINDS: [(&str, Source); 10] = [
        ("report.md", Source::WorkerReport),
        ("run.log", Source::Evidence),
        ("shot.png", Source::Evidence),
        ("report.md", Source::WorkerReport),
        ("run.log", Source::Evidence),
        ("page.html", Source::AgentPage),
        ("report.md", Source::WorkerReport),
        ("run.log", Source::Evidence),
        ("shot.png", Source::Evidence),
        ("report.md", Source::WorkerReport),
    ];
    let median = |mut times: Vec<f64>| {
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let millis = |began: Instant| began.elapsed().as_secs_f64() * 1_000.0;
    let timed = |run: &dyn Fn() -> usize| {
        let mut times = Vec::new();
        let mut said = 0;
        for _ in 0..ROUNDS {
            let began = Instant::now();
            said = run();
            times.push(millis(began));
        }
        (median(times), said)
    };

    let bench = Bench::new(Limits::default());
    for at in 0..ROWS {
        let (name, source) = KINDS[at % KINDS.len()];
        let text = match name {
            "report.md" => format!("# Synthetic report {at}\n\nThe queue drains on close.\n"),
            "run.log" => PASSING_LOG.to_string(),
            _ => "<title>Synthetic</title>".to_string(),
        };
        bench.place(
            &format!("rows/{at}/{name}"),
            &text,
            (at % 600) as i64,
            source,
            of(&format!("w-{}", at % 7), &format!("t-{}", at % TASKS)),
        );
    }

    let everything = Filter::default();
    let (tasks_ms, tasks_bytes) =
        timed(&|| serde_json::to_vec(&bench.store.tasks(&everything)).map_or(0, |held| held.len()));
    let listed = bench.store.tasks(&everything);
    let present = Filter {
        present: Some(true),
        ..Filter::default()
    };
    let (rows_ms, rows_bytes) =
        timed(&|| serde_json::to_vec(&bench.store.list(&present)).map_or(0, |held| held.len()));
    // The first reading of a task opens its files; the ones after it find what
    // those said already read.
    let began = Instant::now();
    let _ = bench.store.bundle("t-1");
    let bundle_first_ms = millis(began);
    let (bundle_ms, bundle_bytes) =
        timed(&|| serde_json::to_vec(&bench.store.bundle("t-1")).map_or(0, |held| held.len()));
    let bundle = bench.store.bundle("t-1").expect("the bundle");

    // A long test log: a thousand binaries' worth of lines, to the size named.
    let binary = format!(
        "{}test result: ok. 60 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n",
        "test store::tests::a_row_is_written_and_read_back ... ok\n".repeat(60)
    );
    let long: String = binary.repeat(LOG_BYTES / binary.len() + 1);
    let log = bench.files.join("long/run.log");
    std::fs::create_dir_all(log.parent().expect("a parent")).expect("mkdir");
    std::fs::write(&log, &long[..LOG_BYTES]).expect("write");
    let limits = Limits::default();
    let rss_before = super::super::tests::rss_kib();
    let (digest_ms, passed) = timed(&|| {
        let file = std::fs::File::open(&log).expect("the log");
        match evidence_digest::read(Format::Text, BufReader::new(file), &limits) {
            Some(Digest::Tests(tests)) => tests.passed,
            _ => 0,
        }
    });
    let rss_after = super::super::tests::rss_kib();

    println!(
        "MEASURE t-36910 stage2 {}",
        serde_json::json!({
            "rows": ROWS,
            "tasks": listed.total,
            "tasks_listing_ms": tasks_ms,
            "tasks_listing_bytes": tasks_bytes,
            "rows_listing_ms": rows_ms,
            "rows_listing_bytes": rows_bytes,
            "bundle_first_ms": bundle_first_ms,
            "bundle_ms": bundle_ms,
            "bundle_bytes": bundle_bytes,
            "bundle_rows": bundle.rows.len(),
            "bundle_runs": bundle.tally.runs,
            "log_bytes": LOG_BYTES,
            "log_digest_ms": digest_ms,
            "log_passed": passed,
            "rss_before_log_digests_kib": rss_before,
            "rss_after_log_digests_kib": rss_after,
        })
    );
    assert_eq!(listed.total, TASKS);
    assert!(passed > 0, "the long log is read as a test log");
}
