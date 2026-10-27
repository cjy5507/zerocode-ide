//! The person's settle (t-71220): work that reads as going on now and that
//! nobody is coming back to, sorted out once and cleared by one press.
//!
//! The person found weeks-old tasks under 「지금」 in the artifact tab's flow —
//! ready work in runs whose coordinator seat had long been left, and finished
//! work no review was ever written for — and nothing that would ever close
//! them: `task-settle --apply` is the coordinator seat's verb, and those runs
//! have nobody in it. This file is the one reading of which task is left
//! behind ([`left_behind`]), which the desk, the board and the flow all draw,
//! and the one road a person's press takes to clear it
//! ([`Ledger::person_settle`]):
//!
//! - an open task nobody is coming back to, with nothing on record, is closed
//!   `outdated` with [`SETTLED_WHY`] — the closing `task-settle --apply`
//!   writes, and like it never a failure;
//! - one with work on record — something handed in, an attempt still open, an
//!   open task a settle keeps waiting on it — stays open for a person to look
//!   at;
//! - finished work no review was written for takes 검토 생략(사람)
//!   ([`ResultAuthor::Person`]): a waiver, never a review;
//! - a settle is undone by putting what it closed back where it stood.
//!
//! The press is the person's alone: [`PressedBy::Person`] comes from the
//! window's board command and nowhere else ([`land_batch::person_press`]), and
//! no verb, worker or request body reaches this road.

use std::collections::HashMap;

use super::coordinator_take::carrier_generation;
use super::land_batch::PressedBy;
use super::*;

/// How long a task in a run whose coordinator seat is live may sit untouched
/// and still be that coordinator's to come back to: the settle pass's own
/// quiet time ([`SETTLE_QUIET_DAYS`]), so the person and the coordinator call
/// the same work old.
const QUIET_MS: i64 = SETTLE_QUIET_DAYS * DAY_MS;

/// The reason a person's settle closes a task with — the line the board reads
/// under 닫힘, and the mark an undo knows the settle's own closings by.
pub const SETTLED_WHY: &str = "settled by the person: nobody was coming back to it";

/// The key 검토 생략(사람) is written under, in the person's record.
pub const REVIEW_WAIVED_KEY: &str = "reviewWaived";

/// How many tasks one press may name: one list's worth ([`MAX_LIST`]). A pile
/// past it is cleared a list at a time.
pub const PRESS_TASKS_MAX: usize = MAX_LIST;

/// What a settle does with a task nobody is coming back to — the pile a person
/// reads it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Left {
    /// Open, with nothing on record: a settle closes it `outdated`. 정리 필요.
    Settle,
    /// Open, with work on record — handed in, carried by an attempt whose
    /// worker is gone, or waited on by an open task that stays — so it stays
    /// open for a person to look at. 정리 필요 · 검토 필요.
    Look,
    /// Completed, handed in, and no review stands for it: 검토 없이 끝남. A
    /// waiver writes 검토 생략(사람) ([`waive_record`]).
    Unreviewed,
}

impl Left {
    /// Every pile, each once.
    pub const ALL: [Self; 3] = [Self::Settle, Self::Look, Self::Unreviewed];

    /// The pile's name on the wire — the key every window table is read by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Settle => "settle",
            Self::Look => "look",
            Self::Unreviewed => "unreviewed",
        }
    }
}

/// Every task of `run` that is left behind at `now_ms`, and the pile it is in.
/// A task not named is somebody's, or over some other way, and stands where
/// its stage puts it.
///
/// Somebody's is: carried by an attempt whose worker is still summoned; held
/// by a decision nobody has made yet (the board puts that in front of the
/// person as it is); or, in a run whose coordinator seat is live, touched —
/// written, or an attempt begun or ended — within the quiet time. Everything
/// else that is open is left: closed by a settle when nothing is on record,
/// kept for a look when something is. A completed task waiting on a review
/// that nobody is coming to write is left the same way.
///
/// A settle does not close a task from under one that waits on it: a task an
/// open task depends on takes that task's reading — somebody's when it is
/// somebody's, kept for a look when it is kept — walked until nothing moves,
/// since keeping one task keeps what it waits on. Two tasks left together go
/// together.
///
/// Finished work is read by the ledger's own reading of where it stands
/// ([`Run::progress_of`], [`awaits_review`]); a caller that already read each
/// task's stage passes what it read instead ([`left_behind_by`]).
#[must_use]
pub fn left_behind(run: &Run, now_ms: i64) -> HashMap<&str, Left> {
    left_behind_by(run, now_ms, |task| {
        awaits_review(run.progress_of(task).stage)
    })
}

/// Whether finished work at `stage` waits on a review nobody has written —
/// handed in and not reviewed, or reviewed against older work. The one
/// statement of it: the press, the desk and the flow all ask it.
#[must_use]
pub const fn awaits_review(stage: WorkStage) -> bool {
    matches!(stage, WorkStage::ReportOnly | WorkStage::Stale)
}

