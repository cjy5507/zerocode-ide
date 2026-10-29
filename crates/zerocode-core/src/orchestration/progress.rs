//! Where a task's work stands on its way to main, read once (t-47538).
//!
//! The coordinator's desk, the sidebar and the task board each turned the
//! same ledger facts into a stage of their own: the desk counted a verified
//! task among the reported ones and a deployed one among the merged, the
//! window read a review written against an older attempt as plain 검증 대기,
//! and one surface turned a translated word back into a stage name. This is
//! the one reading — a projection of [`Run::review_of`] and the task's own
//! attempts, stored nowhere. Every surface draws the [`WorkStage`] it answers.
//!
//! A turn ending is not a stage, a `worker_done` is a report and never a
//! verification, and a coordinator's record that names no source it reviewed
//! is not believed as success: it is [`WorkStage::Unknown`]. Nothing here
//! estimates progress or a time, and nothing here runs anything: the next step
//! is a command to read or check with, shown to a person.

use super::land_check::LAND_CHECK_VERB;
use super::{
    Dispatch, ReviewAuthor, ReviewFacts, Run, Task, TaskStatus, WORKER_TRANSCRIPT_VERB,
    WORKTREE_EVIDENCE_VERB,
};

/// The program a coordinator types the ledger's verbs into, as every briefing
/// names it.
const ORC: &str = "zerocode-orc";

/// One task's work, as far as the ledger can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStage {
    /// The attempt is open.
    Working,
    /// Handed in, and no coordinator has written a review of it. A worker's
    /// own 「verified」 or 「merged」 rides beside it as [`WorkProgress::claimed`].
    ReportOnly,
    /// The attempt ended on the worker's own word that it failed: a
    /// `worker_done` saying `ok:false`.
    Failed,
    /// The attempt ended and nobody saw a result (t-73910): the coordinator
    /// stopped or abandoned it, or its terminal went away — the ledger writes
    /// `succeeded: None` for each, "a failure nobody observed" being the one
    /// thing it will not record. Not a failure, and not a report to review.
    Stopped,
    /// Completed, and every attempt ended handing nothing in: no review can
    /// ever be written ([`ReviewFacts::unreviewable`]). Over, not verified.
    Unreviewable,
    /// Completed, and the person waived its review through the window's own
    /// door: 검토 생략(사람) ([`ReviewFacts::waived`], t-71220). Over, not
    /// verified.
    Waived,
    /// A coordinator reviewed an earlier attempt, or an earlier commit than the
    /// one this attempt handed in, naming the source it read: the review does
    /// not stand for this work.
    Stale,
    /// An earlier attempt the task has moved past: a newer attempt carries the
    /// work, so this one neither waits for a review nor wears the newer one's.
    Superseded,
    /// Verified against its source, and nothing says it landed.
    Verified,
    /// Verified, with no code to land.
    NothingToLand,
    /// Landed, and nothing says it shipped.
    Merged,
    /// Shipped.
    Deployed,
    /// Folded, handed to another run, or outdated.
    Closed,
    /// The ledger cannot say: a completed task no attempt ever carried, or a
    /// coordinator's record from before reviews named the source they read.
    Unknown,
}

impl WorkStage {
    /// Every stage, each once.
    pub const ALL: [Self; 14] = [
        Self::Working,
        Self::ReportOnly,
        Self::Failed,
        Self::Stopped,
        Self::Unreviewable,
        Self::Waived,
        Self::Stale,
        Self::Superseded,
        Self::Verified,
        Self::NothingToLand,
        Self::Merged,
        Self::Deployed,
        Self::Closed,
        Self::Unknown,
    ];

