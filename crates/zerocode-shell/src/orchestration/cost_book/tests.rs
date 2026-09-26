use serde_json::json;
use zerocode_core::orchestration::{Draft, MessageKind, Priority, Text, worker_address};
use zerocode_core::usage_stats;
use zerocode_core::{ProviderSession, SessionKey};

use super::*;

const OPUS: &str = "claude-opus-5-5";
const SCANNED: i64 = 10_000_000;
const TEAM: &str = "team-cost";

/// A run with one finished task — one Claude attempt whose conversation the
/// pane reported, reported done — and one still moving: a Codex worker
/// carrying it. Answers the run and the two task ids.
fn a_run(ledger: &mut Ledger, at: i64) -> (String, String, String) {
    let run = ledger.create_run("costs", at);
    let done = ledger
        .create_task(&run, "finish".into(), "finish".into(), vec![], None, at + 1)
        .expect("a task");
    let moving = ledger
        .create_task(&run, "move".into(), "move".into(), vec![], None, at + 2)
        .expect("a task");
    let finisher = ledger
        .start_worker(&run, "claude", (TEAM, "%2"), Some(&done), at + 3)
        .expect("a worker");
    assert!(ledger.worker_session_reported(
        (TEAM, "%2"),
        ProviderSession {
            key: SessionKey::SessionId,
            id: "conv-private-a".into(),
            transcript_path: Some(
                "/Users/dev/.claude/projects/-Users-dev-repo/conv-private-a.jsonl".into()
            ),
        },
    ));
    ledger
        .post(
            &run,
            Draft {
                from: worker_address(&finisher.worker),
                to: format!("run:{run}"),
                kind: MessageKind::WorkerDone,
                body: Text::from(r#"{"ok":true}"#),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::default(),
                thread: None,
                task: Some(done.clone()),
                dispatch: finisher.dispatch.clone(),
            },
            at + 60_000,
        )
        .expect("the report");
    ledger
        .start_worker(&run, "codex", (TEAM, "%3"), Some(&moving), at + 4)
        .expect("another worker");
    (run, done, moving)
}

/// The Claude scan holding that conversation, with where it ran beside it.
fn claude_scan() -> usage_stats::Ledger {
    usage_stats::Ledger {
        sessions: vec![usage_stats::Session {
            session_id: "conv-private-a".into(),
            first_timestamp: "2026-09-26T00:00:00Z".into(),
            last_timestamp: "2026-09-26T01:00:00Z".into(),
            model: Some(OPUS.into()),
            last_cwd: Some("/Users/dev/repo".into()),
            last_git_branch: Some("wt/t-cost".into()),
            turn_count: 4,
            total_input_tokens: 1_000,
            total_output_tokens: 2_000,
            total_cache_read_tokens: 30_000,
            total_cache_write_tokens: 4_000,
            location_breakdown: Vec::new(),
        }],
        daily_aggregates: Vec::new(),
    }
}

fn row(task: &str, requests: u64) -> String {
    format!(
        "{}\n",
        json!({"at": 1, "task": task, "outcome": "answered", "requests": requests})
    )
}

fn append(path: &Path, text: &str) {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .expect("appended");
}

/// The conversations are read into the book once per scan: beats on the
/// same scans read nothing, a new scan reads once.
#[test]
fn the_conversations_are_read_once_per_scan() {
    let mut book = CostBook::default();
    let mut reads = 0;
    for _ in 0..3 {
        book.begin_with([Some(SCANNED), None, None], |_| reads += 1, &[]);
        book.end();
    }
    assert_eq!(reads, 1, "a beat on the same scans read them again");
    book.begin_with([Some(SCANNED), Some(SCANNED), None], |_| reads += 1, &[]);
    book.end();
    assert_eq!(reads, 2, "a new scan was not read");
}

/// A stamping ledger is read on from its last whole row while the rows
/// already read are still the ones it holds, and not at all while its file
/// has not moved: a row half-written waits for its end, and a ledger that
/// shrank, was rewritten to its old length, or had an earlier row written
/// over before it grew is read again from the top. One that is not there
/// reads as nothing, and a task's rows are summed across the ledgers.
#[test]
fn a_jev_ledger_is_read_on_only_while_the_rows_already_read_are_the_ones_it_holds() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let stall = dir.path().join("stall-cause.jsonl");
    let summon = dir.path().join("summon-choice.jsonl");
    let absent = dir.path().join("step-effort.jsonl");
    std::fs::write(&stall, row("t-1", 1) + &row("t-1", 2) + &row("t-2", 9)).expect("rows");
    std::fs::write(&summon, row("t-1", 10)).expect("rows");
    let ledgers = vec![stall.clone(), summon.clone(), absent];
    let mut book = CostBook::default();
    let beat = |book: &mut CostBook| {
        book.begin_with([None; 3], |_| {}, &ledgers);
        book.end();
        (book.jev_tally("t-1").requests, book.jev_reads)
    };
    assert_eq!(beat(&mut book), (13, 2), "the two ledgers, summed");
    append(&stall, &(row("t-1", 4) + r#"{"at":2,"task":"t-1","outc"#));
    assert_eq!(
        beat(&mut book),
        (17, 3),
        "the whole row, and not the half one"
    );
    append(&stall, "ome\":\"answered\",\"requests\":8}\n");
    assert_eq!(beat(&mut book), (25, 4), "the half row, once whole");
    assert_eq!(beat(&mut book), (25, 4), "a quiet beat read a ledger again");

    std::fs::write(&stall, row("t-1", 5)).expect("a shorter ledger");
    assert_eq!(
        beat(&mut book).0,
        15,
        "a ledger that shrank was not read again"
    );

    // Rewritten to the same length, another task's rows where these were.
    let tick = |path: &Path| {
        let written = std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .expect("a clock");
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|file| file.set_modified(written + std::time::Duration::from_secs(1)))
            .expect("the rewrite's own tick");
    };
    std::fs::write(&stall, row("t-3", 5)).expect("the same length, other rows");
    tick(&stall);
    assert_eq!(
        beat(&mut book).0,
        10,
        "a rewrite to the same length was read as the old rows"
    );
    assert_eq!(book.jev_tally("t-3").requests, 5);

    // An earlier row written over in place, and then a row appended: the
    // ledger grew, and what it grew from is no longer what was read.
    std::fs::write(&summon, row("t-1", 10) + &row("t-2", 7)).expect("rows");
    assert_eq!(beat(&mut book).0, 10);
    std::fs::write(&summon, row("t-1", 10) + &row("t-4", 7) + &row("t-1", 1)).expect("rows");
    tick(&summon);
    assert_eq!(beat(&mut book).0, 11);
    assert_eq!(
        book.jev_tally("t-2").requests,
        0,
        "a row written over still counts"
    );
    assert_eq!(book.jev_tally("t-4").requests, 7);
}

/// A finished task's cost is worked out once and remembered: a quiet beat
/// works out nothing, and a new scan or a new row of the task's is what makes
/// it work the cost out again. A cost no beat asked for is let go.
#[test]
fn a_finished_task_is_worked_out_once_until_what_it_is_made_of_moves() {
    let mut ledger = Ledger::new();
    let (run_id, done, _) = a_run(&mut ledger, 1_000);
    let dir = tempfile::tempdir().expect("a scratch directory");
    let summon = dir.path().join("summon-choice.jsonl");
    std::fs::write(&summon, row(&done, 1)).expect("a row");
    let ledgers = vec![summon.clone()];
    let scan = claude_scan();
    let run = ledger.run(&run_id).expect("the run");
    let task = run.task(&done).expect("the task");

    let mut book = CostBook::default();
    let beat = |book: &mut CostBook, scanned: i64| {
        book.begin_with(
            [Some(scanned), None, None],
            |sessions| sessions.read_claude(&scan, scanned),
            &ledgers,
        );
        let cost = book.cost(run, task);
        book.end();
        cost
    };
    let first = beat(&mut book, SCANNED);
    assert_eq!(book.worked, 1, "{first:?}");
    assert_eq!(first.generation.sessions_linked, 1, "{first:?}");
    assert_eq!(first.jev.requests, 1, "{first:?}");
    for _ in 0..3 {
        assert_eq!(beat(&mut book, SCANNED), first);
    }
    assert_eq!(book.worked, 1, "a quiet beat worked a cost out again");

    beat(&mut book, SCANNED + 1);
    assert_eq!(
        book.worked, 2,
        "a new scan left a cost read off the old one"
    );

    append(&summon, &row(&done, 2));
    let again = beat(&mut book, SCANNED + 1);
    assert_eq!(
        book.worked, 3,
        "a new row of the task's left its cost alone"
    );
    assert_eq!(again.jev.requests, 3, "{again:?}");

    book.begin_with(
        [Some(SCANNED + 1), None, None],
        |sessions| sessions.read_claude(&scan, SCANNED + 1),
        &ledgers,
    );
    book.end();
    assert!(book.memo.is_empty(), "a cost no beat asked for was kept");
}

/// The desk's finished rows and the board's worker rows carry the cost of a
/// finished task and nothing for one still moving — and what they carry is
/// numbers and words: no conversation id, no transcript path, no working
/// directory, though the ledger and the scan hold all three beside it.
#[test]
fn the_rows_carry_a_finished_tasks_cost_and_no_conversation_path_or_directory() {
    let mut ledger = Ledger::new();
    let (_, done, moving) = a_run(&mut ledger, 1_000);
    let scan = claude_scan();
    let mut book = CostBook::default();
    book.begin_with(
        [Some(SCANNED), None, None],
        |sessions| sessions.read_claude(&scan, SCANNED),
        &[],
    );
    let desk =
        super::super::desk::desk_snapshot(&ledger, |_| false, |run, task| book.cost(run, task));
    let rows = super::super::ledger_agents_for_seats(&ledger, &super::super::TeamSeatIndex::new());
    let rows = book.dress(&ledger, rows);
    book.end();

    let desk_row = |id: &str| {
        desk.tasks
            .iter()
            .find(|one| one.id == id)
            .unwrap_or_else(|| panic!("{id} is not on the desk"))
    };
    let costed = desk_row(&done)
        .cost
        .clone()
        .expect("the finished row's cost");
    assert_eq!(costed.attempts, 1, "{costed:?}");
    assert_eq!(costed.generation.sessions_linked, 1, "{costed:?}");
    assert_eq!(costed.generation.cache_read_tokens, 30_000, "{costed:?}");
    assert!(costed.generation.usd.is_some(), "{costed:?}");
    assert!(costed.wall_ms.is_some(), "{costed:?}");
    assert!(
        desk_row(&moving).cost.is_none(),
        "a moving task carries a cost"
    );
    let worker_row = |id: &str| {
        rows.iter()
            .find(|one| one.task_id == id)
            .unwrap_or_else(|| panic!("no worker row carries {id}"))
    };
    assert_eq!(worker_row(&done).cost.as_ref(), Some(&costed));
    assert!(worker_row(&moving).cost.is_none());

    let said = [
        serde_json::to_string(&desk_row(&done).cost).expect("json"),
        serde_json::to_string(&worker_row(&done).cost).expect("json"),
    ];
    for said in said {
        for private in [
            "conv-private",
            "/Users",
            ".jsonl",
            "wt/t-cost",
            "\"session\":",
            "\"sessionId\":",
            "\"transcriptPath\":",
            "\"path\":",
            "\"cwd\":",
        ] {
            assert!(!said.contains(private), "`{private}` left in {said}");
        }
        assert!(said.contains("\"sessionsLinked\":1"), "{said}");
    }
}

/// The median and the 95th of some timings, in microseconds.
fn spread(mut took: Vec<u128>) -> serde_json::Value {
    took.sort_unstable();
    json!({ "p50": took[took.len() / 2], "p95": took[took.len() * 95 / 100], "n": took.len() })
}

/// The costs over the ledger and the transcripts that already happened
/// (t-9470): the finished tasks whose last attempt ended at or after an
/// instant, as task ids and numbers only — no conversation id, path or
/// directory leaves — and what the board's beat pays for them on this
/// machine: the book's conversations read once per scan, the Jev ledgers read
/// from the top, the desk built with no cost asked, on a warm book and on a
/// cold one, and every finished task the ledger holds worked out cold.
///
/// ```sh
/// ZEROCODE_DESK_REPLAY_STORE="$HOME/Library/Application Support/dev.zerocode.app/authority/authority.sqlite" \
/// ZEROCODE_COST_CONFIG_ROOT="$HOME/Library/Application Support/dev.zerocode.app" \
/// ZEROCODE_COST_SINCE=<epoch ms> ZEROCODE_COST_RUN=<run id, optional> \
///   cargo test -p zerocode-shell --bin zerocode-shell \
///   orchestration::cost_book::tests::the_costs_over_the_ledger_and_the_transcripts_that_already_happened \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a measurement over the person's own ledger and transcripts, printed; not a check"]
fn the_costs_over_the_ledger_and_the_transcripts_that_already_happened() {
    use std::time::Instant;
    let store = std::env::var("ZEROCODE_DESK_REPLAY_STORE")
        .expect("ZEROCODE_DESK_REPLAY_STORE names the authority store");
    let config_root = PathBuf::from(
        std::env::var("ZEROCODE_COST_CONFIG_ROOT")
            .expect("ZEROCODE_COST_CONFIG_ROOT names the window's config root"),
    );
    let since: i64 = std::env::var("ZEROCODE_COST_SINCE")
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(0);
    let only_run = std::env::var("ZEROCODE_COST_RUN").ok();
    let ledger = Ledger::rebuild(super::super::desk::projection_at_rest(&store))
        .expect("the ledger as it stands");

    let now = crate::now_epoch_ms();
    let scanning = Instant::now();
    let claude = crate::usage_stats_scan::scan(&config_root, &[], 0, now);
    let codex = crate::usage_stats_scan::codex_scan(&config_root, &[], 0, now);
    let opencode = crate::usage_stats_opencode_scan::scan(&[], 0, now);
    let scan_ms = scanning.elapsed().as_millis();
    let scans = [
        Some(claude.scanned_at),
        Some(codex.scanned_at),
        Some(opencode.scanned_at),
    ];
    let read = |sessions: &mut SessionBook| {
        sessions.read_claude(&claude.ledger, claude.scanned_at);
        sessions.read_codex(&codex.ledger, codex.scanned_at);
        sessions.read_opencode(&opencode.ledger, opencode.scanned_at);
    };
    let ledgers = stamping_ledgers();
    let fill = (0..20)
        .map(|_| {
            let from = Instant::now();
            let mut sessions = SessionBook::default();
            read(&mut sessions);
            std::hint::black_box(&sessions);
            from.elapsed().as_micros()
        })
        .collect();
    let jev_read = (0..20)
        .map(|_| {
            let from = Instant::now();
            let mut fresh = CostBook::default();
            fresh.read_jev(&ledgers);
            std::hint::black_box(&fresh.jev);
            from.elapsed().as_micros()
        })
        .collect();

    // A row appended to a scratch copy of the ledgers: what a beat pays to
    // read on — the person's own files are only copied.
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let copies: Vec<PathBuf> = ledgers
        .iter()
        .filter_map(|path| {
            let copy = scratch.path().join(path.file_name()?);
            std::fs::copy(path, &copy).ok()?;
            Some(copy)
        })
        .collect();
    let mut tail = CostBook::default();
    tail.read_jev(&copies);
    let grown = copies
        .iter()
        .find_map(|copy| {
            let text = std::fs::read_to_string(copy).ok()?;
            let last = text.lines().last()?.to_string();
            Some((copy.clone(), format!("{last}\n")))
        })
        .expect("a stamping ledger with a row");
    let appended = (0..20)
        .map(|_| {
            append(&grown.0, &grown.1);
            let from = Instant::now();
            tail.read_jev(&copies);
            from.elapsed().as_micros()
        })
        .collect();

    let mut book = CostBook::default();
    book.begin_with(scans, read, &ledgers);
    let finished: Vec<(&Run, &Task)> = ledger
        .runs()
        .iter()
        .flat_map(|run| run.tasks.iter().map(move |task| (run, task)))
        .filter(|(run, task)| super::super::desk::finished(run, task))
        .collect();
    let last_ended = |run: &Run, task: &Task| {
        task_cost::attempts(run, &task.id)
            .iter()
            .filter_map(|one| one.ended_ms)
            .max()
    };
    let mut tonight: Vec<(&Run, &Task)> = finished
        .iter()
        .copied()
        .filter(|(run, task)| {
            only_run.as_deref().is_none_or(|id| run.id == id)
                && last_ended(run, task).is_some_and(|ended| ended >= since)
        })
        .collect();
    tonight.sort_by_key(|(run, task)| last_ended(run, task));
    for (run, task) in &tonight {
        let cost = book.cost(run, task);
        let generation = &cost.generation;
        println!(
            "{}",
            json!({
                "task": task.id,
                "attempts": cost.attempts,
                "sessions": format!("{}/{}", generation.sessions_linked, generation.sessions_known),
                "tokens": generation.input_tokens + generation.output_tokens
                    + generation.cache_read_tokens + generation.cache_write_tokens,
                "input": generation.input_tokens,
                "output": generation.output_tokens,
                "cacheRead": generation.cache_read_tokens,
                "cacheWrite": generation.cache_write_tokens,
                "usd": generation.usd,
                "usdReason": generation.usd_reason,
                "jevRequests": cost.jev.requests,
                "wallMs": cost.wall_ms,
            })
        );
    }
    book.end();

    let desk_rows = |desk: &super::super::desk::DeskSnapshot| {
        desk.tasks.iter().filter(|one| one.cost.is_some()).count()
    };
    let beat = |book: &mut CostBook, cold: bool| {
        book.begin_with(scans, read, &ledgers);
        if cold {
            book.memo.clear();
        }
        let desk =
            super::super::desk::desk_snapshot(&ledger, |_| false, |run, task| book.cost(run, task));
        book.end();
        desk
    };
    let costed = desk_rows(&beat(&mut book, false));
    let worked_before = book.worked;
    let warm = (0..200)
        .map(|_| {
            let from = Instant::now();
            std::hint::black_box(beat(&mut book, false));
            from.elapsed().as_micros()
        })
        .collect();
    let recomputed_on_quiet_beats = book.worked - worked_before;
    let cold = (0..200)
        .map(|_| {
            let from = Instant::now();
            std::hint::black_box(beat(&mut book, true));
            from.elapsed().as_micros()
        })
        .collect();
    let none = (0..200)
        .map(|_| {
            let from = Instant::now();
            std::hint::black_box(super::super::desk::desk_snapshot(
                &ledger,
                |_| false,
                |_, _| TaskCost::default(),
            ));
            from.elapsed().as_micros()
        })
        .collect();
    let every_finished = (0..20)
        .map(|_| {
            book.memo.clear();
            let from = Instant::now();
            for (run, task) in &finished {
                std::hint::black_box(book.cost(run, task));
            }
            from.elapsed().as_micros()
        })
        .collect();
    let dispatches: usize = ledger.runs().iter().map(|run| run.dispatches.len()).sum();
    println!(
        "{}",
        json!({
            "tonight": tonight.len(),
            "finishedTasks": finished.len(),
            "dispatches": dispatches,
            "costedDeskRows": costed,
            "sessionsHeld": {
                "claude": claude.ledger.sessions.len(),
                "codex": codex.ledger.sessions.len(),
                "opencode": opencode.ledger.sessions.len(),
            },
            "scanMs": scan_ms,
            "bookFillMicros": spread(fill),
            "jevReadMicros": spread(jev_read),
            "jevAppendedRowMicros": spread(appended),
            "jevRowsRead": ledgers.iter().filter_map(|path| std::fs::read_to_string(path).ok()).map(|text| text.lines().count()).sum::<usize>(),
            "deskMicros": {
                "noCost": spread(none),
                "warmBook": spread(warm),
                "coldMemo": spread(cold),
            },
            "recomputedOnQuietBeats": recomputed_on_quiet_beats,
            "everyFinishedColdMicros": spread(every_finished),
        })
    );
}