/// [`left_behind`] over a reading the caller already made: `awaits` answers
/// for a completed task whether its work waits on a review — the stage the
/// caller read for it, asked through [`awaits_review`] — so no stage is read
/// twice.
#[must_use]
pub fn left_behind_by(
    run: &Run,
    now_ms: i64,
    awaits: impl Fn(&Task) -> bool,
) -> HashMap<&str, Left> {
    let seated = run.coordinator_live().map(|seat| seat.generation);
    let live = seated.is_some();
    let owned = |task: &Task| live && now_ms.saturating_sub(last_touched_ms(run, task)) < QUIET_MS;
    let mut left: HashMap<&str, Left> = HashMap::new();
    for task in &run.tasks {
        let reading = match task.status {
            TaskStatus::Pending
            | TaskStatus::Ready
            | TaskStatus::Blocked
            | TaskStatus::Dispatched => {
                let mut open = run
                    .dispatches
                    .iter()
                    .filter(|held| held.task == task.id && held.is_open())
                    .peekable();
                if open.peek().is_some() {
                    // A live worker carries its attempt, and so does the live
                    // seat its own (`task-take`); an earlier sitting's attempt
                    // is nobody's until the next take ends it.
                    let carried = open.any(|held| {
                        run.worker(&held.worker)
                            .is_some_and(|worker| worker.state.still_summoned())
                            || carrier_generation(&held.worker)
                                .is_some_and(|generation| seated == Some(generation))
                    });
                    (!carried).then_some(Left::Look)
                } else if task.status == TaskStatus::Dispatched {
                    // Carried by no attempt at all: the record is broken, and a
                    // person looks before anything closes it.
                    Some(Left::Look)
                } else if run.pending_gate_on(&task.id).is_some() || owned(task) {
                    None
                } else if on_record(run, task) {
                    Some(Left::Look)
                } else {
                    Some(Left::Settle)
                }
            }
            TaskStatus::Completed => (!owned(task) && awaits(task)).then_some(Left::Unreviewed),
            TaskStatus::Failed | TaskStatus::Closed => None,
        };
        if let Some(reading) = reading {
            left.insert(task.id.as_str(), reading);
        }
    }
    loop {
        let mut moved = false;
        for waiter in run.tasks.iter().filter(|task| !task.status.is_final()) {
            let kept = match left.get(waiter.id.as_str()).copied() {
                None => None,
                Some(Left::Look) => Some(Left::Look),
                Some(Left::Settle | Left::Unreviewed) => continue,
            };
            for dep in &waiter.deps {
                if left.get(dep.as_str()) != Some(&Left::Settle) {
                    continue;
                }
                match kept {
                    None => left.remove(dep.as_str()),
                    Some(look) => left.insert(dep.as_str(), look),
                };
                moved = true;
            }
        }
        if !moved {
            return left;
        }
    }
}

/// Whether anything a person would read before closing a task is on record: a
/// hand-in on any of its attempts, or a review a coordinator wrote. The
/// reading [`Run::handed_nothing_in`] makes, without its need for an attempt —
/// work nobody ever started has nothing on record either.
fn on_record(run: &Run, task: &Task) -> bool {
    run.dispatches
        .iter()
        .any(|held| held.task == task.id && held.source.is_some())
        || ReviewFacts::written_by(task.result.as_str(), task.result_author.as_ref()).written
}

/// The person's record of 검토 생략(사람) — the whole of the result the waiver
/// writes. Who wrote it ([`ResultAuthor::Person`]) is what makes it the
/// person's; this key is what it says.
#[must_use]
pub fn waive_record() -> String {
    serde_json::json!({ REVIEW_WAIVED_KEY: true }).to_string()
}

/// Whether a result the person wrote says 검토 생략 ([`waive_record`]).
#[must_use]
pub fn waived_in(result: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|said| said.get(REVIEW_WAIVED_KEY)?.as_bool())
        == Some(true)
}

/// What a person pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlePress {
    /// Close what a settle closes ([`Left::Settle`]) `outdated`.
    Settle,
    /// Write 검토 생략(사람) on finished work no review stands for
    /// ([`Left::Unreviewed`]).
    Waive,
    /// Put what a settle closed back where it stood — the press the window
    /// offers for a short while after a settle.
    Undo,
}

/// One task a press names, as the window drew it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Named {
    pub run: String,
    pub task: String,
    /// The status a settle closed it from, as that settle answered
    /// ([`Settled::closed`]) — what an undo names, and nothing else does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<TaskStatus>,
}

/// Why a press left a task it named as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeptWhy {
    /// Work on record keeps it open ([`Left::Look`]).
    Look,
    /// The ledger's reading is not the one the press was drawn over any more:
    /// somebody took it, it moved on or it is closed — or an undo named a task
    /// this settle did not close.
    Moved,
}

/// A task a press named and left as it was.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Kept {
    pub run: String,
    pub task: String,
    pub why: KeptWhy,
}

/// What one press did, task by task — all the window says of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Settled {
    /// Closed `outdated`, each with the status it was closed from.
    pub closed: Vec<Named>,
    /// 검토 생략(사람) written.
    pub waived: Vec<Named>,
    /// Back where a settle had closed it from.
    pub reopened: Vec<Named>,
    pub kept: Vec<Kept>,
}