    /// The stage's name on the wire — the key every window table is read by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::ReportOnly => "report_only",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Unreviewable => "unreviewable",
            Self::Waived => "waived",
            Self::Stale => "stale",
            Self::Superseded => "superseded",
            Self::Verified => "verified",
            Self::NothingToLand => "nothing_to_land",
            Self::Merged => "merged",
            Self::Deployed => "deployed",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }

    /// Whether a coordinator's review stands behind it.
    #[must_use]
    pub const fn reviewed(self) -> bool {
        matches!(
            self,
            Self::Verified | Self::NothingToLand | Self::Merged | Self::Deployed
        )
    }

    /// What comes next for work at this stage (t-73910 W4-1), decided here
    /// once: every surface translates the step's word and keeps no table of
    /// its own. A stage added later must say its step to compile.
    #[must_use]
    pub const fn next_step(self) -> NextStep {
        match self {
            Self::Working => NextStep::Wait,
            Self::ReportOnly => NextStep::Review,
            Self::Failed => NextStep::RetryOrClose,
            Self::Stopped => NextStep::RestartOrClose,
            Self::Stale => NextStep::ReviewNewHead,
            Self::Verified => NextStep::Land,
            Self::Merged => NextStep::Ship,
            // The newer attempt carries the work; this one is behind it.
            Self::Superseded
            | Self::Unreviewable
            | Self::Waived
            | Self::NothingToLand
            | Self::Deployed
            | Self::Closed => NextStep::Over,
            Self::Unknown => NextStep::Unknown,
        }
    }
}

/// The step that comes next for a task's work, as a word every window table
/// is read by: what is to be done, never a command (that is
/// [`WorkProgress::next`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextStep {
    /// Its dependencies are not done yet.
    AwaitDependencies,
    /// Nobody carries it: summon a worker, or do it.
    Summon,
    /// A worker is on it.
    Wait,
    /// A decision stands in front of it: answer it.
    Decide,
    /// A dependency failed: mend it, or close this.
    MendDependency,
    /// Review what was handed in.
    Review,
    /// The worker said it failed: hand it out again, or close it.
    RetryOrClose,
    /// Nobody saw it end: start it again from where it stopped, or close it.
    RestartOrClose,
    /// The review stands for older work: review the newer source.
    ReviewNewHead,
    /// Verified: land it.
    Land,
    /// Landed: ship it.
    Ship,
    /// Nothing is left to do.
    Over,
    /// The ledger cannot say.
    Unknown,
}

impl NextStep {
    /// Every step, each once.
    pub const ALL: [Self; 13] = [
        Self::AwaitDependencies,
        Self::Summon,
        Self::Wait,
        Self::Decide,
        Self::MendDependency,
        Self::Review,
        Self::RetryOrClose,
        Self::RestartOrClose,
        Self::ReviewNewHead,
        Self::Land,
        Self::Ship,
        Self::Over,
        Self::Unknown,
    ];
}

/// A [`WorkStage`] with the ids it stands on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkProgress {
    pub stage: WorkStage,
    /// What the worker says beyond its report — verified, merged or deployed —
    /// on [`WorkStage::ReportOnly`] only. A claim, never the fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed: Option<WorkStage>,
    /// The attempt the stage is about: the one the review named for reviewed
    /// work, the one asked about otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// The commit or report the stage stands on: the one a coordinator
    /// reviewed for reviewed work, the one handed in otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The commit the work landed as, when the coordinator wrote one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_head: Option<String>,
    /// On [`WorkStage::Stale`]: the source the review that does not stand had
    /// named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed: Option<String>,
    /// On [`WorkStage::ReportOnly`]: the coordinator withdrew its verification
    /// of this attempt and source, and these are its words why — empty where
    /// it gave none ([`ReviewFacts::withdrawn`], t-81913). The work waits on a
    /// new review, and a bare pass of it is refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<String>,
    /// A command to read or check the work with from here, scoped to its run
    /// and its ids quoted for a POSIX shell — shown to a person, never run, and
    /// never one that writes a review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

impl Run {
    /// Where `task`'s work stands as a whole: its newest attempt's.
    #[must_use]
    pub fn progress_of(&self, task: &Task) -> WorkProgress {
        self.progress_from(
            Some(task),
            &self.review_of(task),
            self.newest_attempt(&task.id),
        )
    }

