//! The schedule's reading of the build line and the project's builds
//! (t-67482), over this morning's queue and the release lane's runs
//! v1.1.76–v1.1.83 as their build records, cleaned of names
//! (`ui/tests/fixtures/board-schedule`). The fixture's moment is 2026-10-21
//! 09:50 KST: the l61 gate waits behind a `PAUSE`, two jobs that started
//! before it still run, and v1.1.83's run is in its root gate.

use std::time::{Duration, Instant, UNIX_EPOCH};

use zerocode_core::builds::Shelf;

use super::*;

/// The fixture's clocks are KST.
const KST: i64 = 9 * 3_600;
/// 2026-10-21 09:50:00 KST.
const NOW_MS: i64 = 1_792_543_800_000;
const HOUR_MS: i64 = 3_600_000;

/// A moment of October 2026 on a KST clock, in epoch milliseconds.
fn kst(day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    (1_790_812_800 + (day - 1) * 86_400 + hour * 3_600 + minute * 60 + second - KST) * 1_000
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/tests/fixtures/board-schedule")
}

/// The fixture copied where its files carry the times they had — git keeps no
/// modification time, and a log's last line is dated by its file's.
fn copied() -> tempfile::TempDir {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    copy_tree(&fixture(), scratch.path());
    let stamps: HashMap<String, i64> = serde_json::from_str(
        &std::fs::read_to_string(fixture().join("stamps.json")).expect("the stamps"),
    )
    .expect("the stamps are JSON");
    for (relative, at_ms) in stamps {
        stamp_at(&scratch.path().join(&relative), at_ms);
    }
    scratch
}

/// Give a file the moment it was last written.
fn stamp_at(path: &Path, at_ms: i64) {
    File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(UNIX_EPOCH + Duration::from_millis(at_ms as u64)))
        .unwrap_or_else(|why| panic!("{}: {why}", path.display()));
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a folder");
    for entry in std::fs::read_dir(from).expect("a fixture folder") {
        let entry = entry.expect("an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("a copy");
        }
    }
}

fn roots_in(scratch: &Path) -> Roots {
    Roots {
        line: Some(scratch.join("build-queue")),
        builds: Some(scratch.join("git")),
        machine: None,
    }
}

/// One look as the command takes it: every run the roots' repository keeps,
/// off a shelf and in no checkout's order, then the book's reading of the rest.
fn look(book: &mut Book, roots: &Roots, ask: &Ask, now_ms: i64) -> Timeline {
    look_off(&mut Shelf::default(), book, roots, ask, now_ms)
}

/// The same look off `shelf`, which the command holds from look to look (the
/// window's one shelf, `BUILD_SHELF`): a record it parsed is parsed again
/// only when its file moved.
fn look_off(shelf: &mut Shelf, book: &mut Book, roots: &Roots, ask: &Ask, now_ms: i64) -> Timeline {
    let builds = roots
        .builds
        .as_deref()
        .map_or_else(History::default, |shared| {
            shelf.history(shared, shared, now_ms)
        });
    book.read(roots, ask, builds, now_ms, KST)
}

/// The fixture as the board reads it, from five days back.
fn fixture_reading(scratch: &Path) -> Timeline {
    let ask = Ask {
        known: None,
        from_ms: Some(NOW_MS - 5 * 24 * HOUR_MS),
        log: None,
    };
    look(&mut Book::default(), &roots_in(scratch), &ask, NOW_MS)
}

fn job<'a>(timeline: &'a Timeline, name: &str) -> &'a Job {
    timeline
        .jobs
        .iter()
        .find(|job| job.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the answer"))
}

#[test]
fn the_runners_lines_are_read_as_the_runner_writes_them() {
    assert_eq!(
        said_of(
            "RUN 0-1792535939-run11955-l60r-g1-build who=run11955-l60r lane=1 cap=1800s 07:39:00"
        ),
        Said::Run {
            name: "0-1792535939-run11955-l60r-g1-build".into(),
            who: "run11955-l60r".into(),
            lane: Some(1),
            cap_secs: Some(1800),
            clock: 7 * 3600 + 39 * 60,
        }
    );
    assert_eq!(
        said_of(
            "DONE 0-1792535941-run11955-l60q-g5-window-실측 rc=137 1200s lane=1 08:03:00 load=9"
        ),
        Said::Done {
            name: "0-1792535941-run11955-l60q-g5-window-실측".into(),
            ending: Ending::Code(137),
            secs: 1200,
            lane: Some(1),
            clock: 8 * 3600 + 3 * 60,
        }
    );
    assert_eq!(
        said_of(
            "DONE 0-1792486118-run11955-l58q-g5-window-실측 rc=capped(1800s) 2929s lane=1 05:09:10 load=7"
        ),
        Said::Done {
            name: "0-1792486118-run11955-l58q-g5-window-실측".into(),
            ending: Ending::Capped,
            secs: 2929,
            lane: Some(1),
            clock: 5 * 3600 + 9 * 60 + 10,
        }
    );
    assert_eq!(
        said_of(
            "REFUSED 1792543620-w67501-t67482-probe: who or cwd missing (who='w' cwd='/work/x') 09:47:01"
        ),
        Said::Refused {
            name: "1792543620-w67501-t67482-probe".into(),
            clock: 9 * 3600 + 47 * 60 + 1,
        }
    );
    assert_eq!(
        said_of("PAUSE set 13:46:54 by coord4275: the release lane runs. Running jobs finish."),
        Said::Pause {
            set: true,
            note: "the release lane runs. Running jobs finish.".into(),
            clock: 13 * 3600 + 46 * 60 + 54,
        }
    );
    assert_eq!(
        said_of("DISK LOW 11G < 12G — lane 1 waits 01:30:05"),
        Said::Disk {
            low: true,
            clock: 3600 + 30 * 60 + 5,
        }
    );
    assert_eq!(
        said_of("DISK OK 15G lane 2 05:25:54"),
        Said::Disk {
            low: false,
            clock: 5 * 3600 + 25 * 60 + 54,
        }
    );
    for other in [
        "orchestration: unroutable address: none",
        "{\"messageId\":\"m-67390\"}",
        "RUNNER START lane 1 05:10:56 pid 83884 gen g10210510 (owners take turns)",
        "HANDOFF lane 2 gen g10111052 started after pid 6685 ended 10:52:41",
        "tmux: timed out waiting for the window",
        "BESIDE x starts beside lane 1's suite run y 10:00:00",
        "RUN half-a-line",
        "DONE x rc=0 12s lane=1",
        "",
    ] {
        assert_eq!(said_of(other), Said::Other, "{other:?}");
    }
    assert_eq!(clock_secs("24:00:00"), None);
    assert_eq!(clock_secs("7:39:00"), None);
    assert_eq!(clock_secs("07:39:00:01"), None);
}

#[test]
fn a_job_name_says_its_class_its_queue_time_its_kind_and_its_landing() {
    let gate = name_parts("0-1792535939-run11955-l60r-g1-build");
    assert_eq!(
        gate,
        NameParts {
            class: FIRST_CLASS,
            queued_ms: Some(1_792_535_939_000),
            kind: Some("g1-build".into()),
            gate: Some("l60".into()),
            part: Some("r".into()),
        }
    );
    // A re-run keeps its kind, so its past is the kind's past.
    assert_eq!(
        name_parts("0-1792537431-run11955-l60q-g5-window-실측-rerun")
            .kind
            .as_deref(),
        Some("g5-window-실측")
    );
    assert_eq!(
        name_parts("0-1792535946-run11955-l60r-g2w-win-check")
            .kind
            .as_deref(),
        Some("g2w-win-check")
    );
    // A worker's job has no kind: its estimate is its cap.
    let worker = name_parts("1792533300-w67074-t67064-e-core");
    assert_eq!(
        (worker.class, worker.queued_ms, worker.kind, worker.gate),
        (PLAIN_CLASS, Some(1_792_533_300_000), None, None)
    );
    assert_eq!(
        name_parts("9-1792543760-w66784-t66539-bench").class,
        LAST_CLASS
    );
    // `green` is a word, not a step; a name with no epoch has no queue time.
    assert_eq!(name_parts("run11955-t-1-green-zo-lint").kind, None);
    assert_eq!(name_parts("tools").queued_ms, None);
    assert_eq!(landing("l60"), Some(("l60".into(), String::new())));
    assert_eq!(landing("lane"), None);
    assert!(is_step("g3b2") && is_step("g5b") && !is_step("green") && !is_step("g"));
}