impl Settled {
    /// Whether the press wrote anything.
    #[must_use]
    pub fn moved(&self) -> bool {
        !(self.closed.is_empty() && self.waived.is_empty() && self.reopened.is_empty())
    }
}

impl Ledger {
    /// A person's press over what is left behind: the tasks the window drew,
    /// each acted on only where the ledger's own reading at this moment still
    /// agrees ([`left_behind`], read once per run before anything moves), and
    /// every other one answered as kept with its reason. Refused whole — with
    /// nothing written — unless `by` is the person, the list is one the
    /// window could draw, and every run it names is held.
    pub fn person_settle(
        &mut self,
        press: SettlePress,
        named: &[Named],
        by: &PressedBy,
        now_ms: i64,
    ) -> Result<Settled, String> {
        if *by != PressedBy::Person {
            return Err("a settle is the person's press, through the window's own door".into());
        }
        if named.is_empty() {
            return Err("a press names the tasks it was drawn over".into());
        }
        if named.len() > PRESS_TASKS_MAX {
            return Err(format!(
                "a press names at most {PRESS_TASKS_MAX} tasks; {} were named",
                named.len()
            ));
        }
        let mut readings: HashMap<&str, HashMap<String, Left>> = HashMap::new();
        for one in named {
            if readings.contains_key(one.run.as_str()) {
                continue;
            }
            let run = self.run(&one.run).ok_or_else(|| unknown_run(&one.run))?;
            let reading = left_behind(run, now_ms)
                .into_iter()
                .map(|(task, left)| (task.to_string(), left))
                .collect();
            readings.insert(one.run.as_str(), reading);
        }
        let mut settled = Settled::default();
        for one in named {
            let left = readings
                .get(one.run.as_str())
                .and_then(|reading| reading.get(&one.task))
                .copied();
            let kept = |why| Kept {
                run: one.run.clone(),
                task: one.task.clone(),
                why,
            };
            match (press, left) {
                (SettlePress::Settle, Some(Left::Settle)) => {
                    let was = self
                        .run(&one.run)
                        .and_then(|run| run.task(&one.task))
                        .map(|task| task.status);
                    self.close_task(
                        &one.run,
                        &one.task,
                        Closure::Outdated {
                            why: SETTLED_WHY.into(),
                        },
                    )?;
                    settled.closed.push(Named { was, ..one.clone() });
                }
                (SettlePress::Settle, Some(Left::Look)) => settled.kept.push(kept(KeptWhy::Look)),
                (SettlePress::Waive, Some(Left::Unreviewed)) => {
                    self.waive_review(&one.run, &one.task, now_ms)?;
                    settled.waived.push(Named {
                        was: None,
                        ..one.clone()
                    });
                }
                (SettlePress::Undo, _) => {
                    if self.reopen_settled(one, now_ms)? {
                        settled.reopened.push(one.clone());
                    } else {
                        settled.kept.push(kept(KeptWhy::Moved));
                    }
                }
                _ => settled.kept.push(kept(KeptWhy::Moved)),
            }
        }
        Ok(settled)
    }

    /// 검토 생략(사람) on one task: the person's record, bound to the newest
    /// attempt and what it handed in, as a coordinator's review is bound.
    fn waive_review(&mut self, run_id: &str, task_id: &str, now_ms: i64) -> Result<(), String> {
        let run = self.run(run_id).ok_or_else(|| unknown_run(run_id))?;
        let newest = run.newest_attempt(task_id);
        let author = ResultAuthor::Person {
            attempt: newest.map(|attempt| attempt.id.clone()),
            source: newest.and_then(|attempt| attempt.source.clone()),
        };
        self.update_task(run_id, task_id, None, Some(waive_record()), author, now_ms)?;
        Ok(())
    }

    /// Put one task a person's settle closed back where it stood: only a task
    /// still closed `outdated` with [`SETTLED_WHY`], and only to the open
    /// status the settle answered it was closed from. Anything else — closed
    /// another way, already back at work, named with a status no settle
    /// closes from — is left as it is, and answered `false`.
    fn reopen_settled(&mut self, one: &Named, now_ms: i64) -> Result<bool, String> {
        let Some(was) = one.was.filter(|was| {
            matches!(
                was,
                TaskStatus::Pending | TaskStatus::Ready | TaskStatus::Blocked
            )
        }) else {
            return Ok(false);
        };
        let run = self.run(&one.run).ok_or_else(|| unknown_run(&one.run))?;
        let settled_here = run.task(&one.task).is_some_and(|task| {
            task.status == TaskStatus::Closed
                && matches!(&task.closed, Some(Closure::Outdated { why }) if why.as_str() == SETTLED_WHY)
        });
        if !settled_here {
            return Ok(false);
        }
        // No result is written: the author is who pressed, and nothing records it.
        let author = ResultAuthor::Person {
            attempt: None,
            source: None,
        };
        self.update_task(&one.run, &one.task, Some(was), None, author, now_ms)?;
        Ok(true)
    }
}
