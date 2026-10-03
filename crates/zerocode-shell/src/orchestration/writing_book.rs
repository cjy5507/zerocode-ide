//! The window's half of the writing badge on a finished task (t-32786): the
//! summary its own worker handed in, linted once and remembered, so a beat
//! that moved nothing works out nothing.
//!
//! The rule is the core's ([`zerocode_core::plain_text::lint`]): a pure
//! function of the text that counts and never refuses. This is where the
//! counts are held. Nothing is stored in the ledger — the summary is already
//! there, and a count worked out from it again is the same count.
//!
//! This is the red skeleton: the memo is held and nothing is dressed yet.

use std::sync::{LazyLock, Mutex, PoisonError};

use zerocode_core::orchestration::Ledger;
use zerocode_core::plain_text::LintMemo;

use super::desk::DeskSnapshot;

static BOOK: LazyLock<Mutex<LintMemo>> = LazyLock::new(Mutex::default);

/// The desk, with the lint of the summary on each finished row whose worker
/// wrote one.
pub(crate) fn dressed(ledger: &Ledger, mut desk: DeskSnapshot) -> DeskSnapshot {
    let mut memo = BOOK.lock().unwrap_or_else(PoisonError::into_inner);
    dress_with(&mut memo, ledger, &mut desk);
    desk
}

fn dress_with(_memo: &mut LintMemo, _ledger: &Ledger, _desk: &mut DeskSnapshot) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::desk;
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
    /// `author` wrote.
    fn a_finished_task(result: &str, author: ResultAuthor) -> (Ledger, String) {
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
        (ledger, id)
    }

    fn desk_of(ledger: &Ledger) -> DeskSnapshot {
        desk::desk_snapshot(ledger, |_| false, |_, _| TaskCost::default())
    }

    /// What the row of `id` wears, as (sentences, words, patterns).
    fn worn(desk: &DeskSnapshot, id: &str) -> Option<(u32, u32, u32)> {
        desk.tasks
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| row.writing.as_ref())
            .map(|found| (found.sentences, found.words, found.patterns))
    }

    #[test]
    fn a_finished_task_wears_the_lint_of_its_workers_summary_and_it_is_worked_out_once() {
        let (ledger, id) = a_finished_task(&body(SUMMARY), worker());
        let mut memo = LintMemo::default();
        for beat in 0..3 {
            let mut desk = desk_of(&ledger);
            dress_with(&mut memo, &ledger, &mut desk);
            assert_eq!(
                worn(&desk, &id),
                Some((1, 2, 1)),
                "beat {beat}: the finished row did not wear its summary's lint"
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
        let (ledger, id) = a_finished_task(&body(SUMMARY), worker());
        let desk = dressed(&ledger, desk_of(&ledger));
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
        let (ledger, id) = a_finished_task(&body(SUMMARY), worker());
        let mut desk = desk_of(&ledger);
        dress_with(&mut memo, &ledger, &mut desk);
        assert_eq!(
            worn(&desk, &id),
            Some((1, 2, 1)),
            "the one summary that is read was not"
        );
        let mut check = |what: &str, result: &str, author: ResultAuthor| {
            let (ledger, id) = a_finished_task(result, author);
            let mut desk = desk_of(&ledger);
            dress_with(&mut memo, &ledger, &mut desk);
            assert_eq!(worn(&desk, &id), None, "{what}");
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
        let mut desk = desk_of(&ledger);
        dress_with(&mut memo, &ledger, &mut desk);
        assert_eq!(
            worn(&desk, &id),
            None,
            "a task that is not finished wore a lint"
        );
    }

    /// A summary with nothing to count is not dressed; a lint nobody asked for
    /// in a beat is let go.
    #[test]
    fn a_lint_nobody_asked_for_in_a_beat_is_let_go() {
        let (ledger, id) = a_finished_task(&body(SUMMARY), worker());
        let mut memo = LintMemo::default();
        let mut desk = desk_of(&ledger);
        dress_with(&mut memo, &ledger, &mut desk);
        assert_eq!(worn(&desk, &id), Some((1, 2, 1)));
        assert_eq!(memo.len(), 1);
        // The task is gone from the next beat's desk: nothing asks for it.
        let mut empty = DeskSnapshot::default();
        dress_with(&mut memo, &Ledger::new(), &mut empty);
        assert_eq!(memo.len(), 0, "a lint nobody asked for stayed in the book");

        let (plain, plain_id) = a_finished_task(&body("1234 5678"), worker());
        let mut desk = desk_of(&plain);
        dress_with(&mut memo, &plain, &mut desk);
        assert_eq!(
            worn(&desk, &plain_id),
            None,
            "a text with no words wore a lint"
        );
    }
}
