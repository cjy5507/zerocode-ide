//! The schedule's reading of the build line and the release lanes (t-67482),
//! over this morning's queue and the lanes v1.1.76–v1.1.83 cleaned of names
//! (`ui/tests/fixtures/board-schedule`). The fixture's moment is 2026-10-21
//! 09:50 KST: the l61 gate waits behind a `PAUSE`, two jobs that started
//! before it still run, and v1.1.83's lane is in its root gate.

use std::time::{Duration, Instant};

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
        release: Some(scratch.join("release")),
    }
}

/// The fixture as the board reads it, from five days back.
fn fixture_reading(scratch: &Path) -> Timeline {
    let ask = Ask {
        known: None,
        from_ms: Some(NOW_MS - 5 * 24 * HOUR_MS),
        log: None,
    };
    Book::default().read(&roots_in(scratch), &ask, NOW_MS, KST)
}

fn job<'a>(timeline: &'a Timeline, name: &str) -> &'a Job {
    timeline
        .jobs
        .iter()
        .find(|job| job.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the answer"))
}

fn lane<'a>(timeline: &'a Timeline, sha: &str) -> &'a LaneRun {
    timeline
        .lanes
        .iter()
        .find(|run| run.sha == sha)
        .unwrap_or_else(|| panic!("lane {sha} is not in the answer"))
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
fn a_lane_log_splits_into_runs_and_tells_its_stages_from_its_verdicts() {
    let text = std::fs::read_to_string(fixture().join("release/out/lane-607e69e3.log"))
        .expect("v1.1.77's log: three runs of one sha");
    let parsed = lane_runs(&text, kst(19, 0, 42, 43) + 400, KST);
    assert_eq!(
        parsed.iter().map(|run| run.start_ms).collect::<Vec<_>>(),
        [kst(18, 14, 5, 48), kst(18, 18, 11, 16), kst(18, 22, 48, 53)]
    );
    let runs = lane_view(&parsed.iter().collect::<Vec<_>>(), None);
    let said: Vec<_> = runs
        .iter()
        .map(|run| (run.verdict.as_deref(), run.reason.as_deref(), run.end_ms))
        .collect();
    assert_eq!(
        said,
        [
            (
                Some("red"),
                Some("solo root harness:knowledge run 2 rc=1"),
                Some(kst(18, 18, 9, 1))
            ),
            (Some("red"), Some("SIGTERM"), Some(kst(18, 22, 31, 51))),
            (Some("green"), None, Some(kst(19, 0, 42, 43))),
        ]
    );
    // A stage killed by a signal never ended; the third run passed midnight.
    let killed = runs[1]
        .stages
        .iter()
        .find(|stage| stage.name == "gate-zo")
        .expect("gate-zo");
    assert_eq!((killed.end_ms, killed.rc), (None, None));
    let green = &runs[2];
    assert_eq!(green.version.as_deref(), Some("1.1.77"));
    let names: Vec<&str> = green
        .stages
        .iter()
        .map(|stage| stage.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "archive",
            "gate-root",
            "gate-zo",
            "flakes",
            "push",
            "build-app",
            "swap-app",
            "build-zo",
            "swap-zo",
            "bundle-updater",
            "publish",
            "sweep",
        ]
    );
    let zo = &green.stages[2];
    assert_eq!(
        (zo.start_ms, zo.end_ms, zo.rc, zo.secs),
        (
            kst(18, 23, 57, 27),
            Some(kst(19, 0, 24, 21)),
            Some(0),
            Some(1614)
        )
    );
    assert!(runs.iter().all(|run| !run.running && run.ahead.is_empty()));
}