/// A log's lines carry a clock and no date. Each is dated back from the file's
/// last write through the lines below it: across midnight, past a line the other
/// lane wrote a second out of order — and whatever a job's name says. This
/// morning a worker named its job `1792532000-…` (06:33:20) and the runner
/// started it at 06:26:45; dating by the name put it, and every line after it,
/// a day ahead.
#[test]
fn a_clock_is_dated_back_from_the_logs_last_write_across_midnight() {
    assert_eq!(
        clock_at_or_before(kst(19, 0, 42, 43), 22 * 3600 + 48 * 60 + 53, KST),
        kst(18, 22, 48, 53),
        "a clock before midnight, read back from after it, is the day before"
    );
    let clock = |hour: u32, minute: u32, second: u32| hour * 3600 + minute * 60 + second;
    assert_eq!(
        dated_back(
            &[
                clock(23, 58, 0),
                clock(23, 59, 30),
                clock(0, 0, 10),
                clock(0, 2, 0)
            ],
            kst(22, 0, 2, 1),
            LINE_SLACK_MS,
            KST
        ),
        [
            kst(21, 23, 58, 0),
            kst(21, 23, 59, 30),
            kst(22, 0, 0, 10),
            kst(22, 0, 2, 0)
        ]
    );
    // The other lane's line may carry a clock a second ahead of the line below it.
    assert_eq!(
        dated_back(
            &[clock(7, 39, 0), clock(7, 38, 59), clock(7, 40, 0)],
            kst(21, 7, 40, 0),
            LINE_SLACK_MS,
            KST
        ),
        [kst(21, 7, 39, 0), kst(21, 7, 38, 59), kst(21, 7, 40, 0)]
    );
    // One writer (a lane log): a line ahead of the one below it is the day before.
    assert_eq!(
        dated_back(
            &[clock(7, 39, 0), clock(7, 38, 59)],
            kst(21, 7, 39, 0),
            0,
            KST
        ),
        [kst(20, 7, 39, 0), kst(21, 7, 38, 59)]
    );
    // A job named after its own start is dated by the log, not by its name.
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let path = scratch.path().join(RUNNER_LOG);
    std::fs::write(
        &path,
        "RUN 1792532000-w67074-t67064-a-red who=w67074-t67064 lane=1 cap=1800s 06:26:45\n\
         DONE 1792532000-w67074-t67064-a-red rc=0 250s lane=1 06:30:55 load=11\n",
    )
    .expect("a log");
    stamp_at(&path, kst(21, 6, 30, 55));
    let mut tail = RunnerTail::default();
    tail.follow(&path, stamp(&path), KST);
    let ended = tail.finished.back().expect("the job ended");
    assert_eq!(
        (ended.queued_ms, ended.start_ms, ended.end_ms),
        (
            Some(kst(21, 6, 33, 20)),
            Some(kst(21, 6, 26, 45)),
            Some(kst(21, 6, 30, 55))
        ),
        "it started before the moment its name gives, and its line dates it"
    );
}

#[test]
fn the_runner_log_is_followed_from_where_the_last_look_left_it() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let path = scratch.path().join(RUNNER_LOG);
    let first = "RUN 1792535939-w1-t1-g1-build who=w1-t1 lane=1 cap=900s 07:39:00\n";
    std::fs::write(
        &path,
        format!("{first}DONE 1792535939-w1-t1-g1-build rc=0 21"),
    )
    .expect("a log");
    stamp_at(&path, kst(21, 7, 39, 1));
    let mut tail = RunnerTail::default();
    tail.follow(&path, stamp(&path), KST);
    assert!(
        tail.open.contains_key("1792535939-w1-t1-g1-build"),
        "the RUN line"
    );
    assert!(tail.finished.is_empty(), "a line still being written waits");
    let mut log = File::options().append(true).open(&path).expect("the log");
    std::io::Write::write_all(&mut log, b"9s lane=1 07:42:39 load=13\n").expect("the rest");
    stamp_at(&path, kst(21, 7, 42, 40));
    tail.follow(&path, stamp(&path), KST);
    let ended = tail.finished.back().expect("the job ended");
    assert_eq!(
        (ended.state, ended.secs, ended.start_ms, ended.end_ms),
        (
            PASSED,
            Some(219),
            Some(kst(21, 7, 39, 0)),
            Some(kst(21, 7, 42, 39))
        )
    );
    assert_eq!(tail.past.get("g1-build").map(VecDeque::len), Some(1));
    // Replaced by another file (a new runner, a rotation): read again from its first byte.
    std::fs::remove_file(&path).expect("gone");
    std::fs::write(
        &path,
        "RUN 1792536000-w2-t2-red who=w2-t2 lane=2 cap=600s 07:40:00\n",
    )
    .expect("anew");
    stamp_at(&path, kst(21, 7, 40, 1));
    tail.follow(&path, stamp(&path), KST);
    assert!(tail.finished.is_empty(), "the replaced log's jobs are gone");
    assert_eq!(
        tail.open.keys().collect::<Vec<_>>(),
        ["1792536000-w2-t2-red"]
    );
    // Cut shorter in place: read again too.
    std::fs::write(
        &path,
        "RUN 1792536100-w3-t3-red who=w3 lane=1 cap=60s 07:41:40\n",
    )
    .expect("cut");
    stamp_at(&path, kst(21, 7, 41, 41));
    tail.follow(&path, stamp(&path), KST);
    assert_eq!(
        tail.open.keys().collect::<Vec<_>>(),
        ["1792536100-w3-t3-red"]
    );
}

#[test]
fn a_job_is_expected_to_take_the_median_of_its_kind_or_its_cap() {
    let scratch = copied();
    let timeline = fixture_reading(scratch.path());
    let lint = job(&timeline, "0-1792543560-run11955-l61z-g3a-zo-lint");
    assert_eq!(
        (
            lint.state,
            lint.lane,
            lint.start_ms,
            lint.gate.as_deref(),
            lint.part.as_deref()
        ),
        (
            RUNNING,
            Some(2),
            Some(kst(21, 9, 46, 10)),
            Some("l61"),
            Some("z")
        )
    );
    // The zo lint's last five passes in the fixture's log, oldest first: l56 105 s, l58 100 s,
    // l56 again 119 s, l59 123 s, l60 113 s — the four before them dropped.
    assert_eq!(
        lint.expected,
        Some(Expected {
            secs: 113,
            samples: 5
        })
    );
    // No kind, no past: the cap bounds it — the header's for a queued job.
    let red = job(&timeline, "1792543750-w67501-t67482-red");
    assert_eq!(
        (red.state, red.cap_secs, red.expected, red.who.as_str()),
        (
            WAITING,
            600,
            Some(Expected {
                secs: 600,
                samples: 0
            }),
            "w67501-t67482"
        )
    );
    let walk = job(&timeline, "1792543500-run11955-t-63492-walk");
    assert_eq!(
        (walk.state, walk.expected),
        (
            RUNNING,
            Some(Expected {
                secs: 1800,
                samples: 0
            })
        )
    );
    // The root gate's tests: a median under the cap, from the kind's passes.
    let tests = job(&timeline, "0-1792543701-run11955-l61r-g2-tests");
    let tests_expected = tests.expected.expect("an estimate");
    assert!(
        tests_expected.samples >= 3 && tests_expected.secs < 1800,
        "{tests_expected:?}"
    );
    let caps = Caps::LAST_KNOWN;
    assert_eq!(caps.cap_secs(Some("99999")), caps.most_secs);
    assert_eq!(caps.cap_secs(Some("12m")), caps.default_secs);
    assert_eq!(caps.cap_secs(None), caps.default_secs);
}