    /// Where the work of `attempt` on `task` stands, over a review the caller
    /// already read with [`Run::review_of`] — read once per row. `task` is
    /// `None` where a sweep compacted the task away and the attempt is all
    /// the ledger has left: the attempt's own end is then all there is to say.
    /// An attempt older than the task's newest reads by its own end, never by
    /// the review of another attempt.
    #[must_use]
    pub fn progress_from(
        &self,
        task: Option<&Task>,
        review: &ReviewFacts,
        attempt: Option<&Dispatch>,
    ) -> WorkProgress {
        let newest = task.and_then(|task| self.newest_attempt(&task.id));
        let (stage, claimed) = stage_of(task, review, attempt, newest);
        let (attempt_id, source) = if stage.reviewed() {
            (review.attempt.clone(), review.source.clone())
        } else {
            (
                attempt.map(|one| one.id.clone()),
                attempt.and_then(|one| one.source.clone()),
            )
        };
        WorkProgress {
            stage,
            claimed,
            next: self.next_step(task, stage, attempt, newest, review),
            attempt: attempt_id,
            source,
            merge_head: review.merge_head.clone().filter(|_| stage.reviewed()),
            reviewed: review.source.clone().filter(|_| stage == WorkStage::Stale),
            withdrawn: review
                .withdrawn
                .clone()
                .filter(|_| stage == WorkStage::ReportOnly),
        }
    }

    /// The command that reads or checks the work at `stage`, or `None` where
    /// there is nothing to look at or the attempt is not the task's newest —
    /// a check of an attempt the task has moved past answers nobody.
    fn next_step(
        &self,
        task: Option<&Task>,
        stage: WorkStage,
        attempt: Option<&Dispatch>,
        newest: Option<&Dispatch>,
        review: &ReviewFacts,
    ) -> Option<String> {
        let task = task?;
        let newest = newest?;
        let current = attempt.filter(|one| one.id == newest.id);
        let (verb, words): (&str, Vec<String>) = match stage {
            WorkStage::ReportOnly | WorkStage::Stale => (
                WORKTREE_EVIDENCE_VERB,
                vec!["--worker".into(), current?.worker.clone()],
            ),
            WorkStage::Failed | WorkStage::Stopped => (
                WORKER_TRANSCRIPT_VERB,
                vec!["--worker".into(), current?.worker.clone()],
            ),
            WorkStage::Verified => {
                let head = review.source.as_deref().and_then(super::commit_named)?;
                (
                    LAND_CHECK_VERB,
                    vec!["--task".into(), task.id.clone(), "--head".into(), head],
                )
            }
            WorkStage::Working
            | WorkStage::Unreviewable
            | WorkStage::Waived
            | WorkStage::Superseded
            | WorkStage::NothingToLand
            | WorkStage::Merged
            | WorkStage::Deployed
            | WorkStage::Closed
            | WorkStage::Unknown => return None,
        };
        let argv = [verb.to_string(), "--run".to_string(), self.id.clone()]
            .into_iter()
            .chain(words)
            .collect();
        Some(command_line(argv, &task.id))
    }
}