#[test]
fn a_running_lanes_stages_ahead_come_from_the_last_full_green_with_their_medians() {
    let scratch = copied();
    let timeline = fixture_reading(scratch.path());
    let running = lane(&timeline, "fc4abd39");
    assert!(running.running && running.end_ms.is_none(), "{running:?}");
    assert_eq!(
        running.version.as_deref(),
        Some("1.1.83"),
        "the version status.json names"
    );
    assert_eq!(
        running
            .stages
            .last()
            .map(|stage| (stage.name.as_str(), stage.start_ms, stage.end_ms)),
        Some(("gate-root", kst(21, 9, 48, 0), None))
    );
    let ahead: Vec<(&str, u32, u8)> = running
        .ahead
        .iter()
        .map(|stage| {
            (
                stage.name.as_str(),
                stage.expected.secs,
                stage.expected.samples,
            )
        })
        .collect();
    assert_eq!(
        ahead.iter().map(|(name, ..)| *name).collect::<Vec<_>>(),
        [
            "gate-root",
            "gate-zo",
            "flakes",
            "push",
            "build-app",
            "swap-app",
            "build-zo",
            "swap-zo",
            "bundle-updater",
            "publish",
            "sweep",
        ]
    );
    // The last five root gates that passed: v1.1.78 5288 s, v1.1.79 3365 s, v1.1.80 3232 s,
    // v1.1.81 3331 s, v1.1.82 3706 s — wall clock from the stage's line to its end's.
    assert_eq!(ahead[0], ("gate-root", 3365, 5));
    assert_eq!(ahead[1], ("gate-zo", 1764, 5));
    assert_eq!(ahead[4], ("build-app", 1122, 5));
    // A lane that ended is never running, and says its own version.
    let ended = lane(&timeline, "3e13aaef");
    assert_eq!(
        (
            ended.running,
            ended.verdict.as_deref(),
            ended.version.as_deref(),
            ended.end_ms
        ),
        (
            false,
            Some("green"),
            Some("1.1.82"),
            Some(kst(21, 4, 51, 31))
        )
    );
}

/// What a whole lane is expected to take before it starts, from the lanes that
/// ended before it — the sum the window draws a running lane's end from.
fn expected_before(runs: &[LaneRun], start_ms: i64) -> u32 {
    let before: Vec<LaneRun> = runs
        .iter()
        .filter(|run| run.start_ms < start_ms)
        .cloned()
        .collect();
    let past = stage_past(&before);
    lane_plan(&before)
        .iter()
        .map(|name| expected_of(past.get(name).map(Vec::as_slice)).secs)
        .sum()
}