#[test]
fn the_fixture_reads_into_the_answer_the_board_draws() {
    let scratch = copied();
    let timeline = fixture_reading(scratch.path());
    let line = &timeline.line;
    assert!(
        line.found && line.paused && !line.disk_low && !line.quiet_wanted,
        "{line:?}"
    );
    assert_eq!(line.disk_floor_gib, Some(12));
    assert_eq!(
        line.pause_note.as_deref(),
        Some(
            "the release lane fc4abd39 runs; the queue starts nothing until it ends. Running jobs finish."
        )
    );
    let count = |state: &str| {
        timeline
            .jobs
            .iter()
            .filter(|job| job.state == state)
            .count()
    };
    assert_eq!(
        (
            count(RUNNING),
            count(WAITING),
            count(HELD),
            count(REFUSED),
            count(LOST)
        ),
        (2, 4, 1, 1, 0),
        "two run, four wait, one a guard held, one was refused, and the job being written is not one"
    );
    assert!(
        timeline
            .jobs
            .iter()
            .all(|job| job.name != "1792543790-w67501-t67482-green"),
        "`.<name>.tmp` is a job still being written"
    );
    let held = job(&timeline, "0-1792543702-run11955-l61q-g5-window-실측");
    assert_eq!(
        (held.state, held.class, held.kind.as_deref()),
        (HELD, FIRST_CLASS, Some("g5-window-실측"))
    );
    let last = job(&timeline, "9-1792543760-w66784-t66539-bench");
    assert_eq!((last.class, last.cap_secs), (LAST_CLASS, 900));
    // This morning's l60 gate, start to end, and a run the cap killed.
    let build = job(&timeline, "0-1792535939-run11955-l60r-g1-build");
    assert_eq!(
        (
            build.state,
            build.start_ms,
            build.end_ms,
            build.secs,
            build.rc
        ),
        (
            PASSED,
            Some(kst(21, 7, 39, 0)),
            Some(kst(21, 7, 42, 39)),
            Some(219),
            Some(0)
        )
    );
    let killed = job(&timeline, "0-1792535941-run11955-l60q-g5-window-실측");
    assert_eq!((killed.state, killed.rc), (FAILED, Some(137)));
    let capped = job(&timeline, "0-1792486118-run11955-l58q-g5-window-실측");
    assert_eq!(
        (capped.state, capped.rc, capped.secs),
        (CAPPED, None, Some(2929))
    );
    // The release lane's runs from five days back: v1.1.77's three runs are
    // three rows, and v1.1.83 runs.
    assert_eq!(
        timeline
            .builds
            .iter()
            .filter(|run| run.id == "release" && run.label == "v1.1.77")
            .count(),
        3
    );
    assert_eq!(
        timeline
            .builds
            .iter()
            .filter(|run| run.state == BuildState::Running)
            .map(|run| run.label.as_str())
            .collect::<Vec<_>>(),
        ["v1.1.83"]
    );
    // The answer the window's scene draws (`ui/tests/board-schedule.mjs`), value for value.
    let said = serde_json::to_value(&timeline).expect("the answer serializes");
    let golden = fixture().join("answer.json");
    if std::env::var_os("BOARD_SCHEDULE_WRITE").is_some() {
        let text = serde_json::to_string_pretty(&said).expect("pretty");
        std::fs::write(&golden, format!("{text}\n")).expect("the golden answer");
    }
    let held_answer: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&golden).expect("ui/tests/fixtures/board-schedule/answer.json"),
    )
    .expect("the golden answer is JSON");
    assert_eq!(
        said, held_answer,
        "the answer moved: BOARD_SCHEDULE_WRITE=1 writes it again"
    );
}

/// The end a run is given at its start — each step's usual time, the median of
/// the newest passing runs before it, summed — against its real end, over the
/// release lane's runs v1.1.79–v1.1.82: each lands within fifteen minutes.
/// v1.1.83 runs at the fixture's moment; as the lane left it, it overran its
/// estimate on a load spike nothing at its start could see (the coordinator's
/// call, m-67652), and the view says so then (`ui/tests/board-schedule.mjs`:
/// the delay tag and the load mark).
#[test]
fn a_runs_end_estimated_at_its_start_lands_within_fifteen_minutes() {
    let scratch = copied();
    let timeline = fixture_reading(scratch.path());
    for label in ["v1.1.79", "v1.1.80", "v1.1.81", "v1.1.82"] {
        let run = timeline
            .builds
            .iter()
            .find(|run| run.label == label)
            .expect("the run");
        assert_eq!(run.state, BuildState::Passed, "{label} passed");
        let took = (run.updated_ms - run.started_ms) / 1000;
        let expected: i64 = run
            .steps
            .iter()
            .map(|step| i64::from(step.expected.secs))
            .sum();
        let off = (expected - took).abs();
        assert!(
            off <= 15 * 60,
            "{label}'s end at its start was {off} s off: {expected} s expected, {took} s taken"
        );
    }
}

/// A range holding no ended run still names the last one that ended
/// (t-84799 ⑤): the board's first line says "last build" from it. A range
/// that holds one sends nothing extra, and a running run is never the last.
#[test]
fn the_newest_ended_run_rides_the_answer_when_the_range_holds_none() {
    let scratch = copied();
    let wide = fixture_reading(scratch.path());
    let open = |run: &zerocode_core::builds::Build| run.state == BuildState::Running;
    let newest = wide
        .builds
        .iter()
        .filter(|run| !open(run))
        .max_by_key(|run| run.updated_ms)
        .expect("the fixture has ended runs")
        .clone();
    assert_eq!(
        wide.last_build, None,
        "a range with an ended run sends none"
    );
    let ask = Ask {
        known: None,
        from_ms: Some(NOW_MS - 1),
        log: None,
    };
    let narrow = look(
        &mut Book::default(),
        &roots_in(scratch.path()),
        &ask,
        NOW_MS,
    );
    assert!(narrow.builds.iter().all(open));
    assert_eq!(narrow.last_build, Some(newest));
}

