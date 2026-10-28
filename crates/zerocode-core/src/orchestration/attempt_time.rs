//! Where an attempt's time went, as far as the ledger's own records can say
//! (t-47835).
//!
//! A stretch is counted only where the ledger holds both of its ends: a
//! question a worker asked, from the asking to its answer — or to the end of
//! the attempt, which closes it unanswered ([`Run::question_is_closed`]) — and
//! a decision gate on the task, from its opening to its resolution. Every
//! other moment of an attempt is `unknown`: the model's turns, the tools, the
//! build line's queue and the waits nobody wrote down. A quota wall, a silence,
//! a continuation or a landing check is an event with one moment and no
//! length, never a stretch: the ledger records no end for any of them.
//!
//! Stretches that overlap are kept as an overlap, not shared out by a rule of
//! precedence: [`AttemptTime::pieces`] cuts the attempt at every stretch's
//! ends, so the pieces add up to the attempt's own wall clock and each says
//! which stretches stood over it.
//!
//! The task's wall clock is [`super::task_cost::TaskCost::wall_ms`] — the
//! first attempt's start to the last one's end, with the gaps and overlaps
//! between attempts in it — and [`TaskTime`] says how much of it any attempt
//! covered. The wait for a review comes after the report and is never added to
//! either. Nothing here is stored, polled or estimated, and an open attempt
//! has no totals: its end is not now.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    Dispatch, GateStatus, MessageKind, Run, Task, handed_in_source, same_source, task_cost,
};

/// A stretch the ledger holds both ends of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// A question the attempt's worker asked.
    Question,
    /// A decision gate standing on the task.
    Gate,
}

/// How a stretch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldEnd {
    /// The seat the question was asked of answered it.
    Answered,
    /// The attempt ended with the question unanswered, which closes it.
    AttemptEnded,
    /// The gate was resolved.
    Resolved,
    /// It still stands.
    Standing,
    /// The record says it is over and holds no moment for it.
    Unknown,
}

/// One stretch, as its record wrote it, before any cut to an attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldSpan {
    pub hold: Hold,
    /// The question's message id, or the gate's id.
    pub record: String,
    pub from_ms: i64,
    pub to_ms: Option<i64>,
    pub ends: HoldEnd,
}

impl HoldSpan {
    /// The moment it stopped standing, as far as `end_ms` reaches: a stretch
    /// still standing stood until then, and one whose end is unknown counts
    /// for nothing.
    fn until(&self, end_ms: i64) -> Option<i64> {
        match self.ends {
            HoldEnd::Standing => Some(end_ms),
            HoldEnd::Unknown => None,
            HoldEnd::Answered | HoldEnd::AttemptEnded | HoldEnd::Resolved => self.to_ms,
        }
    }
}

/// One cut of an attempt, between two stretch ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Piece {
    pub from_ms: i64,
    pub to_ms: i64,
    /// Every stretch standing over the whole piece; empty where the ledger
    /// cannot say what the time went to.
    pub holds: Vec<Hold>,
}

/// One kind of event the attempt's records hold, counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTally {
    pub count: usize,
    pub first_ms: i64,
    pub last_ms: i64,
    /// The newest record of the kind.
    pub last: String,
}

/// One attempt's time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptTime {
    pub dispatch_id: String,
    pub worker_id: String,
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    pub succeeded: Option<bool>,
    /// The attempt's end less its start; `None` while it is open and for a
    /// clock that runs backwards.
    pub wall_ms: Option<i64>,
    pub holds: Vec<HoldSpan>,
    /// The attempt cut at every stretch's ends. Their lengths add up to
    /// `wall_ms`; empty while `wall_ms` is `None`.
    pub pieces: Vec<Piece>,
    /// Each stretch's own time inside the attempt; two kinds standing at once
    /// count for both.
    pub held_ms: Option<BTreeMap<Hold, i64>>,
    /// Time two or more kinds stood at once.
    pub overlap_ms: Option<i64>,
    /// Time no stretch accounts for.
    pub unknown_ms: Option<i64>,
    /// The ledger's own records about the attempt, and its report, by kind.
    pub events: BTreeMap<&'static str, EventTally>,
}

/// After the report: the wait for a coordinator's review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterReport {
    /// The attempt the coordinator's review names, or the newest one before
    /// any review.
    pub attempt: Option<String>,
    /// What the review names as handed in, or what that attempt handed in.
    pub source: Option<String>,
    /// The first `worker_done` of that attempt handing in that source; a later
    /// one, or one handing in something else, is not the report reviewed.
    pub reported_ms: Option<i64>,
    pub verified_ms: Option<i64>,
    pub review_wait_ms: Option<i64>,
    /// When the coordinator first wrote this review's `merged` ([`Run::completion_ms`]) — the
    /// moment the hand-in reached main as the ledger knows it, not git's merge commit.
    pub merged_ms: Option<i64>,
    /// The ledger stamps no moment for a deploy; never estimated.
    pub deployed_ms: Option<i64>,
}

