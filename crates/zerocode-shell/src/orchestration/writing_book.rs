//! The window's half of the writing badge on a finished task (t-32786): the
//! summary its own worker handed in, linted once and remembered, so a beat
//! that moved nothing works out nothing.
//!
//! The rule is the core's ([`zerocode_core::plain_text::lint`]): a pure
//! function of the text that counts and never refuses. This is where the
//! counts are held. Nothing is stored in the ledger: the summary is already
//! there, and a count worked out from it again is the same count. The board's
//! beat asks about the finished rows of two surfaces — the worker rows the task
//! board lists and the desk's pipeline, at most [`super::desk::STAGE_ROWS`] a
//! stage — and both ask by the same task, so one lint serves both. The book
//! holds at most the rows a beat asked for and lets go of the rest. A beat that
//! finds a row's report unchanged neither parses it nor lints it again.

use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use zerocode_core::orchestration::{Ledger, Run, Task, is_workers_own, worker_summary_in};
use zerocode_core::plain_text::{LintMemo, TextLint};

use super::LedgerAgent;
use super::desk::{self, DeskSnapshot, FINISHED_STAGES};

/// What the writing lint counted in each finished task's summary, as far as the
/// board's beat has asked.
#[derive(Debug, Default)]
pub(crate) struct WritingBook {
    memo: LintMemo,
}

static BOOK: LazyLock<Mutex<WritingBook>> = LazyLock::new(Mutex::default);

/// The window's one book, for the board's beat.
pub(crate) fn book() -> MutexGuard<'static, WritingBook> {
    BOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WritingBook {
    /// Opens a beat.
    pub(crate) fn begin(&mut self) {
        self.memo.begin();
    }

    /// Lets go of every lint this beat did not ask for.
    pub(crate) fn end(&mut self) {
        self.memo.end();
    }

    /// The desk, with the lint of the summary on each finished row whose worker
    /// wrote one.
    pub(crate) fn dress_desk(&mut self, ledger: &Ledger, mut desk: DeskSnapshot) -> DeskSnapshot {
        dress_desk_with(&mut self.memo, ledger, &mut desk);
        desk
    }

    /// The worker rows the task board lists, each carrying its finished task's
    /// lint where its worker wrote a summary.
    pub(crate) fn dress_agents(
        &mut self,
        ledger: &Ledger,
        mut agents: Vec<LedgerAgent>,
    ) -> Vec<LedgerAgent> {
        dress_agents_with(&mut self.memo, ledger, &mut agents);
        agents
    }
}

/// Every finished row the desk carries asks the memo for the lint of its
/// worker's summary. A row whose worker wrote none, and a summary with nothing
/// to count, wear nothing.
fn dress_desk_with(memo: &mut LintMemo, ledger: &Ledger, desk: &mut DeskSnapshot) {
    for row in desk
        .tasks
        .iter_mut()
        .filter(|row| FINISHED_STAGES.contains(&row.stage))
    {
        let Some(run) = ledger.run(&row.run) else {
            continue;
        };
        let Some(task) = run.task(&row.id) else {
            continue;
        };
        row.writing = writing_of(memo, run, task);
    }
}

/// The same for the worker rows: the lint rides the row whose task is finished
/// ([`desk::finished`]), as its cost does.
fn dress_agents_with(memo: &mut LintMemo, ledger: &Ledger, agents: &mut [LedgerAgent]) {
    for row in agents.iter_mut() {
        let Some(run) = ledger.run(&row.run) else {
            continue;
        };
        let Some(task) = run.task(&row.task_id) else {
            continue;
        };
        if desk::finished(run, task) {
            row.writing = writing_of(memo, run, task);
        }
    }
}