/// An answer carries the newest finished jobs and says how many of the range's
/// it could not carry and from when, so a day is never drawn short in silence.
#[test]
fn an_answer_says_how_many_finished_jobs_it_could_not_carry() {
    let count = SENT_JOBS + 50;
    let clock = |index: usize| (1 + (index / 60) as i64, (index % 60) as i64);
    let at = |index: usize| kst(21, clock(index).0, clock(index).1, 0);
    let text: String = (0..count)
        .map(|index| {
            let (hour, minute) = clock(index);
            format!("DONE 1792500000-w1-t{index}-a rc=0 60s lane=1 {hour:02}:{minute:02}:00\n")
        })
        .collect();
    let now = kst(21, 9, 0, 0);
    let (_scratch, roots) = lone_queue(&text, now);
    let ask_from = |from_ms| Ask {
        known: None,
        from_ms: Some(from_ms),
        log: None,
    };
    let mut book = Book::default();

    let whole = look(&mut book, &roots, &ask_from(at(0)), now);
    assert_eq!(whole.jobs.len(), SENT_JOBS, "the newest, up to the cap");
    assert_eq!(
        whole.jobs_left_out,
        Some(JobsLeftOut {
            count: 50,
            before_ms: at(50),
        }),
        "the oldest fifty, older than the oldest job carried"
    );
    assert_eq!(whole.jobs[0].end_ms, Some(at(50)));
    let said = serde_json::to_value(&whole).expect("the answer serializes");
    assert_eq!(said["jobs_left_out"]["count"], 50);
    assert_eq!(said["jobs_left_out"]["before_ms"], at(50));

    let exactly = look(&mut book, &roots, &ask_from(at(50)), now);
    assert_eq!(exactly.jobs.len(), SENT_JOBS);
    assert_eq!(exactly.jobs_left_out, None, "a range the cap holds whole");

    let narrow = look(&mut book, &roots, &ask_from(at(60)), now);
    assert_eq!(narrow.jobs_left_out, None, "a range under the cap");
    let quiet = serde_json::to_value(&narrow).expect("the answer serializes");
    assert!(
        quiet.get("jobs_left_out").is_none(),
        "no word when none is left out"
    );
}

/// The window places a waiting job's start on the runner's lanes, so the
/// answer carries the lanes the runner's log named — `lane=` on a `RUN` or a
/// `DONE` line, in order, none before it names one — beside the flag that
/// holds them for a measurement (the review t-69069, P2-2: the window had the
/// runner's two lanes written into it).
#[test]
fn the_answer_carries_the_lanes_the_runner_named_and_its_quiet_wait() {
    let lanes_of = |timeline: &Timeline| {
        serde_json::to_value(timeline).expect("the answer serializes")["line"]["runner_lanes"]
            .clone()
    };
    let scratch = copied();
    assert_eq!(
        lanes_of(&fixture_reading(scratch.path())),
        serde_json::json!([1, 2]),
        "this morning's runner ran two lanes"
    );
    // A runner with a third lane, and a measurement that waits for a quiet machine.
    let queue = scratch.path().join("build-queue");
    let log = queue.join(RUNNER_LOG);
    let mut text = std::fs::read_to_string(&log).expect("the runner log");
    text.push_str(concat!(
        "RUN 1792543680-w67501-t67482-probe2 who=w67501-t67482 lane=3 cap=600s 09:48:10\n",
        "DONE 1792543680-w67501-t67482-probe2 rc=0 20s lane=3 09:48:30 load=5.1\n",
    ));
    std::fs::write(&log, text).expect("the runner log grows");
    stamp_at(&log, NOW_MS - 30_000);
    std::fs::write(queue.join(QUIET_FLAG), "").expect("the quiet flag");
    let three = fixture_reading(scratch.path());
    assert_eq!(lanes_of(&three), serde_json::json!([1, 2, 3]));
    assert_eq!(
        serde_json::to_value(&three).expect("the answer serializes")["line"]["quiet_wanted"],
        serde_json::json!(true)
    );
    // A new runner's log that names no lane yet: the window is told none.
    std::fs::write(&log, "PAUSE lifted 09:49:00 by coord4275: the lane ended\n")
        .expect("a new runner log");
    stamp_at(&log, NOW_MS - 30_000);
    assert_eq!(
        lanes_of(&fixture_reading(scratch.path())),
        serde_json::json!([])
    );
}

#[test]
fn a_look_where_nothing_moved_answers_unchanged_and_one_that_moved_does_not() {
    let scratch = copied();
    let roots = roots_in(scratch.path());
    let mut book = Book::default();
    let first = look(&mut book, &roots, &Ask::default(), NOW_MS);
    assert!(!first.unchanged && !first.jobs.is_empty());
    let known = Ask {
        known: Some(first.revision),
        ..Ask::default()
    };
    let again = look(&mut book, &roots, &known, NOW_MS + 5_000);
    assert!(
        again.unchanged && again.jobs.is_empty() && again.builds.is_empty(),
        "{again:?}"
    );
    assert_eq!(again.revision, first.revision);
    // The runner wrote a line: the next look is a new revision with the job ended.
    let log = scratch.path().join("build-queue").join(RUNNER_LOG);
    let mut file = File::options()
        .append(true)
        .open(&log)
        .expect("the runner log");
    std::io::Write::write_all(
        &mut file,
        b"DONE 0-1792543560-run11955-l61z-g3a-zo-lint rc=0 118s lane=2 09:48:08 load=4\n",
    )
    .expect("a line");
    std::fs::remove_file(
        scratch
            .path()
            .join("build-queue/run/0-1792543560-run11955-l61z-g3a-zo-lint.job"),
    )
    .expect("the runner moved it");
    let moved = look(&mut book, &roots, &known, NOW_MS + 10_000);
    assert!(!moved.unchanged && moved.revision > first.revision);
    assert_eq!(
        job(&moved, "0-1792543560-run11955-l61z-g3a-zo-lint").state,
        PASSED
    );
}

/// What git says of the board's tasks rides the answer (t-71840), and a
/// landing that moved moves the revision as a file does: the window holding
/// the last revision is told the new one, not `unchanged`; the same landings
/// again move nothing.
/// A line of jobs written here: their files in `in/` and `done/`, the
/// runner's log of the ended ones, dated on the fixture's morning.
fn line_of(scratch: &Path, ended: &[(&str, &str, i32, u32)], waiting: &[(&str, &str)]) -> Roots {
    let queue = scratch.join("build-queue");
    for folder in [QUEUE_IN, QUEUE_RUN, QUEUE_DONE] {
        std::fs::create_dir_all(queue.join(folder)).expect("a queue folder");
    }
    let mut log = String::new();
    for (minute, (name, header, rc, secs)) in (0..).zip(ended) {
        std::fs::write(
            queue.join(QUEUE_DONE).join(format!("{name}{JOB_SUFFIX}")),
            header,
        )
        .expect("an ended job");
        log.push_str(&format!(
            "RUN {name} who=w lane=1 cap=1800s 09:{minute:02}:00\nDONE {name} rc={rc} {secs}s lane=1 09:{minute:02}:30 load=3\n"
        ));
    }
    for (name, header) in waiting {
        std::fs::write(
            queue.join(QUEUE_IN).join(format!("{name}{JOB_SUFFIX}")),
            header,
        )
        .expect("a waiting job");
    }
    let runner = queue.join(RUNNER_LOG);
    std::fs::write(&runner, log).expect("the runner's log");
    stamp_at(&runner, kst(21, 9, 30, 0));
    Roots {
        line: Some(queue),
        builds: None,
        machine: None,
    }
}

/// A job's `# task:` line ties its run to every ledger task it names, whole
/// and never shared out; `# what:` is the person's words and ties nothing,
/// and a piece that is no task id is dropped (t-73910).
#[test]
fn a_jobs_task_line_ties_its_run_to_each_task_it_names_and_shares_it() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let roots = line_of(
        scratch.path(),
        &[
            (
                "0-1792540000-coord-l78x-g2-tests",
                "# who: coord-l78x\n# what: l78x 묶음 t-3\n# task: t-2, t-1,gate-l78x, t-, t-12a\n",
                1,
                600,
            ),
            (
                "1792540100-w1-t1-red",
                "# who: w1-t1\n# task: t-1\n",
                0,
                100,
            ),
        ],
        &[("1792540200-w4-t4-red", "# who: w4-t4\n# task: t-4\n")],
    );
    let ask = Ask {
        known: None,
        from_ms: Some(NOW_MS - 5 * 24 * HOUR_MS),
        log: None,
    };
    let read = look(&mut Book::default(), &roots, &ask, NOW_MS);
    assert_eq!(
        job(&read, "0-1792540000-coord-l78x-g2-tests").tasks,
        ["t-1", "t-2"]
    );
    assert_eq!(job(&read, "1792540200-w4-t4-red").tasks, ["t-4"]);
    assert_eq!(
        read.task_lines.get("t-1"),
        Some(&TaskLine {
            jobs: 2,
            red: 1,
            secs: 700,
            shared: 2
        })
    );
    assert_eq!(
        read.task_lines.get("t-2"),
        Some(&TaskLine {
            jobs: 1,
            red: 1,
            secs: 600,
            shared: 2
        })
    );
    assert!(
        !read.task_lines.contains_key("t-3"),
        "`# what:` was read for a task"
    );
    assert!(
        !read.task_lines.contains_key("t-4"),
        "a waiting job has spent no line time"
    );
}

