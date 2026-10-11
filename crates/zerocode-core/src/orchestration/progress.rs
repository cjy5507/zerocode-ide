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
    /// The attempt ended without a successful report.
    Failed,
    /// Completed, and every attempt ended handing nothing in: no review can
    /// ever be written ([`ReviewFacts::unreviewable`]). Over, not verified.
    Unreviewable,
    /// A coordinator reviewed another attempt, or another commit than the one
    /// this attempt handed in: the review does not stand for this work.
    Stale,
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
    pub const ALL: [Self; 11] = [
        Self::Working,
        Self::ReportOnly,
        Self::Failed,
        Self::Unreviewable,
        Self::Stale,
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
            Self::Unreviewable => "unreviewable",
            Self::Stale => "stale",
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
    /// A command to read or check the work with from here, its ids quoted for
    /// a POSIX shell — shown to a person, never run, and never one that writes
    /// a review.
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
    #[must_use]
    pub fn progress_from(
        &self,
        task: Option<&Task>,
        review: &ReviewFacts,
        attempt: Option<&Dispatch>,
    ) -> WorkProgress {
        let (stage, claimed) = stage_of(task, review, attempt);
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
            next: self.next_step(task, stage, attempt, review),
            attempt: attempt_id,
            source,
            merge_head: review.merge_head.clone().filter(|_| stage.reviewed()),
            reviewed: review.source.clone().filter(|_| stage == WorkStage::Stale),
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
        review: &ReviewFacts,
    ) -> Option<String> {
        let task = task?;
        let newest = self.newest_attempt(&task.id)?;
        let current = attempt.filter(|one| one.id == newest.id);
        let argv: Vec<String> = match stage {
            // What the checkout can prove: its changes, its runs, its receipts.
            WorkStage::ReportOnly | WorkStage::Stale => vec![
                WORKTREE_EVIDENCE_VERB.into(),
                "--worker".into(),
                current?.worker.clone(),
            ],
            // The attempt's own words, to read why it ended.
            WorkStage::Failed => vec![
                WORKER_TRANSCRIPT_VERB.into(),
                "--worker".into(),
                current?.worker.clone(),
            ],
            // The landing check of the head the review named.
            WorkStage::Verified => {
                let head = review.source.clone()?;
                vec![
                    LAND_CHECK_VERB.into(),
                    "--task".into(),
                    task.id.clone(),
                    "--head".into(),
                    head,
                ]
            }
            WorkStage::Working
            | WorkStage::Unreviewable
            | WorkStage::NothingToLand
            | WorkStage::Merged
            | WorkStage::Deployed
            | WorkStage::Closed
            | WorkStage::Unknown => return None,
        };
        Some(command_line(argv, task))
    }
}

/// `argv` as one line a person can paste: the program, then each word bare
/// while it is shell-plain and quoted when it is not — with the request name
/// the ledger asks of a verb that writes, named after the task and its words so
/// that pasting it twice is one request.
fn command_line(mut argv: Vec<String>, task: &Task) -> String {
    if super::needs_a_retry_name(&argv) {
        let named = format!("{}-{}", argv[0], task.id);
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

/// The stage, and the worker's claim beside a report. Closed first; then a
/// coordinator's facts, in the order they supersede each other, believed only
/// with the source they were written against; then what no review can be
/// written for; then the attempt itself.
fn stage_of(
    task: Option<&Task>,
    review: &ReviewFacts,
    attempt: Option<&Dispatch>,
) -> (WorkStage, Option<WorkStage>) {
    if task.is_some_and(|task| task.status == TaskStatus::Closed || task.closed.is_some()) {
        return (WorkStage::Closed, None);
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
    if review.unreviewable {
        return (WorkStage::Unreviewable, None);
    }
    let Some(attempt) = attempt else {
        return (WorkStage::Unknown, None);
    };
    if attempt.is_open() {
        return (WorkStage::Working, None);
    }
    if attempt.succeeded != Some(true) {
        return (WorkStage::Failed, None);
    }
    // A coordinator's review whose attempt or source moved on: `review_of`
    // withheld its facts, and kept the ids that say why.
    if review.author == ReviewAuthor::Coordinator
        && (review.superseded_by.is_some() || review.source_now.is_some())
    {
        return (WorkStage::Stale, None);
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