/// The lint of what a task's own worker wrote for a person, asked of the memo
/// by what the summary is read from: the result's words and whether the worker
/// wrote them. While both stand, the beat parses and lints nothing — not even
/// when a coordinator replaced the result and the summary is read from the
/// worker's report in the run's mail, which is searched once.
fn writing_of(memo: &mut LintMemo, run: &Run, task: &Task) -> Option<TextLint> {
    let author = task.result_author.as_ref();
    let stamp = (task.result.as_str(), is_workers_own(author));
    memo.lint_in(&run.id, &task.id, &stamp, || worker_summary_in(run, task))
        .filter(|found| found.sentences > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::orchestration::task_cost::TaskCost;
    use zerocode_core::orchestration::{ResultAuthor, TaskStatus};

    /// A Korean summary with two codebase metaphors and one translationese
    /// phrase: the numbers below are what the lint counts in it.
    const SUMMARY: &str =
        "Jev 자리의 판정이 느려서 스윕 박자를 조정함으로써 재시도가 줄어들게 되는 것이다.";

    fn worker() -> ResultAuthor {
        ResultAuthor::Worker {
            worker: "w-1".into(),
            dispatch: None,
        }
    }

    fn body(summary: &str) -> String {
        serde_json::json!({ "ok": true, "summary": summary }).to_string()
    }

    /// A run somebody is still at, with one completed task whose result
    /// `author` wrote: the ledger, the run's id and the task's id.
    fn a_finished_task(result: &str, author: ResultAuthor) -> (Ledger, String, String) {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("writing", 1);
        ledger
            .start_worker(&run, "claude", ("team-writing", "%2"), None, 2)
            .expect("somebody is at the run");
        let id = ledger
            .create_task(&run, "x".into(), "t".into(), vec![], None, 100)
            .expect("a task");
        ledger
            .update_task(
                &run,
                &id,
                Some(TaskStatus::Completed),
                Some(result.to_string()),
                author,
                10,
            )
            .expect("done");
        (ledger, run, id)
    }

    fn desk_of(ledger: &Ledger) -> DeskSnapshot {
        desk::desk_snapshot(ledger, |_| false, |_, _| TaskCost::default())
    }

    /// A worker row of `run` carrying `task`, as the board lists it.
    fn agent_of(run: &str, task: &str) -> LedgerAgent {
        LedgerAgent {
            run: run.to_string(),
            task_id: task.to_string(),
            ..LedgerAgent::default()
        }
    }

    /// What the desk row of `id` wears, as (sentences, words, patterns).
    fn worn(desk: &DeskSnapshot, id: &str) -> Option<(u32, u32, u32)> {
        desk.tasks
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| row.writing.as_ref())
            .map(|found| (found.sentences, found.words, found.patterns))
    }

    /// One beat over the desk: open it, dress, close it.
    fn beat(memo: &mut LintMemo, ledger: &Ledger) -> DeskSnapshot {
        memo.begin();
        let mut desk = desk_of(ledger);
        dress_desk_with(memo, ledger, &mut desk);
        memo.end();
        desk
    }

    #[test]
    fn a_finished_task_wears_the_lint_of_its_workers_summary_and_it_is_worked_out_once() {
        let (ledger, _, id) = a_finished_task(&body(SUMMARY), worker());
        let mut memo = LintMemo::default();
        for at in 0..3 {
            assert_eq!(
                worn(&beat(&mut memo, &ledger), &id),
                Some((1, 2, 1)),
                "beat {at}: the finished row did not wear its summary's lint"
            );
        }
        assert_eq!(
            memo.worked(),
            1,
            "three beats over the same summary worked it out more than once"
        );
    }

    #[test]
    fn the_lint_rides_the_row_as_the_window_reads_it() {
        let (ledger, run, id) = a_finished_task(&body(SUMMARY), worker());
        let mut book = WritingBook::default();
        book.begin();
        let desk = book.dress_desk(&ledger, desk_of(&ledger));
        let agents = book.dress_agents(&ledger, vec![agent_of(&run, &id)]);
        book.end();
        let said = serde_json::to_value(&desk).expect("the desk serializes");
        let row = said["tasks"]
            .as_array()
            .and_then(|rows| rows.iter().find(|one| one["id"] == id.as_str()))
            .expect("the finished row");
        assert_eq!(row["writing"]["lang"], "ko", "{row}");
        assert_eq!(row["writing"]["sentences"], 1, "{row}");
        assert_eq!(row["writing"]["words"], 2, "{row}");
        assert_eq!(row["writing"]["patterns"], 1, "{row}");
        assert_eq!(row["writing"]["hits"][0]["find"], "Jev 자리", "{row}");
        assert_eq!(row["writing"]["hits"][0]["plain"], "Jev 기능", "{row}");
        let agent = serde_json::to_value(&agents[0]).expect("the worker row serializes");
        assert_eq!(agent["writing"]["words"], 2, "{agent}");
    }

    /// The task board's worker row and the desk's row ask by the same task, so
    /// one lint serves both; a row whose task is still moving wears nothing.
    #[test]
    fn a_finished_worker_row_wears_the_lint_the_desk_row_wears_and_a_moving_one_none() {
        let (mut ledger, run, id) = a_finished_task(&body(SUMMARY), worker());
        let moving = ledger
            .create_task(&run, "y".into(), "u".into(), vec![], None, 101)
            .expect("a second task");
        ledger
            .update_task(&run, &moving, None, Some(body(SUMMARY)), worker(), 11)
            .expect("a result with no ending");
        let mut memo = LintMemo::default();
        memo.begin();
        let mut desk = desk_of(&ledger);
        dress_desk_with(&mut memo, &ledger, &mut desk);
        let mut agents = vec![agent_of(&run, &id), agent_of(&run, &moving)];
        dress_agents_with(&mut memo, &ledger, &mut agents);
        memo.end();
        let counts = |row: &LedgerAgent| {
            row.writing
                .as_ref()
                .map(|found| (found.sentences, found.words, found.patterns))
        };
        assert_eq!(worn(&desk, &id), Some((1, 2, 1)));
        assert_eq!(counts(&agents[0]), Some((1, 2, 1)), "the finished row");
        assert_eq!(counts(&agents[1]), None, "a task still moving wore a lint");
        assert_eq!(
            memo.worked(),
            1,
            "the desk and the task board each worked the same summary out"
        );
    }

    /// Only what the worker wrote, only for a task that is finished.
    #[test]
    fn only_a_workers_own_summary_on_a_finished_task_is_linted() {
        let coordinator = ResultAuthor::Coordinator {
            seat: "team-1/%1".into(),
            generation: Some(1),
            attempt: None,
            source: None,
            completed_ms: None,
        };
        let mut memo = LintMemo::default();
        // The one summary that is read: a worker's own, on a finished task.
        let (ledger, _, id) = a_finished_task(&body(SUMMARY), worker());
        assert_eq!(
            worn(&beat(&mut memo, &ledger), &id),
            Some((1, 2, 1)),
            "the one summary that is read was not"
        );
        let mut check = |what: &str, result: &str, author: ResultAuthor| {
            let (ledger, _, id) = a_finished_task(result, author);
            assert_eq!(worn(&beat(&mut memo, &ledger), &id), None, "{what}");
        };
        check(
            "a coordinator's result is not the worker's writing",
            &body(SUMMARY),
            coordinator,
        );
        check("a report without prose", r#"{"ok":true}"#, worker());
        check(
            "a summary that is not text",
            r#"{"ok":true,"summary":3}"#,
            worker(),
        );

        // A task still moving wears nothing, whatever its result says.
        let mut ledger = Ledger::new();
        let run = ledger.create_run("writing", 1);
        ledger
            .start_worker(&run, "claude", ("team-writing", "%2"), None, 2)
            .expect("somebody is at the run");
        let id = ledger
            .create_task(&run, "x".into(), "t".into(), vec![], None, 100)
            .expect("a task");
        ledger
            .update_task(&run, &id, None, Some(body(SUMMARY)), worker(), 10)
            .expect("a result with no ending");
        assert_eq!(
            worn(&beat(&mut memo, &ledger), &id),
            None,
            "a task that is not finished wore a lint"
        );
    }

    /// A summary with nothing to count is not dressed; a lint nobody asked for
    /// in a beat is let go.
    #[test]
    fn a_lint_nobody_asked_for_in_a_beat_is_let_go() {
        let (ledger, _, id) = a_finished_task(&body(SUMMARY), worker());
        let mut memo = LintMemo::default();
        assert_eq!(worn(&beat(&mut memo, &ledger), &id), Some((1, 2, 1)));
        assert_eq!(memo.len(), 1);
        // The task is gone from the next beat's desk: nothing asks for it.
        memo.begin();
        dress_desk_with(&mut memo, &Ledger::new(), &mut DeskSnapshot::default());
        memo.end();
        assert_eq!(memo.len(), 0, "a lint nobody asked for stayed in the book");

        let (plain, _, plain_id) = a_finished_task(&body("1234 5678"), worker());
        assert_eq!(
            worn(&beat(&mut memo, &plain), &plain_id),
            None,
            "a text with no words wore a lint"
        );
    }

    /// A ledger of finished tasks the desk carries only part of: the book holds
    /// the rows the desk sends — a stage's cap — and no more, however many beats
    /// ask and however the tasks churn.
    #[test]
    fn the_book_holds_only_the_rows_the_desk_sends_over_a_long_run() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("writing", 1);
        ledger
            .start_worker(&run, "claude", ("team-writing", "%2"), None, 2)
            .expect("somebody is at the run");
        let finished = desk::STAGE_ROWS + 16;
        for at in 0..finished {
            let id = ledger
                .create_task(
                    &run,
                    "x".into(),
                    format!("t{at}"),
                    vec![],
                    None,
                    100 + at as i64,
                )
                .expect("a task");
            ledger
                .update_task(
                    &run,
                    &id,
                    Some(TaskStatus::Completed),
                    Some(body(&format!("{SUMMARY} 번호 {at}."))),
                    worker(),
                    10,
                )
                .expect("done");
        }
        let mut memo = LintMemo::default();
        for _ in 0..200 {
            beat(&mut memo, &ledger);
            assert!(
                memo.len() <= desk::STAGE_ROWS,
                "the book grew past the rows the desk sends: {}",
                memo.len()
            );
        }
        assert_eq!(memo.len(), desk::STAGE_ROWS);
        assert_eq!(
            memo.worked(),
            desk::STAGE_ROWS,
            "two hundred beats over the same summaries worked something out twice"
        );
    }

    /// The cost of a beat that asks about a full stage of finished rows, cold
    /// (every summary worked out) and warm (every summary remembered), and the
    /// resident memory around two thousand beats. A measurement, run on purpose:
    /// `cargo test -p zerocode-shell --bin zerocode-shell writing_book::tests::the_cost
    /// -- --ignored --nocapture`, on the normal profile and under `taskpolicy -b`.
    #[test]
    #[ignore = "a measurement, run on purpose"]
    fn the_cost_of_a_beat_over_a_full_stage_of_finished_rows() {
        const BEATS: usize = 2_000;
        let mut ledger = Ledger::new();
        let run = ledger.create_run("writing", 1);
        ledger
            .start_worker(&run, "claude", ("team-writing", "%2"), None, 2)
            .expect("somebody is at the run");
        // A summary of about 600 bytes: what a worker writes when it is asked to be short.
        let summary = SUMMARY.repeat(3);
        for at in 0..desk::STAGE_ROWS {
            let id = ledger
                .create_task(
                    &run,
                    "x".into(),
                    format!("t{at}"),
                    vec![],
                    None,
                    100 + at as i64,
                )
                .expect("a task");
            ledger
                .update_task(
                    &run,
                    &id,
                    Some(TaskStatus::Completed),
                    Some(body(&format!("{summary} 번호 {at}."))),
                    worker(),
                    10,
                )
                .expect("done");
        }
        let micros = |from: std::time::Instant| from.elapsed().as_micros();
        let started = std::time::Instant::now();
        for _ in 0..BEATS {
            std::hint::black_box(desk_of(&ledger));
        }
        let bare_us = micros(started) / BEATS as u128;
        let bare = desk_of(&ledger);
        // What a beat pays for the copy of the desk alone, so the book's own part
        // is the warm beat less this.
        let started = std::time::Instant::now();
        for _ in 0..BEATS {
            std::hint::black_box(bare.clone());
        }
        let clone_us = micros(started) / BEATS as u128;
        let mut memo = LintMemo::default();
        let started = std::time::Instant::now();
        memo.begin();
        let mut cold = bare.clone();
        dress_desk_with(&mut memo, &ledger, &mut cold);
        memo.end();
        let cold_us = micros(started);
        let rss_before = resident_kb();
        let started = std::time::Instant::now();
        for _ in 0..BEATS {
            memo.begin();
            let mut desk = std::hint::black_box(bare.clone());
            dress_desk_with(&mut memo, &ledger, &mut desk);
            memo.end();
        }
        let warm_us = micros(started) / BEATS as u128;
        let rss_after = resident_kb();
        assert_eq!(memo.worked(), desk::STAGE_ROWS);
        eprintln!(
            "WRITING_BOOK_NUMBERS rows={} summary_bytes={} beats={BEATS} desk_snapshot_us={bare_us} desk_clone_us={clone_us} cold_dress_us={cold_us} warm_beat_us={warm_us} warm_book_us={} rss_kb_before={rss_before} rss_kb_after={rss_after} held={}",
            desk::STAGE_ROWS,
            summary.len(),
            warm_us.saturating_sub(clone_us),
            memo.len()
        );
    }

    /// This process's resident memory in KB, from `ps` — a measurement helper,
    /// never part of the shipped code. The child starts through the window's one
    /// door ([`crate::proc::quiet_command`]), as every child in this crate does.
    fn resident_kb() -> u64 {
        crate::proc::quiet_command("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }
}