/// A done file that comes after its runner line, or whose `# task:` line is
/// rewritten, moves the answer: a view that knows the last revision is not
/// told "unchanged" over a tie it never saw (the review R2, P2-4). A tie the
/// look read is no change the next time.
#[test]
fn a_done_file_that_arrives_late_or_is_rewritten_moves_the_revision() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let (first, late) = ("1792540100-w1-t1-red", "1792540200-w2-t2-red");
    let roots = line_of(
        scratch.path(),
        &[
            (first, "# who: w1-t1\n# task: t-1\n", 0, 100),
            (late, "# who: w2-t2\n# task: t-2\n", 0, 50),
        ],
        &[],
    );
    let done = roots.line.as_ref().expect("the line").join(QUEUE_DONE);
    let file = |name: &str| done.join(format!("{name}{JOB_SUFFIX}"));
    std::fs::remove_file(file(late)).expect("the late job's file is not there yet");
    let ask = |known: Option<u64>| Ask {
        known,
        from_ms: Some(NOW_MS - 5 * 24 * HOUR_MS),
        log: None,
    };
    let mut book = Book::default();
    let read = look(&mut book, &roots, &ask(None), NOW_MS);
    assert_eq!(job(&read, first).tasks, ["t-1"]);
    assert!(job(&read, late).tasks.is_empty());
    assert!(
        look(&mut book, &roots, &ask(Some(read.revision)), NOW_MS).unchanged,
        "a look where nothing moved"
    );

    std::fs::write(file(first), "# who: w1-t1\n# task: t-5\n").expect("rewritten");
    stamp_at(&file(first), kst(21, 9, 40, 0));
    std::fs::write(file(late), "# who: w2-t2\n# task: t-2\n").expect("arrived");
    let moved = look(&mut book, &roots, &ask(Some(read.revision)), NOW_MS);
    assert!(
        !moved.unchanged,
        "the ties moved and the view was told nothing"
    );
    assert!(moved.revision > read.revision);
    assert_eq!(job(&moved, first).tasks, ["t-5"]);
    assert_eq!(job(&moved, late).tasks, ["t-2"]);
    assert!(
        look(&mut book, &roots, &ask(Some(moved.revision)), NOW_MS).unchanged,
        "a tie the look read is no change the next time"
    );
}

/// A done file that arrives between the look tying the ended jobs and the
/// look signing what it tied is a change the next look must report: the
/// signature holds the stamps the tie was made from, not the disk's a moment
/// later (the review R5, P2-4).
#[test]
fn a_done_file_arriving_in_the_gap_after_the_tie_is_a_change_the_next_look_reports() {
    const GAP: &str = "1792540200-w2-t2-red";
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let roots = line_of(
        scratch.path(),
        &[(GAP, "# who: w2-t2\n# task: t-2\n", 0, 50)],
        &[],
    );
    let file = roots
        .line
        .as_ref()
        .expect("the line")
        .join(QUEUE_DONE)
        .join(format!("{GAP}{JOB_SUFFIX}"));
    std::fs::remove_file(&file).expect("the file is not there yet");
    let ask = |known: Option<u64>| Ask {
        known,
        from_ms: Some(NOW_MS - 5 * 24 * HOUR_MS),
        log: None,
    };
    let mut book = Book::default();
    // Reads once so the roots are the book's own and the hook survives.
    look(&mut book, &roots, &ask(None), NOW_MS);
    book.after_tie = Some(|roots| {
        let done = roots.line.as_ref().expect("the line").join(QUEUE_DONE);
        std::fs::write(
            done.join(format!("{GAP}{JOB_SUFFIX}")),
            "# who: w2-t2\n# task: t-2\n",
        )
        .expect("arrives in the gap");
    });
    let tied = look(&mut book, &roots, &ask(None), NOW_MS);
    assert!(
        job(&tied, GAP).tasks.is_empty(),
        "the tie was made before the file came"
    );
    book.after_tie = None;
    let next = look(&mut book, &roots, &ask(Some(tied.revision)), NOW_MS);
    assert!(
        !next.unchanged,
        "the file that came in the gap is a change the view was never told of"
    );
    assert_eq!(job(&next, GAP).tasks, ["t-2"]);
    assert!(
        look(&mut book, &roots, &ask(Some(next.revision)), NOW_MS).unchanged,
        "and once told, it is no change again"
    );
}

/// A job naming no task ties itself to none, and the answer says nothing of
/// tasks rather than a zero.
#[test]
fn a_job_naming_no_task_is_tied_to_none() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let roots = line_of(
        scratch.path(),
        &[(
            "1792540300-w9-t9-red",
            "# who: w9-t9\n# what: t-9 고침\n",
            0,
            40,
        )],
        &[],
    );
    let ask = Ask {
        known: None,
        from_ms: Some(NOW_MS - 5 * 24 * HOUR_MS),
        log: None,
    };
    let read = look(&mut Book::default(), &roots, &ask, NOW_MS);
    assert!(job(&read, "1792540300-w9-t9-red").tasks.is_empty());
    assert!(read.task_lines.is_empty());
    let said = serde_json::to_value(&read).expect("json");
    assert!(said.get("task_lines").is_none(), "{said}");
    assert!(said["jobs"][0].get("tasks").is_none(), "{said}");
}

#[test]
fn a_landing_rides_the_answer_and_moves_its_revision() {
    let scratch = copied();
    let roots = roots_in(scratch.path());
    let mut book = Book::default();
    let first = look(&mut book, &roots, &Ask::default(), NOW_MS);
    assert!(first.landings.is_empty());
    let landed = Landed {
        landed_ms: Some(NOW_MS - HOUR_MS),
        released_ms: None,
    };
    let told = || BTreeMap::from([("t-66760".to_string(), landed)]);
    book.land(told());
    let known = |revision| Ask {
        known: Some(revision),
        ..Ask::default()
    };
    let moved = look(&mut book, &roots, &known(first.revision), NOW_MS + 5_000);
    assert!(!moved.unchanged && moved.revision > first.revision);
    assert_eq!(moved.landings.get("t-66760"), Some(&landed));
    assert_eq!(
        serde_json::to_value(&moved).expect("the answer serializes")["landings"]["t-66760"],
        serde_json::json!({"landed_ms": NOW_MS - HOUR_MS, "released_ms": null}),
        "the window reads these keys"
    );
    book.land(told());
    let again = look(&mut book, &roots, &known(moved.revision), NOW_MS + 10_000);
    assert!(again.unchanged && again.revision == moved.revision);
}