/// One task's time across its attempts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTime {
    pub task_id: String,
    pub attempts: usize,
    pub first_started_ms: Option<i64>,
    /// [`super::task_cost::TaskCost::wall_ms`].
    pub wall_ms: Option<i64>,
    /// Time at least one attempt was open.
    pub covered_ms: Option<i64>,
    /// Time inside `wall_ms` no attempt was open.
    pub gap_ms: Option<i64>,
    /// Time two or more attempts were open at once.
    pub attempt_overlap_ms: Option<i64>,
    pub gates: Vec<HoldSpan>,
    pub after_report: AfterReport,
}

/// Where `attempt`'s time went.
#[must_use]
pub fn attempt_time(run: &Run, attempt: &Dispatch) -> AttemptTime {
    let mut holds = questions(run, attempt);
    holds.extend(gates(run, &attempt.task));
    let wall_ms = attempt
        .ended_ms
        .filter(|ended| attempt.started_ms >= 0 && *ended >= attempt.started_ms)
        .map(|ended| ended - attempt.started_ms);
    let pieces = attempt
        .ended_ms
        .filter(|_| wall_ms.is_some())
        .map(|ended| cut(attempt.started_ms, ended, &holds))
        .unwrap_or_default();
    let sums = wall_ms.map(|_| totals(&pieces));
    AttemptTime {
        dispatch_id: attempt.id.clone(),
        worker_id: attempt.worker.clone(),
        started_ms: attempt.started_ms,
        ended_ms: attempt.ended_ms,
        succeeded: attempt.succeeded,
        wall_ms,
        holds,
        pieces,
        held_ms: sums.as_ref().map(|(held, _, _)| held.clone()),
        overlap_ms: sums.as_ref().map(|(_, overlap, _)| *overlap),
        unknown_ms: sums.as_ref().map(|(_, _, unknown)| *unknown),
        events: events(run, &attempt.id),
    }
}

/// Where `task`'s time went across its attempts, and after its report.
#[must_use]
pub fn task_time(run: &Run, task: &Task) -> TaskTime {
    let attempts = task_cost::attempts(run, &task.id);
    let wall_ms = task_cost::wall_ms(&attempts);
    let cover = attempt_cover(&attempts);
    TaskTime {
        task_id: task.id.clone(),
        attempts: attempts.len(),
        first_started_ms: attempts.iter().map(|one| one.started_ms).min(),
        wall_ms,
        covered_ms: cover.map(|(covered, _)| covered),
        gap_ms: wall_ms
            .zip(cover)
            .map(|(wall, (covered, _))| wall - covered),
        attempt_overlap_ms: cover.map(|(_, overlap)| overlap),
        gates: gates(run, &task.id),
        after_report: after_report(run, task),
    }
}

fn questions(run: &Run, attempt: &Dispatch) -> Vec<HoldSpan> {
    run.messages()
        .iter()
        .filter(|held| {
            held.kind == MessageKind::Question
                && held.thread.is_none()
                && held.dispatch.as_deref() == Some(attempt.id.as_str())
        })
        .map(|question| {
            let (to_ms, ends) = match (run.answer_to(question), attempt.ended_ms) {
                (Some(answer), _) => (Some(answer.created_ms), HoldEnd::Answered),
                (None, Some(ended)) => (Some(ended), HoldEnd::AttemptEnded),
                (None, None) => (None, HoldEnd::Standing),
            };
            HoldSpan {
                hold: Hold::Question,
                record: question.id.clone(),
                from_ms: question.created_ms,
                to_ms,
                ends,
            }
        })
        .collect()
}

fn gates(run: &Run, task_id: &str) -> Vec<HoldSpan> {
    run.gates
        .iter()
        .filter(|gate| gate.task == task_id)
        .map(|gate| {
            let ends = match (gate.resolved_ms, gate.status) {
                (Some(_), _) => HoldEnd::Resolved,
                (None, GateStatus::Pending) => HoldEnd::Standing,
                (None, GateStatus::Resolved) => HoldEnd::Unknown,
            };
            HoldSpan {
                hold: Hold::Gate,
                record: gate.id.clone(),
                from_ms: gate.created_ms,
                to_ms: gate.resolved_ms,
                ends,
            }
        })
        .collect()
}