/// The ETA a lane is given at its start, against its real end, over the
/// fixture's lanes (v1.1.76–v1.1.83). v1.1.79–v1.1.82 land within fifteen
/// minutes. v1.1.83 does not, and is written down rather than bent into a
/// model (the coordinator's call, m-67652): it started at load 4.3, then its
/// build-app ran 2726 s against a median of 1122 s at load 108–126 on twelve
/// cores, after a flaky zo test's solo reruns added 193 s — nothing a start
/// can see. What the view does then is say so (`ui/tests/board-schedule.mjs`:
/// the delay tag and the load mark).
#[test]
fn the_lane_eta_at_its_start_lands_within_fifteen_minutes() {
    let out = fixture().join("release/out");
    let mut parsed = Vec::new();
    let stamps: HashMap<String, i64> = serde_json::from_str(
        &std::fs::read_to_string(fixture().join("stamps.json")).expect("the stamps"),
    )
    .expect("the stamps are JSON");
    for entry in std::fs::read_dir(&out).expect("the lane logs") {
        let path = entry.expect("an entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name")
            .to_string();
        if name == "lane-fc4abd39.log" {
            continue;
        }
        let anchor = stamps[&format!("release/out/{name}")];
        let text = std::fs::read_to_string(&path).expect("a lane log");
        parsed.extend(lane_runs(&text, anchor, KST));
    }
    // v1.1.83's whole log, as the lane left it when it ended.
    let ended = fixture().join("lanes-ended/lane-fc4abd39.log");
    let anchor = stamps["lanes-ended/lane-fc4abd39.log"];
    parsed.extend(lane_runs(
        &std::fs::read_to_string(ended).expect("v1.1.83 ended"),
        anchor,
        KST,
    ));
    let runs = lane_view(&parsed.iter().collect::<Vec<_>>(), None);
    let mut offs = Vec::new();
    for (sha, version) in [
        ("4e1ce638", "1.1.79"),
        ("bba0342d", "1.1.80"),
        ("76df2c46", "1.1.81"),
        ("3e13aaef", "1.1.82"),
        ("fc4abd39", "1.1.83"),
    ] {
        let run = runs.iter().find(|run| run.sha == sha).expect("the lane");
        assert_eq!(
            (run.verdict.as_deref(), run.version.as_deref()),
            (Some("green"), Some(version)),
            "{sha} ended green"
        );
        let took = (run.end_ms.expect("an end") - run.start_ms) / 1000;
        let expected = i64::from(expected_before(&runs, run.start_ms));
        let off = (expected - took).abs();
        eprintln!(
            "lane v{version}: took {took} s, expected at its start {expected} s, off {off} s ({:.1} min)",
            off as f64 / 60.0
        );
        offs.push((version, off));
    }
    for (version, off) in &offs[..4] {
        assert!(
            *off <= 15 * 60,
            "v{version}'s ETA at its start was {off} s off"
        );
    }
    assert!(
        offs[4].1 > 15 * 60,
        "v1.1.83 is the lane a load spike overran; if it lands now, this note is stale"
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
    assert_eq!(median(&[3, 1, 2]), Some(2));
    assert_eq!(median(&[1, 2, 3, 10]), Some(2));
    assert_eq!(median(&[]), None);
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
    // The lanes from five days back: v1.1.77's three runs are three rows.
    assert_eq!(
        timeline
            .lanes
            .iter()
            .filter(|run| run.sha == "607e69e3")
            .count(),
        3
    );
    assert_eq!(timeline.lanes.iter().filter(|run| run.running).count(), 1);
    // The answer the window's scene draws (`ui/tests/board-schedule.mjs`), value for value.
    let mut said = serde_json::to_value(&timeline).expect("the answer serializes");
    said["line"]["folder"] = serde_json::json!("<fixture>/build-queue");
    said["line"]["lanes_folder"] = serde_json::json!("<fixture>/release/out");
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
    let first = book.read(&roots, &Ask::default(), NOW_MS, KST);
    assert!(!first.unchanged && !first.jobs.is_empty());
    let known = Ask {
        known: Some(first.revision),
        ..Ask::default()
    };
    let again = book.read(&roots, &known, NOW_MS + 5_000, KST);
    assert!(
        again.unchanged && again.jobs.is_empty() && again.lanes.is_empty(),
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
    let moved = book.read(&roots, &known, NOW_MS + 10_000, KST);
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
        release: None,
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
    let read = Book::default().read(&roots, &ask, NOW_MS, KST);
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
    let read = book.read(&roots, &ask(None), NOW_MS, KST);
    assert_eq!(job(&read, first).tasks, ["t-1"]);
    assert!(job(&read, late).tasks.is_empty());
    assert!(
        book.read(&roots, &ask(Some(read.revision)), NOW_MS, KST)
            .unchanged,
        "a look where nothing moved"
    );

    std::fs::write(file(first), "# who: w1-t1\n# task: t-5\n").expect("rewritten");
    stamp_at(&file(first), kst(21, 9, 40, 0));
    std::fs::write(file(late), "# who: w2-t2\n# task: t-2\n").expect("arrived");
    let moved = book.read(&roots, &ask(Some(read.revision)), NOW_MS, KST);
    assert!(
        !moved.unchanged,
        "the ties moved and the view was told nothing"
    );
    assert!(moved.revision > read.revision);
    assert_eq!(job(&moved, first).tasks, ["t-5"]);
    assert_eq!(job(&moved, late).tasks, ["t-2"]);
    assert!(
        book.read(&roots, &ask(Some(moved.revision)), NOW_MS, KST)
            .unchanged,
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
    book.read(&roots, &ask(None), NOW_MS, KST);
    book.after_tie = Some(|roots| {
        let done = roots.line.as_ref().expect("the line").join(QUEUE_DONE);
        std::fs::write(
            done.join(format!("{GAP}{JOB_SUFFIX}")),
            "# who: w2-t2\n# task: t-2\n",
        )
        .expect("arrives in the gap");
    });
    let tied = book.read(&roots, &ask(None), NOW_MS, KST);
    assert!(
        job(&tied, GAP).tasks.is_empty(),
        "the tie was made before the file came"
    );
    book.after_tie = None;
    let next = book.read(&roots, &ask(Some(tied.revision)), NOW_MS, KST);
    assert!(
        !next.unchanged,
        "the file that came in the gap is a change the view was never told of"
    );
    assert_eq!(job(&next, GAP).tasks, ["t-2"]);
    assert!(
        book.read(&roots, &ask(Some(next.revision)), NOW_MS, KST)
            .unchanged,
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
    let read = Book::default().read(&roots, &ask, NOW_MS, KST);
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
    let first = book.read(&roots, &Ask::default(), NOW_MS, KST);
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
    let moved = book.read(&roots, &known(first.revision), NOW_MS + 5_000, KST);
    assert!(!moved.unchanged && moved.revision > first.revision);
    assert_eq!(moved.landings.get("t-66760"), Some(&landed));
    assert_eq!(
        serde_json::to_value(&moved).expect("the answer serializes")["landings"]["t-66760"],
        serde_json::json!({"landed_ms": NOW_MS - HOUR_MS, "released_ms": null}),
        "the window reads these keys"
    );
    book.land(told());
    let again = book.read(&roots, &known(moved.revision), NOW_MS + 10_000, KST);
    assert!(again.unchanged && again.revision == moved.revision);
}

#[test]
fn a_log_is_read_only_for_a_job_or_a_lane_this_reading_listed() {
    let scratch = copied();
    let roots = roots_in(scratch.path());
    let mut book = Book::default();
    let read = |book: &mut Book, log: LogAsk| {
        book.read(
            &roots,
            &Ask {
                log: Some(log),
                ..Ask::default()
            },
            NOW_MS,
            KST,
        )
        .log
        .expect("a log answer")
    };
    let build = read(
        &mut book,
        LogAsk::Job("0-1792535939-run11955-l60r-g1-build".into()),
    );
    assert!(!build.missing && build.cut, "{:?}", build.lines.first());
    assert_eq!(build.lines.len(), LOG_TAIL_LINES);
    assert_eq!(
        build.lines.last().map(String::as_str),
        Some("    Finished `dev` profile [unoptimized] target(s) in 3m 39s")
    );
    let running = read(
        &mut book,
        LogAsk::Job("1792543500-run11955-t-63492-walk".into()),
    );
    assert_eq!(
        (running.missing, running.cut, running.lines.len()),
        (false, false, 4)
    );
    for stranger in [
        "../runner",
        "../../release/status.json",
        ".hidden",
        "not-a-job-here",
        "",
    ] {
        let refused = read(&mut book, LogAsk::Job(stranger.into()));
        assert!(refused.missing && refused.lines.is_empty(), "{stranger:?}");
    }
    let lane = read(&mut book, LogAsk::Lane("3e13aaef".into()));
    assert!(!lane.missing && lane.lines.iter().any(|line| line.ends_with("== green")));
    assert!(read(&mut book, LogAsk::Lane("../out".into())).missing);
}

#[test]
fn a_look_costs_little_when_nothing_moved() {
    let scratch = copied();
    let roots = roots_in(scratch.path());
    let mut book = Book::default();
    let first = book.read(&roots, &Ask::default(), NOW_MS, KST);
    let known = Ask {
        known: Some(first.revision),
        ..Ask::default()
    };
    let mut took: Vec<f64> = (0..200)
        .map(|_| {
            let began = Instant::now();
            let quiet = book.read(&roots, &known, NOW_MS, KST);
            assert!(quiet.unchanged);
            began.elapsed().as_secs_f64() * 1_000_000.0
        })
        .collect();
    took.sort_by(f64::total_cmp);
    let (middle, high) = (took[took.len() / 2], took[took.len() * 95 / 100]);
    eprintln!(
        "a quiet look: {middle:.0} µs median, {high:.0} µs p95 — every 5 s while the view is open"
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
        release: None,
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
    book.read(&roots, &Ask::default(), kst(21, 7, 0, 2), KST);
    let filler = "orchestration echo that tells nothing\n".repeat(6 * 1024 * 1024 / 38);
    append(
        &runner_log(&roots),
        &format!(
            "RUN 1792535100-w1-t1-gone who=w1 lane=1 cap=900s 07:01:00\n{filler}RUN 1792535400-w1-t1-late who=w1 lane=1 cap=900s 07:05:00\n"
        ),
        kst(21, 7, 6, 0),
    );
    book.read(&roots, &Ask::default(), kst(21, 7, 6, 5), KST);
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
    book.read(&roots, &Ask::default(), kst(21, 7, 34, 0), KST);
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
    book.read(&roots, &Ask::default(), kst(21, 7, 51, 0), KST);
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
    book.read(&roots, &Ask::default(), kst(21, 7, 0, 30), KST);
    assert_eq!(book.runner.open.len(), 2);
    // Written over in place — one inode, longer than before, other bytes where the offset was.
    let rewritten = "RUN 1792536000-w9-t9-new-one who=w9 lane=2 cap=600s 08:00:00\n\
                     RUN 1792536010-w9-t9-new-two who=w9 lane=2 cap=600s 08:00:10\n\
                     RUN 1792536020-w9-t9-new-three who=w9 lane=2 cap=600s 08:00:20\n";
    std::fs::write(runner_log(&roots), rewritten).expect("written over");
    stamp_at(&runner_log(&roots), kst(21, 8, 0, 30));
    book.read(&roots, &Ask::default(), kst(21, 8, 0, 40), KST);
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
    book.read(&roots, &Ask::default(), kst(21, 7, 0, 5), KST);
    assert!(book.runner.open.contains_key("1792535000-w1-t1-stale"));
    // Two hours on the runner writes again; the job is far past its minute.
    append(
        &runner_log(&roots),
        "PAUSE lifted 09:00:00 by coord4275: the lane ended\n",
        kst(21, 9, 0, 1),
    );
    let later = book.read(&roots, &Ask::default(), kst(21, 9, 0, 5), KST);
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
    let ended = book.read(&roots, &Ask::default(), kst(21, 9, 1, 5), KST);
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
    let first = book.read(&roots, &Ask::default(), kst(21, 7, 0, 5), KST);
    let known = Ask {
        known: Some(first.revision),
        ..Ask::default()
    };
    let deadline = kst(21, 7, 0, 0) + 60_000 + LOST_AFTER_CAP_MS;
    let at_cap = book.read(&roots, &known, deadline, KST);
    assert!(
        at_cap.unchanged,
        "a run at its grace boundary expired early"
    );
    let expired = book.read(&roots, &known, deadline + 1, KST);
    assert!(
        !expired.unchanged,
        "the clock moved past the grace but the answer did not"
    );
    assert!(expired.revision > first.revision);
    assert_eq!(job(&expired, name).state, LOST);
    assert!(!book.runner.open.contains_key(name));
    let later = book.read(
        &roots,
        &Ask {
            known: Some(expired.revision),
            ..Ask::default()
        },
        deadline + HOUR_MS,
        KST,
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
    let first = book.read(&roots, &Ask::default(), kst(21, 7, 0, 5), KST);
    let quiet = book.read(
        &roots,
        &Ask {
            known: Some(first.revision),
            ..Ask::default()
        },
        kst(21, 9, 0, 5),
        KST,
    );
    assert!(quiet.unchanged, "a job still owned by the runner expired");
    let later = book.read(&roots, &Ask::default(), kst(21, 9, 0, 5), KST);
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
    book.read(&roots, &Ask::default(), kst(21, 7, 0, 5), KST);
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
fn a_pipe_in_place_of_status_or_the_disk_floor_does_not_hold_the_look() {
    let (scratch, mut roots) = lone_queue("", kst(21, 7, 0, 1));
    let release = scratch.path().join("release");
    std::fs::create_dir_all(release.join("out")).expect("the lanes' folder");
    pipe_at(&release.join("status.json"));
    pipe_at(&roots.line.as_ref().expect("a queue").join(DISK_FLOOR_FLAG));
    roots.release = Some(release);
    let answered = within(5, move || {
        Book::default()
            .read(&roots, &Ask::default(), kst(21, 7, 0, 5), KST)
            .line
            .disk_floor_gib
    });
    assert_eq!(answered, Some(None), "a pipe held the look");
}

#[test]
fn a_status_file_or_disk_floor_past_its_cap_is_not_read() {
    let (scratch, roots) = lone_queue("", kst(21, 7, 0, 1));
    let queue = roots.line.clone().expect("a queue");
    std::fs::write(
        queue.join(DISK_FLOOR_FLAG),
        format!("{}20", " ".repeat(8 * 1024)),
    )
    .expect("a long floor");
    let heavy = format!(
        "{{\"sha\": \"fc4abd39\", \"pad\": \"{}\", \"outcome\": null}}",
        "x".repeat(2 * 1024 * 1024)
    );
    let release = scratch.path().join("release");
    std::fs::create_dir_all(release.join("out")).expect("the lanes' folder");
    std::fs::write(release.join("status.json"), heavy).expect("a heavy status");
    assert!(
        lane_status(&release.join("status.json")).is_none(),
        "a 2 MiB status.json was read"
    );
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

/// ④ A lane log over its cap is read by its last stretch: the runs above it
/// are not in the answer.
#[test]
fn a_lane_log_over_its_cap_is_read_by_its_last_stretch() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let release = scratch.path().join("release");
    let out = release.join("out");
    std::fs::create_dir_all(&out).expect("the lanes' folder");
    let log = out.join("lane-aaaaaaa1.log");
    let filler =
        "08:00:01 a line that belongs to no stage and fills the log\n".repeat(8 * 1024 * 1024 / 60);
    std::fs::write(
        &log,
        format!(
            "08:00:00 == lane aaaaaaa1 start (dry=0)\n{filler}09:00:00 == lane aaaaaaa2 start (dry=0)\n09:00:01 == archive\n"
        ),
    )
    .expect("a long lane log");
    stamp_at(&log, kst(21, 9, 0, 2));
    let roots = Roots {
        line: None,
        release: Some(release),
    };
    let timeline = Book::default().read(&roots, &Ask::default(), kst(21, 9, 1, 0), KST);
    let shas: Vec<&str> = timeline.lanes.iter().map(|run| run.sha.as_str()).collect();
    assert_eq!(
        shas,
        ["aaaaaaa2"],
        "the 8 MiB log was read from its first byte"
    );
}

/// ⑤ A lane's stages ahead come from the last lane that passed every stage,
/// however many newer logs hold none — and the answer says whether it had one.
#[test]
fn a_full_green_older_than_the_newest_eight_logs_still_plans_the_stages_ahead() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let release = scratch.path().join("release");
    let out = release.join("out");
    std::fs::create_dir_all(&out).expect("the lanes' folder");
    let write_log = |sha: &str, text: &str, at_ms: i64| {
        let path = out.join(format!("lane-{sha}.log"));
        std::fs::write(&path, text).expect("a lane log");
        stamp_at(&path, at_ms);
    };
    write_log(
        "aaaaaaa0",
        "08:00:00 == lane aaaaaaa0 start (dry=0)\n08:00:00 == archive\n08:00:10 archive rc=0 10s free=20G\n\
         08:00:10 == gate\n08:10:10 gate rc=0 600s free=20G\n08:10:10 == push\n08:10:20 push rc=0 10s free=20G\n08:10:20 == green\n",
        kst(21, 8, 11, 0),
    );
    for n in 1..=8 {
        let sha = format!("bbbbbbb{n}");
        write_log(
            &sha,
            &format!(
                "08:3{n}:00 == lane {sha} start (dry=0)\n08:3{n}:00 == archive\n08:3{n}:10 archive rc=0 10s free=20G\n\
                 08:3{n}:10 == gate\n08:3{n}:30 gate rc=1 20s free=20G\n08:3{n}:30 == red (gate)\n"
            ),
            kst(21, 8, 40 + n, 0),
        );
    }
    write_log(
        "ccccccc0",
        "09:30:00 == lane ccccccc0 start (dry=0)\n09:30:00 == archive\n09:30:10 archive rc=0 10s free=20G\n09:30:10 == gate\n",
        kst(21, 9, 40, 0),
    );
    std::fs::write(
        release.join("status.json"),
        "{\"sha\": \"ccccccc0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\", \"phase\": \"gate\", \"outcome\": null, \"version\": \"1.2.3\"}",
    )
    .expect("the status");
    let roots = Roots {
        line: None,
        release: Some(release.clone()),
    };
    let timeline = Book::default().read(&roots, &Ask::default(), kst(21, 9, 50, 0), KST);
    let running = lane(&timeline, "ccccccc0");
    assert!(running.running, "{running:?}");
    assert_eq!(
        running
            .ahead
            .iter()
            .map(|stage| stage.name.as_str())
            .collect::<Vec<_>>(),
        ["gate", "push"],
        "nine newer logs hid the full green that plans the stages ahead"
    );
    let said = serde_json::to_value(running).expect("serializes");
    assert_eq!(said["ahead_planned"], serde_json::json!(true));
    // With no lane that passed every stage anywhere, the answer says it has no plan.
    std::fs::remove_file(out.join("lane-aaaaaaa0.log")).expect("the green goes");
    let bare = Book::default().read(&roots, &Ask::default(), kst(21, 9, 50, 0), KST);
    let said = serde_json::to_value(lane(&bare, "ccccccc0")).expect("serializes");
    assert_eq!(
        said["ahead_planned"],
        serde_json::json!(false),
        "a lane with no plan was given an estimate that looks whole"
    );
}

/// t-83513: the steps ahead of a running lane are the ones its status.json
/// says it runs (`plan`), each with the median of its past where it has one —
/// not the steps of an older green, which a lane that changed its steps would
/// still carry. A planned step a run began and never ended is a stage of that
/// run, not its verdict.
#[test]
fn a_running_lanes_steps_ahead_are_the_ones_its_status_says_it_runs() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let release = scratch.path().join("release");
    let out = release.join("out");
    std::fs::create_dir_all(&out).expect("the lanes' folder");
    let write_log = |sha: &str, text: &str, at_ms: i64| {
        let path = out.join(format!("lane-{sha}.log"));
        std::fs::write(&path, text).expect("a lane log");
        stamp_at(&path, at_ms);
    };
    // An older green of the lane's old steps, and a red that stopped inside a
    // step no run ever ended.
    write_log(
        "aaaaaaa0",
        "08:00:00 == lane aaaaaaa0 start (dry=0)\n08:00:00 == archive\n08:00:10 archive rc=0 10s free=20G\n\
         08:00:10 == gate\n08:10:10 gate rc=0 600s free=20G\n08:10:10 == push\n08:10:20 push rc=0 10s free=20G\n08:10:20 == green\n",
        kst(21, 8, 11, 0),
    );
    write_log(
        "bbbbbbb0",
        "08:30:00 == lane bbbbbbb0 start (dry=0)\n08:30:00 == archive\n08:30:10 archive rc=0 10s free=20G\n\
         08:30:10 == build\n08:40:00 build was cut\n08:40:00 == red (build)\n",
        kst(21, 8, 41, 0),
    );
    write_log(
        "ccccccc0",
        "09:30:00 == lane ccccccc0 start (dry=0)\n09:30:00 == archive\n09:30:10 archive rc=0 10s free=20G\n09:30:10 == gate\n",
        kst(21, 9, 40, 0),
    );
    let status = |plan: &str| {
        std::fs::write(
            release.join("status.json"),
            format!(
                "{{\"sha\": \"ccccccc0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\", \"phase\": \"gate\", \"outcome\": null{plan}}}"
            ),
        )
        .expect("the status");
    };
    status(
        ", \"plan\": [{\"name\": \"archive\", \"words\": {\"ko\": \"묶기\"}}, {\"name\": \"gate\"}, \
         {\"name\": \"build\"}, {\"name\": \"publish\", \"words\": {}}]",
    );
    let roots = Roots {
        line: None,
        release: Some(release.clone()),
    };
    let timeline = Book::default().read(&roots, &Ask::default(), kst(21, 9, 50, 0), KST);
    let running = lane(&timeline, "ccccccc0");
    assert!(running.running && running.ahead_planned, "{running:?}");
    assert_eq!(
        running
            .ahead
            .iter()
            .map(|stage| (
                stage.name.as_str(),
                stage.expected.secs,
                stage.expected.samples
            ))
            .collect::<Vec<_>>(),
        [("gate", 600, 1), ("build", 0, 0), ("publish", 0, 0)],
        "the steps the lane says it runs, with the past each has"
    );
    let cut = lane(&timeline, "bbbbbbb0");
    assert_eq!(
        (
            cut.stages
                .iter()
                .map(|stage| stage.name.as_str())
                .collect::<Vec<_>>(),
            cut.verdict.as_deref(),
        ),
        (vec!["archive", "build"], Some("red")),
        "a planned step a run never ended is that run's stage, not its verdict"
    );
    // A lane that says no steps is planned, as before, by the last full green.
    status("");
    let unsaid = Book::default().read(&roots, &Ask::default(), kst(21, 9, 50, 0), KST);
    assert_eq!(
        lane(&unsaid, "ccccccc0")
            .ahead
            .iter()
            .map(|stage| stage.name.as_str())
            .collect::<Vec<_>>(),
        ["gate", "push"]
    );
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
        Book::default()
            .read(roots, &Ask::default(), kst(21, 9, 50, 0), KST)
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

/// ⑦ The answer says how each lane ended in the board's own words, so the
/// window compares none of the lane's: `passed` for the explicit green
/// verdict, `failed` for red, `ended` otherwise.
#[test]
fn a_lane_run_carries_the_result_the_window_reads() {
    let scratch = copied();
    let timeline = fixture_reading(scratch.path());
    let said = serde_json::to_value(&timeline).expect("the answer serializes");
    let lanes = said["lanes"].as_array().expect("lanes");
    assert!(!lanes.is_empty());
    for lane in lanes {
        let verdict = lane["verdict"].as_str();
        let want = match (verdict, lane["running"].as_bool()) {
            (_, Some(true)) => "running",
            (Some("green"), _) => "passed",
            (Some("red"), _) => "failed",
            _ => "ended",
        };
        assert_eq!(lane["result"], serde_json::json!(want), "{lane}");
    }
}

/// lane.sh's judge_sha can reject an unknown sha before archive. sweep_sha
/// still logs a successful sweep, then the explicit `== red (reason)` result.
/// That cleanup must never teach the reader that red means passed.
fn verdict_fixture() -> Vec<ParsedRun> {
    let text = std::fs::read_to_string(fixture().join("verdicts/lane-aaaaaaaa.log"))
        .expect("the scrubbed lane verdict log");
    lane_runs(&text, kst(21, 8, 4, 46), KST)
}

#[test]
fn lane_verdicts_do_not_learn_success_from_cleanup_only_failures() {
    let parsed = verdict_fixture();
    let read = |runs: &[&ParsedRun]| {
        lane_view(runs, None)
            .into_iter()
            .map(|run| (run.verdict, run.result))
            .collect::<Vec<_>>()
    };
    let alone = read(&[&parsed[1], &parsed[4]]);
    let mixed = read(&parsed.iter().collect::<Vec<_>>());
    eprintln!(
        "gate failure alone: {}; with cleanup-only failures: {}",
        alone[0].1, mixed[1].1
    );
    assert_eq!(
        alone,
        [(Some("red".into()), FAILED), (Some("green".into()), PASSED)]
    );
    assert_eq!(
        mixed[1], alone[0],
        "a cleanup-only red changed another red's result"
    );
    assert_eq!(mixed[4], alone[1], "the true green changed its result");
    assert_eq!(mixed[0].1, FAILED, "unknown sha ended red");
    assert_eq!(mixed[2].1, ENDED, "a refusal did not pass");
    assert_eq!(mixed[3].1, ENDED, "a superseded run did not pass");
}

#[test]
fn lane_verdicts_do_not_need_a_successful_neighbor_to_read_red() {
    let parsed = verdict_fixture();
    let runs = lane_view(&[&parsed[1]], None);
    assert_eq!(runs[0].result, FAILED);
}

#[test]
fn lane_verdicts_cleanup_only_failures_do_not_plan_a_running_lane() {
    let parsed = verdict_fixture();
    let runs = lane_view(&[&parsed[0], &parsed[2], &parsed[3]], None);
    assert!(
        lane_plan(&runs).is_empty(),
        "failed cleanup became a successful lane plan"
    );
}

#[test]
fn lane_verdicts_failed_gate_durations_do_not_enter_successful_history() {
    let parsed = verdict_fixture();
    let runs = lane_view(&parsed.iter().collect::<Vec<_>>(), None);
    assert_eq!(stage_past(&runs).get("gate-root"), Some(&vec![10]));
}

#[test]
fn lane_verdicts_a_gate_cleared_by_flakes_is_still_successful() {
    let mut parsed = verdict_fixture().remove(4);
    // lane.sh accepts a failed gate once its solo flake judgments pass.
    // The final green is authoritative even with that gate's nonzero code.
    for mark in &mut parsed.marks {
        if let LaneMark::End { name, rc, .. } = mark
            && name == "gate-root"
        {
            *rc = 1;
        }
    }
    let runs = lane_view(&[&parsed], None);
    assert_eq!(runs[0].result, PASSED);
    assert_eq!(stage_past(&runs).get("gate-root"), Some(&vec![10]));
    assert!(lane_plan(&runs).iter().any(|name| name == "publish"));
}

#[test]
fn lane_verdicts_an_unknown_outcome_is_not_learned_as_success() {
    let mut parsed = verdict_fixture().remove(4);
    for mark in &mut parsed.marks {
        if let LaneMark::Begin { name, .. } = mark
            && name == "green"
        {
            *name = "unknown-outcome".into();
        }
    }
    let runs = lane_view(&[&parsed], None);
    assert_eq!(runs[0].result, ENDED);
    assert!(lane_plan(&runs).is_empty());
}