#[test]
fn a_log_is_read_only_for_a_job_this_reading_listed_or_a_run_its_build_keeps() {
    let scratch = copied();
    let roots = roots_in(scratch.path());
    let lane_log = scratch.path().join("lane-fc4abd39.log");
    std::fs::write(
        &lane_log,
        "09:47:57 == lane fc4abd39 start\n09:50:01 == green\n",
    )
    .expect("a log");
    let logged = NOW_MS - 60_000;
    zerocode_core::builds::begin(
        &scratch.path().join("git"),
        "release",
        &zerocode_core::builds::Run {
            started_ms: logged,
            log: lane_log.display().to_string(),
            ..zerocode_core::builds::Run::default()
        },
    )
    .expect("a run that names its log");
    let mut book = Book::default();
    let mut read = |log: LogAsk| {
        look(
            &mut book,
            &roots,
            &Ask {
                log: Some(log),
                ..Ask::default()
            },
            NOW_MS,
        )
        .log
        .expect("a log answer")
    };
    let build = read(LogAsk::Job("0-1792535939-run11955-l60r-g1-build".into()));
    assert!(!build.missing && build.cut, "{:?}", build.lines.first());
    assert_eq!(build.lines.len(), LOG_TAIL_LINES);
    assert_eq!(
        build.lines.last().map(String::as_str),
        Some("    Finished `dev` profile [unoptimized] target(s) in 3m 39s")
    );
    let running = read(LogAsk::Job("1792543500-run11955-t-63492-walk".into()));
    assert_eq!(
        (running.missing, running.cut, running.lines.len()),
        (false, false, 4)
    );
    for stranger in ["../runner", "../../git/zo", ".hidden", "not-a-job-here", ""] {
        let refused = read(LogAsk::Job(stranger.into()));
        assert!(refused.missing && refused.lines.is_empty(), "{stranger:?}");
    }
    let run = |id: &str, started_ms: i64| {
        LogAsk::Build(BuildRun {
            id: id.into(),
            started_ms,
        })
    };
    let lane = read(run("release", logged));
    assert!(!lane.missing && lane.lines.iter().any(|line| line.ends_with("== green")));
    assert_eq!(
        lane.build,
        Some(BuildRun {
            id: "release".into(),
            started_ms: logged
        })
    );
    assert!(read(run("release", logged + 1)).missing, "no such run");
    assert!(read(run("../release", logged)).missing, "not a build");
    assert!(
        read(run("release", kst(21, 9, 47, 57))).missing,
        "a kept run that names no log"
    );
}

/// Timed as the window takes it every 5 s: the builds off the one shelf the
/// command holds from look to look, then the book. A shelf made new for each
/// look opened and parsed every kept record each time — 10.2 ms for the
/// fixture's ten on the Windows runner (t-98321) — a cost the window never
/// pays once its shelf holds them.
#[test]
fn a_look_costs_little_when_nothing_moved() {
    let scratch = copied();
    let roots = roots_in(scratch.path());
    let mut shelf = Shelf::default();
    let mut book = Book::default();
    let first = look_off(&mut shelf, &mut book, &roots, &Ask::default(), NOW_MS);
    let known = Ask {
        known: Some(first.revision),
        ..Ask::default()
    };
    let shared = roots.builds.clone().expect("the fixture keeps builds");
    assert!(
        !shelf.history(&shared, &shared, NOW_MS).runs.is_empty(),
        "the shelf holds the fixture's runs"
    );
    let micros = |began: Instant| began.elapsed().as_secs_f64() * 1_000_000.0;
    let (mut took, mut builds): (Vec<f64>, Vec<f64>) = (0..200)
        .map(|_| {
            let began = Instant::now();
            let history = shelf.history(&shared, &shared, NOW_MS);
            let off_shelf = micros(began);
            let quiet = book.read(&roots, &known, history, NOW_MS, KST);
            assert!(quiet.unchanged);
            (micros(began), off_shelf)
        })
        .unzip();
    took.sort_by(f64::total_cmp);
    builds.sort_by(f64::total_cmp);
    let (middle, high) = (took[took.len() / 2], took[took.len() * 95 / 100]);
    let shelf_middle = builds[builds.len() / 2];
    eprintln!(
        "a quiet look: {middle:.0} µs median ({shelf_middle:.0} µs of it the builds \
         off the shelf), {high:.0} µs p95 — every 5 s while the view is open"
    );
    assert!(middle < 5_000.0, "a quiet look took {middle:.0} µs");
}

/* ---- the review t-69069's P3 (t-69106) ------------------------------------ */

/// A queue folder with nothing queued and a runner log of `text`, last
/// written at `at_ms`.
fn lone_queue(text: &str, at_ms: i64) -> (tempfile::TempDir, Roots) {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let queue = scratch.path().join("build-queue");
    for place in [QUEUE_IN, QUEUE_RUN, QUEUE_DONE] {
        std::fs::create_dir_all(queue.join(place)).expect("a queue folder");
    }
    std::fs::write(queue.join(RUNNER_LOG), text).expect("the runner log");
    stamp_at(&queue.join(RUNNER_LOG), at_ms);
    let roots = Roots {
        line: Some(queue),
        builds: None,
        machine: None,
    };
    (scratch, roots)
}

fn append(path: &Path, text: &str, at_ms: i64) {
    let mut file = File::options().append(true).open(path).expect("the file");
    std::io::Write::write_all(&mut file, text.as_bytes()).expect("a line");
    stamp_at(path, at_ms);
}

fn runner_log(roots: &Roots) -> PathBuf {
    roots.line.as_ref().expect("a queue").join(RUNNER_LOG)
}

/// `f` on its own thread, given `secs` to answer: a reader that waits on a
/// pipe never does, and the test says so instead of hanging.
#[cfg(unix)]
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tell, hear) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tell.send(f());
    });
    hear.recv_timeout(Duration::from_secs(secs)).ok()
}

/// A named pipe nobody writes to.
#[cfg(unix)]
fn pipe_at(path: &Path) {
    let name = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()))
        .expect("a name");
    // SAFETY: `name` is a NUL-terminated path that outlives the call.
    assert_eq!(
        unsafe { libc::mkfifo(name.as_ptr(), 0o600) },
        0,
        "a pipe at {}",
        path.display()
    );
}

/// ① A runner log that gained more than a look reads is read by its last
/// stretch, as a first look reads a long one — not whole.
#[test]
fn a_runner_log_that_grew_far_between_looks_is_read_by_its_last_stretch() {
    let (_scratch, roots) = lone_queue(
        "RUN 1792535000-w1-t1-early who=w1 lane=1 cap=900s 07:00:00\n",
        kst(21, 7, 0, 1),
    );
    let mut book = Book::default();
    look(&mut book, &roots, &Ask::default(), kst(21, 7, 0, 2));
    let filler = "orchestration echo that tells nothing\n".repeat(6 * 1024 * 1024 / 38);
    append(
        &runner_log(&roots),
        &format!(
            "RUN 1792535100-w1-t1-gone who=w1 lane=1 cap=900s 07:01:00\n{filler}RUN 1792535400-w1-t1-late who=w1 lane=1 cap=900s 07:05:00\n"
        ),
        kst(21, 7, 6, 0),
    );
    look(&mut book, &roots, &Ask::default(), kst(21, 7, 6, 5));
    let open: Vec<&str> = book.runner.open.keys().map(String::as_str).collect();
    assert!(
        open.contains(&"1792535400-w1-t1-late"),
        "the last stretch is read: {open:?}"
    );
    assert!(
        !open.contains(&"1792535100-w1-t1-gone"),
        "a gain of 6 MiB was read from its first byte: {open:?}"
    );
}

