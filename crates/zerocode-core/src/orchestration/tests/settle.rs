//! t-71220: the person's settle — which work nobody is coming back to
//! (`left_behind`), and the one press that clears it: closing what has nothing
//! on record, keeping what does, waiving the review of finished work, and
//! putting a settle back.

use super::*;
use crate::orchestration::land_batch::PressedBy;
use crate::orchestration::settle::{
    KeptWhy, Left, Named, PRESS_TASKS_MAX, REVIEW_WAIVED_KEY, SETTLED_WHY, SettlePress,
    left_behind, waive_record,
};

const QUIET: i64 = SETTLE_QUIET_DAYS * 86_400_000;

fn task_of(bench: &mut Bench, spec: &str) -> String {
    bench.json(&format!("task-create --spec {spec}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string()
}

/// A task with one attempt that ended without handing anything in.
fn task_with_an_empty_attempt(bench: &mut Bench, spec: &str) -> String {
    let id = task_of(bench, spec);
    let (worker, _pane) = bench.seat(&format!("worker-start --agent codex --task {id}"));
    bench
        .ledger
        .end_attempt(&worker, Ending::Stopped, "lost", bench.clock)
        .expect("the attempt ends");
    id
}

/// A task a worker carried and handed `head` in on: completed, no review.
fn handed_in(bench: &mut Bench, spec: &str, head: &str) -> String {
    let id = task_of(bench, spec);
    let (_worker, pane) = bench.seat(&format!("worker-start --agent claude --task {id}"));
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{head}\"}}"),
    );
    id
}

/// The run's coordinator left: its seat is vacated, as a leader's exit leaves it.
fn leave_the_seat(bench: &mut Bench) {
    let at = bench.clock;
    bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .expect("the run has a seat")
        .vacated_ms = Some(at);
}

fn reading(bench: &Bench) -> std::collections::BTreeMap<String, Left> {
    left_behind(&bench.ledger.runs()[0], bench.clock)
        .into_iter()
        .map(|(task, left)| (task.to_string(), left))
        .collect()
}

fn named(bench: &Bench, tasks: &[&String]) -> Vec<Named> {
    let run = bench.ledger.runs()[0].id.clone();
    tasks
        .iter()
        .map(|task| Named {
            run: run.clone(),
            task: (*task).clone(),
            was: None,
        })
        .collect()
}

fn status(bench: &Bench, task: &str) -> TaskStatus {
    bench.ledger.runs()[0].task(task).expect("the task").status
}

/// In a run whose coordinator sits, a task touched within the quiet week is
/// that coordinator's turn and an older one is left — never started, or every
/// attempt ended handing nothing in. A task an attempt still carries is
/// somebody's whoever sits. In a run nobody sits in, every open task is left.
#[test]
fn a_seated_run_keeps_its_week_and_an_empty_seat_keeps_nothing_open() {
    let mut bench = Bench::new();
    bench.json("run-create --name week");
    let never = task_of(&mut bench, "never");
    let empty = task_with_an_empty_attempt(&mut bench, "empty");
    bench.clock += QUIET + 1;
    let fresh = task_of(&mut bench, "fresh");
    let carried = task_of(&mut bench, "carried");
    bench.seat(&format!("worker-start --agent codex --task {carried}"));

    let read = reading(&bench);
    assert_eq!(read.get(&never), Some(&Left::Settle), "{read:?}");
    assert_eq!(read.get(&empty), Some(&Left::Settle), "{read:?}");
    assert_eq!(read.get(&fresh), None, "the coordinator's own week");
    assert_eq!(read.get(&carried), None, "a live attempt carries it");

    leave_the_seat(&mut bench);
    let read = reading(&bench);
    assert_eq!(
        read.get(&fresh),
        Some(&Left::Settle),
        "nobody sits to come back to it"
    );
    assert_eq!(read.get(&carried), None, "its worker is still at it");
}

/// Work on record stays open for a look: a task something was handed in on and
/// that was put back to work. Finished work no review stands for waits in a
/// pile of its own, and a standing decision is the person's to answer, so it
/// is no pile's.
#[test]
fn work_on_record_is_kept_for_a_look_and_finished_work_waits_apart() {
    let mut bench = Bench::new();
    bench.json("run-create --name record");
    let back = handed_in(&mut bench, "back", "aaaaaaa1111111");
    bench.json(&format!("task-update --task {back} --status ready"));
    let finished = handed_in(&mut bench, "finished", "bbbbbbb2222222");
    let asked = task_of(&mut bench, "asked");
    bench.json(&format!(
        "gate-create --task {asked} --question which --options [\"a\",\"b\"]"
    ));
    leave_the_seat(&mut bench);

    let read = reading(&bench);
    assert_eq!(read.get(&back), Some(&Left::Look), "{read:?}");
    assert_eq!(read.get(&finished), Some(&Left::Unreviewed), "{read:?}");
    assert_eq!(read.get(&asked), None, "{read:?}");
}