/// `argv` as one line a person can paste: the program, then each word bare
/// while it is shell-plain and quoted when it is not — with the request name
/// the ledger asks of a verb that writes, named after the task and its words so
/// that pasting it twice is one request.
fn command_line(mut argv: Vec<String>, task: &str) -> String {
    if super::needs_a_retry_name(&argv) {
        let named = format!("{}-{task}", argv[0]);
        let last = argv.last().cloned().unwrap_or_default();
        argv.push(super::RETRY_REQUEST.to_string());
        argv.push(format!(
            "{named}-{}",
            last.chars().take(12).collect::<String>()
        ));
    }
    std::iter::once(ORC.to_string())
        .chain(argv.iter().map(|word| crate::vault::resume_word(word)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The stage, and the worker's claim beside a report. Closed first; then an
/// attempt older than `newest`, by its own end alone; then a coordinator's
/// facts, in the order they supersede each other, believed only with the
/// source they were written against; then the person's waiver, bound the
/// same way; then what no review can be written for;
/// then the attempt itself. A review that no longer stands reads stale only
/// where the coordinator's record carried review keys and named the source it
/// read: one that named no source is unknown, whatever attempt it named, and a
/// note that carries no review leaves the report waiting.
fn stage_of(
    task: Option<&Task>,
    review: &ReviewFacts,
    attempt: Option<&Dispatch>,
    newest: Option<&Dispatch>,
) -> (WorkStage, Option<WorkStage>) {
    if task.is_some_and(|task| task.status == TaskStatus::Closed || task.closed.is_some()) {
        return (WorkStage::Closed, None);
    }
    if let Some(older) = attempt.filter(|one| newest.is_some_and(|newest| newest.id != one.id)) {
        let stage = if review.unreviewable {
            WorkStage::Unreviewable
        } else if older.is_open() {
            WorkStage::Working
        } else if older.succeeded == Some(false) {
            WorkStage::Failed
        } else if older.succeeded.is_none() {
            WorkStage::Stopped
        } else {
            WorkStage::Superseded
        };
        return (stage, None);
    }
    let fact = [
        (review.deployed, WorkStage::Deployed),
        (review.merged, WorkStage::Merged),
        (review.nothing_to_land, WorkStage::NothingToLand),
        (review.verified, WorkStage::Verified),
    ]
    .into_iter()
    .find_map(|(holds, stage)| holds.then_some(stage));
    if let Some(stage) = fact {
        let bound = review
            .source
            .as_deref()
            .is_some_and(|named| !named.trim().is_empty());
        return (if bound { stage } else { WorkStage::Unknown }, None);
    }
    if review.waived {
        return (WorkStage::Waived, None);
    }
    if review.unreviewable {
        return (WorkStage::Unreviewable, None);
    }
    let Some(attempt) = attempt else {
        return (WorkStage::Unknown, None);
    };
    if attempt.is_open() {
        return (WorkStage::Working, None);
    }
    // Its worker said it failed, or nobody saw a result (t-73910).
    if attempt.succeeded == Some(false) {
        return (WorkStage::Failed, None);
    }
    if attempt.succeeded.is_none() {
        return (WorkStage::Stopped, None);
    }
    if review.author == ReviewAuthor::Coordinator
        && (review.superseded_by.is_some() || review.source_now.is_some())
        && task.is_some_and(|task| ReviewFacts::from_result(task.result.as_str()).written)
    {
        let named = review
            .source
            .as_deref()
            .is_some_and(|named| !named.trim().is_empty());
        return (
            if named {
                WorkStage::Stale
            } else {
                WorkStage::Unknown
            },
            None,
        );
    }
    let claim = [
        (review.claimed_deployed, WorkStage::Deployed),
        (review.claimed_merged, WorkStage::Merged),
        (review.claimed_verified, WorkStage::Verified),
    ]
    .into_iter()
    .find_map(|(holds, stage)| holds.then_some(stage));
    (WorkStage::ReportOnly, claim)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command shown for a person to paste quotes every word a shell would split or expand,
    /// and a verb that writes carries the one request name that makes pasting it twice one request.
    #[test]
    fn a_shown_command_quotes_what_a_shell_would_read_twice() {
        let read = command_line(
            vec![
                WORKTREE_EVIDENCE_VERB.into(),
                "--worker".into(),
                "w-1; rm -rf x".into(),
            ],
            "t-1",
        );
        assert_eq!(
            read,
            "zerocode-orc worktree-evidence --worker 'w-1; rm -rf x'"
        );
        let check = command_line(
            vec![
                LAND_CHECK_VERB.into(),
                "--task".into(),
                "t-1".into(),
                "--head".into(),
                "abcdef1234567890".into(),
            ],
            "t-1",
        );
        assert_eq!(
            check,
            "zerocode-orc land-check --task t-1 --head abcdef1234567890 --retry-request land-check-t-1-abcdef123456"
        );
    }
}