/// ② A rotated, replaced or rewritten log does not take the kinds' past with
/// it: the estimate stays the median of what passed, not the cap.
#[test]
fn a_rotated_runner_log_keeps_the_past_the_estimates_stand_on() {
    let passes: String = (0..3)
        .map(|n| {
            format!(
                "RUN 17925359{n}0-w1-t1-g1-build who=w1 lane=1 cap=900s 07:3{n}:00\nDONE 17925359{n}0-w1-t1-g1-build rc=0 {}s lane=1 07:3{n}:30 load=1\n",
                100 + n * 10
            )
        })
        .collect();
    let (scratch, roots) = lone_queue(&passes, kst(21, 7, 33, 0));
    let mut book = Book::default();
    look(&mut book, &roots, &Ask::default(), kst(21, 7, 34, 0));
    let before = book.runner.expected(Some("g1-build"), 900);
    assert_eq!((before.secs, before.samples), (110, 3));
    // The runner starts a new log (the old one moved away): another file, one job in it.
    let fresh = scratch.path().join("runner.new");
    std::fs::write(
        &fresh,
        "RUN 1792536200-w2-t2-other who=w2 lane=2 cap=600s 07:50:00\n",
    )
    .expect("a new log");
    stamp_at(&fresh, kst(21, 7, 50, 1));
    std::fs::rename(&fresh, runner_log(&roots)).expect("the new log takes the name");
    look(&mut book, &roots, &Ask::default(), kst(21, 7, 51, 0));
    assert!(
        book.runner.finished.is_empty(),
        "the old log's jobs are gone"
    );
    let after = book.runner.expected(Some("g1-build"), 900);
    assert_eq!(
        (after.secs, after.samples),
        (110, 3),
        "a rotated log sent every estimate back to its cap"
    );
}

/// ② The same file cut and grown past where it was read is another file too.
#[test]
fn a_runner_log_cut_and_grown_past_the_offset_is_read_again_from_its_start() {
    let old = "RUN 1792535000-w1-t1-old-one who=w1 lane=1 cap=900s 07:00:00\n\
               RUN 1792535010-w1-t1-old-two who=w1 lane=1 cap=900s 07:00:10\n";
    let (_scratch, roots) = lone_queue(old, kst(21, 7, 0, 20));
    let mut book = Book::default();
    look(&mut book, &roots, &Ask::default(), kst(21, 7, 0, 30));
    assert_eq!(book.runner.open.len(), 2);
    // Written over in place — one inode, longer than before, other bytes where the offset was.
    let rewritten = "RUN 1792536000-w9-t9-new-one who=w9 lane=2 cap=600s 08:00:00\n\
                     RUN 1792536010-w9-t9-new-two who=w9 lane=2 cap=600s 08:00:10\n\
                     RUN 1792536020-w9-t9-new-three who=w9 lane=2 cap=600s 08:00:20\n";
    std::fs::write(runner_log(&roots), rewritten).expect("written over");
    stamp_at(&runner_log(&roots), kst(21, 8, 0, 30));
    look(&mut book, &roots, &Ask::default(), kst(21, 8, 0, 40));
    let mut open: Vec<&str> = book.runner.open.keys().map(String::as_str).collect();
    open.sort_unstable();
    assert_eq!(
        open,
        [
            "1792536000-w9-t9-new-one",
            "1792536010-w9-t9-new-two",
            "1792536020-w9-t9-new-three"
        ],
        "the log was read on from the old offset"
    );
}

/// ③ A `RUN` no `DONE` ever follows is not held for ever: past its cap and a
/// grace it leaves the open map, and the board still draws it, lost.
#[test]
fn a_run_with_no_done_line_expires_into_a_lost_job() {
    let (_scratch, roots) = lone_queue(
        "RUN 1792535000-w1-t1-stale who=w1 lane=1 cap=60s 07:00:00\n",
        kst(21, 7, 0, 1),
    );
    let mut book = Book::default();
    look(&mut book, &roots, &Ask::default(), kst(21, 7, 0, 5));
    assert!(book.runner.open.contains_key("1792535000-w1-t1-stale"));
    // Two hours on the runner writes again; the job is far past its minute.
    append(
        &runner_log(&roots),
        "PAUSE lifted 09:00:00 by coord4275: the lane ended\n",
        kst(21, 9, 0, 1),
    );
    let later = look(&mut book, &roots, &Ask::default(), kst(21, 9, 0, 5));
    assert!(
        !book.runner.open.contains_key("1792535000-w1-t1-stale"),
        "a run past its cap by two hours is still held open"
    );
    assert_eq!(
        job(&later, "1792535000-w1-t1-stale").state,
        LOST,
        "the board still says it was lost"
    );
    // Its end line, if it ever comes, replaces the lost row rather than doubling it.
    append(
        &runner_log(&roots),
        "DONE 1792535000-w1-t1-stale rc=0 59s lane=1 09:01:00 load=1\n",
        kst(21, 9, 1, 1),
    );
    let ended = look(&mut book, &roots, &Ask::default(), kst(21, 9, 1, 5));
    let rows: Vec<&Job> = ended
        .jobs
        .iter()
        .filter(|job| job.name == "1792535000-w1-t1-stale")
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].state, PASSED);
}

#[test]
fn a_quiet_runner_expires_only_after_the_cap_and_grace_and_moves_the_revision() {
    let name = "1792535000-w1-t1-stale";
    let (_scratch, roots) = lone_queue(
        "RUN 1792535000-w1-t1-stale who=w1 lane=1 cap=60s 07:00:00\n",
        kst(21, 7, 0, 1),
    );
    let mut book = Book::default();
    let first = look(&mut book, &roots, &Ask::default(), kst(21, 7, 0, 5));
    let known = Ask {
        known: Some(first.revision),
        ..Ask::default()
    };
    let deadline = kst(21, 7, 0, 0) + 60_000 + LOST_AFTER_CAP_MS;
    let at_cap = look(&mut book, &roots, &known, deadline);
    assert!(
        at_cap.unchanged,
        "a run at its grace boundary expired early"
    );
    let expired = look(&mut book, &roots, &known, deadline + 1);
    assert!(
        !expired.unchanged,
        "the clock moved past the grace but the answer did not"
    );
    assert!(expired.revision > first.revision);
    assert_eq!(job(&expired, name).state, LOST);
    assert!(!book.runner.open.contains_key(name));
    let later = look(
        &mut book,
        &roots,
        &Ask {
            known: Some(expired.revision),
            ..Ask::default()
        },
        deadline + HOUR_MS,
    );
    assert!(
        later.unchanged,
        "an already expired run kept moving the revision"
    );
    assert_eq!(
        book.runner
            .finished
            .iter()
            .filter(|job| job.name == name)
            .count(),
        1
    );
}

/// A run still in `run/` is the runner's own to end: it is never expired.
#[test]
fn a_run_still_in_the_run_folder_is_not_expired() {
    let (_scratch, roots) = lone_queue(
        "RUN 1792535000-w1-t1-slow who=w1 lane=1 cap=60s 07:00:00\n",
        kst(21, 7, 0, 1),
    );
    let queue = roots.line.clone().expect("a queue");
    std::fs::write(
        queue.join(QUEUE_RUN).join("1792535000-w1-t1-slow.job"),
        "# who: w1\n",
    )
    .expect("a running job");
    let mut book = Book::default();
    let first = look(&mut book, &roots, &Ask::default(), kst(21, 7, 0, 5));
    let quiet = look(
        &mut book,
        &roots,
        &Ask {
            known: Some(first.revision),
            ..Ask::default()
        },
        kst(21, 9, 0, 5),
    );
    assert!(quiet.unchanged, "a job still owned by the runner expired");
    let later = look(&mut book, &roots, &Ask::default(), kst(21, 9, 0, 5));
    assert!(book.runner.open.contains_key("1792535000-w1-t1-slow"));
    assert_eq!(job(&later, "1792535000-w1-t1-slow").state, RUNNING);
}