/// `[start_ms, end_ms]` cut at every stretch's ends inside it, each piece
/// with the stretches standing over all of it; neighbours standing under the
/// same stretches are one piece.
fn cut(start_ms: i64, end_ms: i64, holds: &[HoldSpan]) -> Vec<Piece> {
    let inside: Vec<(Hold, i64, i64)> = holds
        .iter()
        .filter_map(|span| {
            let from = span.from_ms.max(start_ms);
            let to = span.until(end_ms)?.min(end_ms);
            (to > from).then_some((span.hold, from, to))
        })
        .collect();
    let mut bounds: Vec<i64> = inside
        .iter()
        .flat_map(|(_, from, to)| [*from, *to])
        .chain([start_ms, end_ms])
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    let mut pieces: Vec<Piece> = Vec::new();
    for pair in bounds.windows(2) {
        let (from_ms, to_ms) = (pair[0], pair[1]);
        let mut standing: Vec<Hold> = inside
            .iter()
            .filter(|(_, from, to)| *from <= from_ms && *to >= to_ms)
            .map(|(hold, _, _)| *hold)
            .collect();
        standing.sort_unstable();
        standing.dedup();
        if let Some(last) = pieces.last_mut().filter(|last| last.holds == standing) {
            last.to_ms = to_ms;
            continue;
        }
        pieces.push(Piece {
            from_ms,
            to_ms,
            holds: standing,
        });
    }
    pieces
}

/// Each kind's time, the time two kinds or more stood at once, and the time
/// none did.
fn totals(pieces: &[Piece]) -> (BTreeMap<Hold, i64>, i64, i64) {
    let mut held: BTreeMap<Hold, i64> = BTreeMap::new();
    let (mut overlap, mut unknown) = (0, 0);
    for piece in pieces {
        let length = piece.to_ms - piece.from_ms;
        for hold in &piece.holds {
            *held.entry(*hold).or_default() += length;
        }
        match piece.holds.len() {
            0 => unknown += length,
            1 => {}
            _ => overlap += length,
        }
    }
    (held, overlap, unknown)
}

/// The time at least one of `attempts` was open and the time two or more
/// were, or `None` while one is open ([`task_cost::wall_ms`]).
pub(super) fn attempt_cover(attempts: &[&Dispatch]) -> Option<(i64, i64)> {
    task_cost::wall_ms(attempts).map(|_| {
        spread(
            attempts
                .iter()
                .filter_map(|one| Some((one.started_ms, one.ended_ms?))),
        )
    })
}

/// The time at least one of `spans` was open, and the time two or more were.
fn spread(spans: impl Iterator<Item = (i64, i64)>) -> (i64, i64) {
    let mut edges: Vec<(i64, i32)> = spans.flat_map(|(from, to)| [(from, 1), (to, -1)]).collect();
    edges.sort_unstable();
    let (mut covered, mut overlap, mut open, mut since) = (0, 0, 0, 0);
    for (at, step) in edges {
        if open >= 1 {
            covered += at - since;
        }
        if open >= 2 {
            overlap += at - since;
        }
        open += step;
        since = at;
    }
    (covered, overlap)
}

fn events(run: &Run, dispatch_id: &str) -> BTreeMap<&'static str, EventTally> {
    let mut tally: BTreeMap<&'static str, EventTally> = BTreeMap::new();
    for held in run.messages().iter().filter(|held| {
        held.dispatch.as_deref() == Some(dispatch_id)
            && (held.kind.is_the_ledgers_own() || held.kind == MessageKind::WorkerDone)
            && !super::gate_reuse::is_gate_evidence(held)
    }) {
        tally
            .entry(held.kind.as_str())
            .and_modify(|seen| {
                seen.count += 1;
                seen.first_ms = seen.first_ms.min(held.created_ms);
                if held.created_ms >= seen.last_ms {
                    seen.last_ms = held.created_ms;
                    seen.last.clone_from(&held.id);
                }
            })
            .or_insert_with(|| EventTally {
                count: 1,
                first_ms: held.created_ms,
                last_ms: held.created_ms,
                last: held.id.clone(),
            });
    }
    tally
}

fn after_report(run: &Run, task: &Task) -> AfterReport {
    let review = run.review_of(task);
    let newest = run.newest_attempt(&task.id);
    let (attempt, source) = match review.attempt.clone() {
        Some(named) => (Some(named), review.source.clone()),
        None => (
            newest.map(|one| one.id.clone()),
            newest.and_then(|one| one.source.clone()),
        ),
    };
    let reported_ms = attempt
        .as_deref()
        .zip(source.as_deref())
        .and_then(|(id, named)| {
            run.messages()
                .iter()
                .filter(|held| {
                    held.kind == MessageKind::WorkerDone
                        && held.dispatch.as_deref() == Some(id)
                        && same_source(&handed_in_source(held.body.as_str(), &held.id), named)
                })
                .map(|held| held.created_ms)
                .min()
        })
        .filter(|at| *at >= 0);
    let verified_ms = review.verified_ms.filter(|at| review.verified && *at >= 0);
    AfterReport {
        attempt,
        source,
        reported_ms,
        verified_ms,
        review_wait_ms: reported_ms
            .zip(verified_ms)
            .and_then(|(reported, verified)| verified.checked_sub(reported))
            .filter(|wait| *wait >= 0),
        merged_ms: run.completion_ms(task),
        deployed_ms: None,
    }
}