/// A settle never closes a task from under one that waits on it: a task a
/// current one depends on is current with it, and two tasks left together go
/// together.
#[test]
fn a_task_a_current_one_waits_on_is_current_with_it() {
    let mut bench = Bench::new();
    bench.json("run-create --name deps");
    let first = task_of(&mut bench, "first");
    let chained = task_of(&mut bench, "chained");
    bench.clock += QUIET + 1;
    let waiting = bench.json(&format!("task-create --spec waiting --deps {first}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let read = reading(&bench);
    assert_eq!(read.get(&first), None, "a fresh task waits on it: {read:?}");
    assert_eq!(read.get(&waiting), None);
    assert_eq!(read.get(&chained), Some(&Left::Settle));

    bench.clock += QUIET + 1;
    let read = reading(&bench);
    assert_eq!(
        (read.get(&first), read.get(&waiting)),
        (Some(&Left::Settle), Some(&Left::Settle)),
        "left together, they go together"
    );
}

/// The person's settle closes `outdated` what the ledger still reads as left
/// with nothing on record, answers every other task it names as kept with the
/// reason, and is undone by putting what it closed back where it stood. It is
/// the person's press alone, and a press the ledger does not take writes
/// nothing.
#[test]
fn the_persons_settle_closes_what_is_left_and_an_undo_puts_it_back() {
    let mut bench = Bench::new();
    bench.json("run-create --name settle");
    let never = task_of(&mut bench, "never");
    let back = handed_in(&mut bench, "back", "aaaaaaa1111111");
    bench.json(&format!("task-update --task {back} --status ready"));
    bench.clock += QUIET + 1;
    let fresh = task_of(&mut bench, "fresh");
    let pressed = named(&bench, &[&never, &back, &fresh]);
    let before = bench.ledger.export();

    // Not the person: refused whole.
    let coordinator = PressedBy::Coordinator {
        run: pressed[0].run.clone(),
        seat: "team-1/%1".into(),
        generation: 1,
    };
    for by in [coordinator, PressedBy::Ledger] {
        let said = bench
            .ledger
            .person_settle(SettlePress::Settle, &pressed, &by, bench.clock)
            .expect_err("only the person presses a settle");
        assert!(said.contains("person"), "{said}");
    }
    // Nothing named, too much named, a run the ledger does not hold.
    assert!(
        bench
            .ledger
            .person_settle(SettlePress::Settle, &[], &PressedBy::Person, bench.clock)
            .is_err()
    );
    let flood = vec![pressed[0].clone(); PRESS_TASKS_MAX + 1];
    assert!(
        bench
            .ledger
            .person_settle(SettlePress::Settle, &flood, &PressedBy::Person, bench.clock)
            .is_err()
    );
    let stranger = [Named {
        run: "run-nobody".into(),
        ..pressed[0].clone()
    }];
    assert!(
        bench
            .ledger
            .person_settle(
                SettlePress::Settle,
                &stranger,
                &PressedBy::Person,
                bench.clock
            )
            .is_err()
    );
    assert_eq!(
        bench.ledger.export(),
        before,
        "a refused press wrote nothing"
    );

    let settled = bench
        .ledger
        .person_settle(
            SettlePress::Settle,
            &pressed,
            &PressedBy::Person,
            bench.clock,
        )
        .expect("the person's settle");
    assert_eq!(
        settled.closed,
        [Named {
            was: Some(TaskStatus::Ready),
            ..pressed[0].clone()
        }]
    );
    let kept: Vec<(&str, KeptWhy)> = settled
        .kept
        .iter()
        .map(|one| (one.task.as_str(), one.why))
        .collect();
    assert_eq!(
        kept,
        [
            (back.as_str(), KeptWhy::Look),
            (fresh.as_str(), KeptWhy::Moved)
        ]
    );
    assert!(settled.moved());
    let task = bench.ledger.runs()[0].task(&never).expect("the task");
    assert_eq!(task.status, TaskStatus::Closed);
    assert_eq!(
        task.closed,
        Some(Closure::Outdated {
            why: SETTLED_WHY.into()
        })
    );
    assert_eq!(status(&bench, &back), TaskStatus::Ready, "kept open");
    assert!(
        bench
            .ledger
            .person_settle(
                SettlePress::Settle,
                &pressed,
                &PressedBy::Person,
                bench.clock
            )
            .expect("a second press")
            .closed
            .is_empty(),
        "a second press finds nothing left to close"
    );

    // The undo names what the settle answered, and puts it back where it stood.
    let undone = bench
        .ledger
        .person_settle(
            SettlePress::Undo,
            &settled.closed,
            &PressedBy::Person,
            bench.clock,
        )
        .expect("the undo");
    assert_eq!(undone.reopened, settled.closed);
    let task = bench.ledger.runs()[0].task(&never).expect("the task");
    assert_eq!(
        (task.status, task.closed.as_ref()),
        (TaskStatus::Ready, None)
    );
    let again = bench
        .ledger
        .person_settle(
            SettlePress::Undo,
            &settled.closed,
            &PressedBy::Person,
            bench.clock,
        )
        .expect("a second undo");
    assert!(again.reopened.is_empty() && !again.moved());
    assert_eq!(again.kept[0].why, KeptWhy::Moved);

    // An undo opens nothing it did not close: a task the coordinator's own
    // settle pass closed, and a status no settle closes from.
    let empty = task_with_an_empty_attempt(&mut bench, "empty");
    bench.clock += QUIET + 1;
    bench.json("task-settle --apply");
    assert_eq!(status(&bench, &empty), TaskStatus::Closed);
    let not_ours = [Named {
        was: Some(TaskStatus::Ready),
        ..named(&bench, &[&empty])[0].clone()
    }];
    let undone = bench
        .ledger
        .person_settle(
            SettlePress::Undo,
            &not_ours,
            &PressedBy::Person,
            bench.clock,
        )
        .expect("the undo answers");
    assert!(undone.reopened.is_empty(), "{undone:?}");
    assert_eq!(status(&bench, &empty), TaskStatus::Closed);
}

/// 검토 생략(사람) is the person's record and never a review: it moves finished
/// work out of 검토 대기 for good, says no key a coordinator writes, binds to
/// the attempt and the hand-in it was written against, and is nobody else's to
/// write — the same key in a worker's words is nothing.
#[test]
fn a_waiver_is_the_persons_record_and_never_a_review() {
    let mut bench = Bench::new();
    bench.json("run-create --name waive");
    let finished = handed_in(&mut bench, "finished", "aaaaaaa1111111");
    let fresh = handed_in(&mut bench, "fresh", "bbbbbbb2222222");
    bench.clock += QUIET + 1;
    // Touched again: the newest hand-in is the coordinator's week.
    bench.json(&format!("task-update --task {fresh} --status ready"));
    bench.seat(&format!("worker-start --agent claude --task {fresh}"));
    let read = reading(&bench);
    assert_eq!(read.get(&finished), Some(&Left::Unreviewed), "{read:?}");

    let pressed = named(&bench, &[&finished, &fresh]);
    let waived = bench
        .ledger
        .person_settle(
            SettlePress::Waive,
            &pressed,
            &PressedBy::Person,
            bench.clock,
        )
        .expect("the waiver");
    assert_eq!(waived.waived, pressed[..1]);
    assert_eq!(waived.kept[0].task, fresh);

    let run = &bench.ledger.runs()[0];
    let task = run.task(&finished).expect("the task");
    assert!(matches!(
        task.result_author,
        Some(ResultAuthor::Person { .. })
    ));
    let review = run.review_of(task);
    assert!(review.waived, "{review:?}");
    assert_eq!(review.author, ReviewAuthor::Person);
    assert!(
        !review.verified && !review.merged && !review.deployed && !review.written,
        "a waiver is no review: {review:?}"
    );
    assert_eq!(run.progress_of(task).stage, WorkStage::Waived);
    assert_eq!(task.status, TaskStatus::Completed, "done is done");
    assert_eq!(reading(&bench).get(&finished), None, "out of every pile");
    // The record comes back from the store's own round trip, as every row does.
    let carried = Ledger::rebuild(bench.ledger.export()).expect("a readable ledger");
    let task = carried.runs()[0].task(&finished).expect("the task");
    assert!(matches!(
        task.result_author,
        Some(ResultAuthor::Person { .. })
    ));
    assert!(carried.runs()[0].review_of(task).waived);

    // The same key in a worker's words, or nobody's, waives nothing.
    for author in [
        Some(ResultAuthor::Worker {
            worker: "w-1".into(),
            dispatch: None,
        }),
        None,
    ] {
        assert!(!ReviewFacts::written_by(&waive_record(), author.as_ref()).waived);
    }
    assert!(waive_record().contains(REVIEW_WAIVED_KEY));

    // A new hand-in is not waived by the old record.
    bench.json(&format!("task-update --task {finished} --status ready"));
    let (_worker, pane) = bench.seat(&format!("worker-start --agent claude --task {finished}"));
    bench.json_at(
        &pane,
        "send --type worker_done --body {\"ok\":true,\"head\":\"ccccccc3333333\"}",
    );
    let run = &bench.ledger.runs()[0];
    let task = run.task(&finished).expect("the task");
    assert!(!run.review_of(task).waived);
    assert_eq!(run.progress_of(task).stage, WorkStage::ReportOnly);
}