/// ④ The runner's log is another process's file: a link in its place is not
/// followed.
#[cfg(unix)]
#[test]
fn a_runner_log_that_is_a_link_is_not_followed() {
    let (scratch, roots) = lone_queue("", kst(21, 7, 0, 1));
    let real = scratch.path().join("elsewhere.log");
    std::fs::write(
        &real,
        "RUN 1792535000-w1-t1-linked who=w1 lane=1 cap=900s 07:00:00\n",
    )
    .expect("a real log");
    stamp_at(&real, kst(21, 7, 0, 1));
    std::fs::remove_file(runner_log(&roots)).expect("the log goes");
    std::os::unix::fs::symlink(&real, runner_log(&roots)).expect("a link in its place");
    let mut book = Book::default();
    look(&mut book, &roots, &Ask::default(), kst(21, 7, 0, 5));
    assert!(
        book.runner.open.is_empty(),
        "the log was read through a link: {:?}",
        book.runner.open
    );
}

/// ④ The lane's `status.json` and the runner's `DISK_FLOOR` may not be a pipe
/// the reader stands waiting on — with the book's lock held — nor a file of
/// any size.
#[cfg(unix)]
#[test]
fn a_pipe_in_place_of_the_disk_floor_does_not_hold_the_look() {
    let (_scratch, roots) = lone_queue("", kst(21, 7, 0, 1));
    pipe_at(&roots.line.as_ref().expect("a queue").join(DISK_FLOOR_FLAG));
    let answered = within(5, move || {
        look(
            &mut Book::default(),
            &roots,
            &Ask::default(),
            kst(21, 7, 0, 5),
        )
        .line
        .disk_floor_gib
    });
    assert_eq!(answered, Some(None), "a pipe held the look");
}

#[test]
fn a_disk_floor_past_its_cap_is_not_read() {
    let (_scratch, roots) = lone_queue("", kst(21, 7, 0, 1));
    let queue = roots.line.clone().expect("a queue");
    std::fs::write(
        queue.join(DISK_FLOOR_FLAG),
        format!("{}20", " ".repeat(8 * 1024)),
    )
    .expect("a long floor");
    assert_eq!(read_floor(&queue.join(DISK_FLOOR_FLAG)), None);
}

/// ④ The tail the window opens reads a pipe as nothing at all.
#[cfg(unix)]
#[test]
fn the_log_tail_of_a_pipe_is_missing_and_does_not_wait() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let pipe = scratch.path().join("job.log");
    pipe_at(&pipe);
    let tail = within(5, move || tail_of(Some(&pipe))).expect("the tail waited on a pipe");
    assert!(tail.missing && tail.lines.is_empty());
}

/// ⑥ The caps are the runner's own numbers, read off the runner's script in
/// the queue folder — not copies of them. Without such a script: the runner's
/// last known numbers.
#[test]
fn the_caps_are_read_off_the_runners_script_and_fall_back_without_one() {
    let waiting = |folder: &Path, header: &str| {
        std::fs::write(
            folder.join(QUEUE_IN).join("1792543000-w1-t1-g1-build.job"),
            format!("# who: w1\n# cwd: /x\n{header}"),
        )
        .expect("a waiting job");
    };
    let capped = |roots: &Roots| {
        look(
            &mut Book::default(),
            roots,
            &Ask::default(),
            kst(21, 9, 50, 0),
        )
        .jobs
        .iter()
        .map(|job| job.cap_secs)
        .collect::<Vec<_>>()
    };
    let (_scratch, roots) = lone_queue("", kst(21, 9, 0, 0));
    let queue = roots.line.clone().expect("a queue");
    waiting(&queue, "");
    assert_eq!(
        capped(&roots),
        [900],
        "no script: the runner's last known default"
    );
    waiting(&queue, "# cap: 99999\n");
    assert_eq!(
        capped(&roots),
        [1800],
        "no script: the runner's last known most"
    );
    std::fs::write(
        queue.join("runner10.sh"),
        "#!/bin/zsh\ncap=$(field cap $job)\n  case \"$cap\" in ''|*[!0-9]*) cap=600;; esac; [ $cap -gt 1200 ] && cap=1200\n",
    )
    .expect("the runner's script");
    assert_eq!(capped(&roots), [1200], "the script's most");
    waiting(&queue, "# cap: 300\n");
    assert_eq!(capped(&roots), [300]);
    waiting(&queue, "# cap: nine\n");
    assert_eq!(capped(&roots), [600], "the script's default");
}

/// A project with no coordinator queue shows its jobs on the machine's line
/// (`zo line`, 11-08): this project's, in the schedule's words — a slot is a
/// lane counted from 1, a job taken out before it ran is not drawn — and
/// another project's never; the pause stands as the line's.
#[test]
fn a_project_with_no_queue_shows_its_jobs_on_the_machines_line() {
    use zerocode_core::line::{Class, Job as LineJob, JobState, Line, Pause};
    let scratch = tempfile::tempdir().expect("a scratch folder");
    let shared = scratch.path().join("git");
    std::fs::create_dir_all(&shared).expect("the shared git folder");
    let home = scratch.path().join("zo-home");
    let line = Line::at(&home);
    line.make().expect("the line's folders");
    let job = |id: &str, project: &Path, state: JobState, slot: Option<u32>| LineJob {
        id: id.into(),
        project: project.display().to_string(),
        checkout: "/Users/dev/shop".into(),
        owner: "run-1/w-2".into(),
        what: "cargo test -p shop".into(),
        task: Some("t-7".into()),
        class: Class::First,
        quiet: false,
        solo: false,
        cap_secs: 600,
        command: vec!["cargo".into(), "test".into()],
        notify: None,
        state,
        queued_ms: kst(8, 9, 0, 0),
        started_ms: slot.map(|_| kst(8, 9, 1, 0)),
        ended_ms: None,
        rc: None,
        slot,
        pid: None,
        retry_of: None,
        stop_asked: false,
        ended_by: None,
    };
    line.save(&job("j-ours-running", &shared, JobState::Running, Some(1)))
        .expect("a running job");
    line.save(&job("j-ours-waiting", &shared, JobState::Waiting, None))
        .expect("a waiting job");
    line.save(&job("j-ours-withdrawn", &shared, JobState::Withdrawn, None))
        .expect("a withdrawn job");
    line.save(&job(
        "j-theirs",
        Path::new("/elsewhere/.git"),
        JobState::Waiting,
        None,
    ))
    .expect("another project's job");
    line.set_pause(&Pause {
        by: "person".into(),
        since_ms: kst(8, 9, 2, 0),
        why: "release lane".into(),
        until: None,
    })
    .expect("a pause");
    let roots = Roots {
        line: None,
        builds: Some(shared),
        machine: Some(home),
    };
    let seen = look(
        &mut Book::default(),
        &roots,
        &Ask::default(),
        kst(8, 9, 5, 0),
    );
    let names: Vec<(&str, &str, Option<u8>, &str)> = seen
        .jobs
        .iter()
        .map(|job| (job.name.as_str(), job.state, job.lane, job.class))
        .collect();
    assert_eq!(
        names,
        [
            ("j-ours-running", "running", Some(2), "first"),
            ("j-ours-waiting", "waiting", None, "first")
        ]
    );
    assert_eq!(seen.jobs[0].tasks, ["t-7"]);
    assert!(seen.line.found && seen.line.paused, "{:?}", seen.line);
    assert_eq!(seen.line.pause_note.as_deref(), Some("release lane"));
    assert_eq!(seen.line.runner_lanes, [1, 2]);
    let none = Roots {
        machine: None,
        ..roots
    };
    assert!(
        look(
            &mut Book::default(),
            &none,
            &Ask::default(),
            kst(8, 9, 5, 0)
        )
        .jobs
        .is_empty()
    );
}
