//! CI checks on a hosted review, as this window understands them.
//!
//! Orca reaches GitHub three ways and normalises all three into ONE shape
//! before any component sees it: a GraphQL rollup first, a REST fallback
//! (`gh api repos/…/commits/<sha>/check-runs`), and `gh pr checks` last
//! (`getPRChecks`/`getPRChecksViaRestFallback`, out/main/index.js:59639-59760).
//! The normalisers are the interesting part and they are what this module is —
//! GitHub's vocabulary is wider than the panel's, and the narrowing is a set
//! of decisions rather than a passthrough:
//!
//!   - a run that has not `completed` has **no** conclusion yet, whatever the
//!     field says: status wins and the conclusion reads `pending`
//!   - `stale` and `startup_failure` are failures, and nothing in the panel
//!     knows those two words exist
//!   - a legacy commit status speaks `state` instead, where `error` is also a
//!     failure and `pending` is the only non-terminal word
//!
//! Reading these off the wire in the window would put the same table in four
//! places (list, details, counts, prompt). It lives here once, and the shell
//! hands the window a `CheckRun`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Where a run is in its life. GitHub's own three words, narrowed: anything
/// that is not queued or running has finished, so `completed` is the default
/// rather than a fourth state (`mapCheckRunRESTStatus`, index.js:56740).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
}

impl CheckStatus {
    /// From a REST check-run `status`. Unknown words read as finished, which
    /// is the measured default and the safe one: an unrecognised word must not
    /// leave a row spinning forever.
    pub fn from_check_run(status: &str) -> Self {
        match status.to_ascii_lowercase().as_str() {
            "queued" => Self::Queued,
            "in_progress" => Self::InProgress,
            _ => Self::Completed,
        }
    }

    /// From a legacy commit status `state`, which has only the two
    /// (`mapCommitStatusRESTStatus`, :56770). `pending` there means "queued",
    /// NOT the panel's pending — the conclusion below carries that.
    pub fn from_commit_status(state: &str) -> Self {
        if state.eq_ignore_ascii_case("pending") {
            Self::Queued
        } else {
            Self::Completed
        }
    }
}

/// How a run ended. `Pending` is not one of GitHub's conclusions — it is what
/// this window calls a run that has not ended, so the list has one field to
/// sort, colour and count by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckConclusion {
    Success,
    Failure,
    Cancelled,
    TimedOut,
    Skipped,
    Neutral,
    ActionRequired,
    Pending,
}

impl CheckConclusion {
    /// The measured `conclusionMap` (:56750-56769), including the two words a
    /// reader would not guess: `stale` and `startup_failure` are failures.
    ///
    /// `None` means GitHub said something this build does not know — the panel
    /// draws it as neither passing nor failing rather than guessing, and that
    /// is why the return is an `Option` instead of falling back to `Neutral`.
    pub fn from_check_run(status: &str, conclusion: Option<&str>) -> Option<Self> {
        if CheckStatus::from_check_run(status) != CheckStatus::Completed {
            return Some(Self::Pending);
        }
        let word = conclusion?;
        match word.to_ascii_lowercase().as_str() {
            "success" => Some(Self::Success),
            "failure" | "stale" | "startup_failure" => Some(Self::Failure),
            "cancelled" => Some(Self::Cancelled),
            "timed_out" => Some(Self::TimedOut),
            "skipped" => Some(Self::Skipped),
            "neutral" => Some(Self::Neutral),
            "action_required" => Some(Self::ActionRequired),
            _ => None,
        }
    }

    /// From a legacy commit status `state` (`mapCommitStatusRESTConclusion`,
    /// :56774). `error` joins `failure`; there is no cancelled, no skipped.
    pub fn from_commit_status(state: &str) -> Option<Self> {
        match state.to_ascii_lowercase().as_str() {
            "success" => Some(Self::Success),
            "failure" | "error" => Some(Self::Failure),
            "pending" => Some(Self::Pending),
            _ => None,
        }
    }

    /// Where this conclusion sorts in the list. Failures first because they
    /// are why the panel is open (`CHECK_SORT_ORDER`,
    /// checks-panel-content-DbtDhGaT.js:1600).
    pub fn sort_rank(self) -> u8 {
        match self {
            Self::Failure | Self::TimedOut | Self::ActionRequired => 0,
            Self::Cancelled => 1,
            Self::Pending => 2,
            Self::Neutral => 3,
            Self::Skipped => 4,
            Self::Success => 5,
        }
    }

    /// Does this row read as a failure in the LIST — the question the icon and
    /// the failing count ask (`isFailedCheck`, :1628).
    pub fn reads_as_failed(self) -> bool {
        matches!(
            self,
            Self::Failure | Self::Cancelled | Self::TimedOut | Self::ActionRequired
        )
    }

    /// Is this something an agent can be asked to FIX — a narrower question,
    /// and Orca answers it differently: `getBrokenChecks`
    /// (fix-checks-agent-launch-DGiOvpEA.js:41) drops `action_required`, which
    /// `isFailedCheck` keeps.
    ///
    /// The asymmetry is deliberate and worth preserving: an approval blocker
    /// is a person's click on GitHub, not a defect in the tree, so handing it
    /// to an agent is asking it to fix something that is not broken. The
    /// details pane says exactly that in its own words.
    pub fn is_fixable_breakage(self) -> bool {
        matches!(self, Self::Failure | Self::Cancelled | Self::TimedOut)
    }
}

/// One row of the checks list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRun {
    pub name: String,
    pub status: CheckStatus,
    /// `None` when GitHub reported a conclusion this build does not know.
    #[serde(default)]
    pub conclusion: Option<CheckConclusion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_run_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<u64>,
}

impl CheckRun {
    /// The conclusion to reason with. An unknown word and a missing one both
    /// land on `pending`, which is how every consumer in Orca reads it
    /// (`getCheckConclusion`: `check.conclusion ?? "pending"`).
    pub fn effective_conclusion(&self) -> CheckConclusion {
        self.conclusion.unwrap_or(CheckConclusion::Pending)
    }

    /// The row's identity, in Orca's own order of preference
    /// (`getCheckIdentityKey`, :1610). The index is the last resort because
    /// two unnamed runs of the same name would otherwise collapse into one
    /// expandable row.
    pub fn identity_key(&self, index: usize) -> String {
        if let Some(id) = self.check_run_id {
            return format!("check-run:{id}");
        }
        if let Some(id) = self.workflow_run_id {
            return format!("workflow-run:{id}");
        }
        match &self.url {
            Some(url) => format!("url:{url}"),
            None => format!("fallback:{}:{index}", self.name),
        }
    }
}

/// The Actions run a details URL points at, or `None`. Orca reads it with
/// `/\/actions\/runs\/(\d+)(?:\/|$)/` (`parseActionsRunId`, :60051); this
/// walks the segments instead, because a regex literal is invisible to this
/// repo's own source scanners and the two agree on every input.
pub fn actions_run_id(url: &str) -> Option<u64> {
    let mut segments = url.split('/');
    while let Some(segment) = segments.next() {
        if segment != "actions" {
            continue;
        }
        if segments.next() != Some("runs") {
            continue;
        }
        let digits = segments.next()?;
        // The regex ends at a `/` or the string, so a segment that is not all
        // digits is not a run id — `runs/12ab` matches nothing.
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            return digits.parse().ok();
        }
    }
    None
}

/// What the summary row counts. Three independent tallies over the same list,
/// not a partition: an unknown conclusion is in none of them, which is why
/// they can sum to less than the list's length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckTally {
    pub passing: usize,
    pub failing: usize,
    pub pending: usize,
}

impl CheckTally {
    pub fn of(checks: &[CheckRun]) -> Self {
        let mut tally = Self::default();
        for check in checks {
            match check.effective_conclusion() {
                CheckConclusion::Success => tally.passing += 1,
                CheckConclusion::Pending => tally.pending += 1,
                other if other.reads_as_failed() => tally.failing += 1,
                _ => {}
            }
        }
        tally
    }
}

/// Failures first, then the list's own order. Orca sorts with a comparator on
/// the rank alone (`sorted`, :1927), which JS keeps stable; `sort_by_key` is
/// stable too, so equal ranks stay in the order GitHub returned them — the
/// list does not reshuffle under a refresh that changed nothing.
pub fn sort_for_display(checks: &mut [CheckRun]) {
    checks.sort_by_key(|check| check.effective_conclusion().sort_rank());
}

/// The runs an agent can be asked to fix, in display order.
pub fn broken_checks(checks: &[CheckRun]) -> Vec<&CheckRun> {
    checks
        .iter()
        .filter(|check| check.effective_conclusion().is_fixable_breakage())
        .collect()
}

/// A check's own page, fetched when a row is opened.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckDetails {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The three fields of a check run's `output`, which is where a linter
    /// puts what it actually found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub annotations: Vec<CheckAnnotation>,
    #[serde(default)]
    pub jobs: Vec<CheckJob>,
}

/// One annotation — a file, a line, and what is wrong there.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckAnnotation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_details: Option<String>,
}

/// One job of a workflow run, with the steps that failed inside it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckJob {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The tail of this job's log, fetched only for jobs that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_tail: Option<String>,
    #[serde(default)]
    pub steps: Vec<CheckStep>,
}

/// One step inside a job.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CheckStep {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
}

/// Did a job or step end badly? A wider list than a check run's, because the
/// Actions API answers with words check runs never use — `failed`, `stale`,
/// `startup_failure` (`isCheckJobFailureState`, index.js:59930).
pub fn job_state_failed(state: Option<&str>) -> bool {
    matches!(
        state.map(str::to_ascii_lowercase).as_deref(),
        Some(
            "failure"
                | "failed"
                | "action_required"
                | "cancelled"
                | "stale"
                | "startup_failure"
                | "timed_out"
        )
    )
}

impl CheckJob {
    /// The word a row shows for this job: its conclusion if it has one, else
    /// where it is.
    pub fn state(&self) -> Option<&str> {
        self.conclusion.as_deref().or(self.status.as_deref())
    }

    pub fn failed(&self) -> bool {
        job_state_failed(self.state())
    }
}

impl CheckStep {
    pub fn state(&self) -> Option<&str> {
        self.conclusion.as_deref().or(self.status.as_deref())
    }

    pub fn failed(&self) -> bool {
        job_state_failed(self.state())
    }
}

/// How many trailing lines of a failed job's log travel in a fix prompt
/// (`PROMPT_LOG_TAIL_LINES`, fix-checks-agent-launch:8).
pub const PROMPT_LOG_TAIL_LINES: usize = 150;

/// The tail of a log, as a prompt carries it: the last
/// [`PROMPT_LOG_TAIL_LINES`] lines with CRLF normalised
/// (`truncateLogTailForPrompt`, :46).
pub fn log_tail_for_prompt(log: &str) -> String {
    let normalised = log.replace("\r\n", "\n");
    let mut breaks = 0usize;
    // Walk back from the end so a 16KB log is not split into a vector of
    // lines to count them.
    for (index, byte) in normalised.bytes().enumerate().rev() {
        if byte != b'\n' {
            continue;
        }
        breaks += 1;
        if breaks >= PROMPT_LOG_TAIL_LINES {
            return normalised[index + 1..].to_string();
        }
    }
    normalised
}

/* ---- the numbers, the stack, and the line an agent reads (t-2733) ---------
 *
 * Three things the PR depth adds, each pure so the shell and the window can
 * both lean on them without a network: the one numbers table every polling
 * interval and byte cap comes from, the order a stack of reviews stands in,
 * and the fingerprint that says whether CI actually moved since the last
 * look — the fact that decides whether an agent is told anything at all. */

/// Every number the PR depth has, in one place.
///
/// A settings overlay (`checks.<field>`) is the only road to a different
/// value; see [`Limits::overlaid`]. The poll is Orca's own `POLL_MS` (4s),
/// and the floor under it exists because a zero would spin the panel against
/// `gh`'s rate budget — the overlay may slow the beat, never remove it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// Background observation cadence, independent of the Checks panel.
    pub observe_ms: u64,
    pub review_ms: u64,
    pub force_ms: u64,
    pub gh_calls_max: usize,
    /// The longest one `gh` call may run before the observer gives up on it.
    pub gh_call_ms: u64,
    pub head_nudges_max: u64,
    pub mail_bytes_max: usize,
    /// How often a visible checks panel asks again, in milliseconds.
    pub poll_ms: u64,
    /// The least an overlay may ask for.
    pub poll_ms_min: u64,
    /// The per-PR diff cache's byte cap (LRU, oldest PR out first).
    pub diff_cache_bytes: u64,
    /// The largest whole-PR diff the cache will hold; past it a PR's diff is
    /// read again on every file rather than pinned.
    pub diff_bytes_max: u64,
    /// Files one PR detail lists — the wire's own page.
    pub files_max: usize,
    /// Review comments one PR detail reads into threads.
    pub conversation_max: usize,
    /// Open reviews read to draw the stack map.
    pub stack_prs_max: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            observe_ms: 30_000,
            review_ms: 120_000,
            force_ms: 300_000,
            gh_calls_max: 32,
            gh_call_ms: 10_000,
            head_nudges_max: 3,
            mail_bytes_max: 8_192,
            poll_ms: 4_000,
            poll_ms_min: 1_000,
            diff_cache_bytes: 8 * 1024 * 1024,
            diff_bytes_max: 4 * 1024 * 1024,
            files_max: 100,
            conversation_max: 100,
            stack_prs_max: 50,
        }
    }
}

/// The overlay's key prefix; `checks.poll_ms` names [`Limits::poll_ms`].
pub const OVERLAY_PREFIX: &str = "checks.";

impl Limits {
    /// Lay a settings overlay over the table. Unknown keys are ignored, the
    /// poll is floored, and every other field takes the value as given — the
    /// table is the authority on what exists, the overlay only on what it
    /// says (the same contract `artifact::Limits` keeps).
    #[must_use]
    pub fn overlaid(mut self, overlay: &BTreeMap<String, u64>) -> Self {
        for (key, value) in overlay {
            let Some(field) = key.strip_prefix(OVERLAY_PREFIX) else {
                continue;
            };
            let value = *value;
            let as_usize = usize::try_from(value).unwrap_or(usize::MAX);
            match field {
                "observe_ms" => self.observe_ms = value.max(self.poll_ms_min),
                "review_ms" => self.review_ms = value.max(self.poll_ms_min),
                "force_ms" => self.force_ms = value.max(self.poll_ms_min),
                "gh_calls_max" => self.gh_calls_max = as_usize.max(1),
                "gh_call_ms" => self.gh_call_ms = value.max(self.poll_ms_min),
                "head_nudges_max" => self.head_nudges_max = value,
                "mail_bytes_max" => self.mail_bytes_max = as_usize.clamp(512, 16_384),
                "poll_ms" => self.poll_ms = value.max(self.poll_ms_min),
                "diff_cache_bytes" => self.diff_cache_bytes = value,
                "diff_bytes_max" => self.diff_bytes_max = value,
                "files_max" => self.files_max = as_usize,
                "conversation_max" => self.conversation_max = as_usize,
                "stack_prs_max" => self.stack_prs_max = as_usize,
                _ => {}
            }
        }
        self
    }
}

/// One review as the stack reads it: its number and the two branches that
/// link it to its neighbours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackPr {
    pub number: u64,
    /// `headRefName` — the branch this review adds.
    pub head: String,
    /// `baseRefName` — the branch it merges into, and therefore the review
    /// below it when that branch is somebody else's head.
    pub base: String,
}

/// Where the reviews stand: the chain the anchor is on, head→base, and the
/// reviews in the set that no link reaches from it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct StackOrder {
    pub chain: Vec<u64>,
    pub orphans: Vec<u64>,
}

/// The stack the review `anchor` is on.
///
/// Pure, and defined by the links alone: a review's parent is the one whose
/// head is this review's base, its child the one whose base is this head.
/// The chain reads from the topmost child down to the bottom parent — the
/// order a person merges from the other end of. A review the set does not
/// hold anchors nothing, so everything is an orphan of it, and a cycle stops
/// at the first review already visited rather than walking forever.
pub fn stack_order(anchor: u64, prs: &[StackPr]) -> StackOrder {
    let Some(standing) = prs.iter().find(|pr| pr.number == anchor) else {
        return StackOrder {
            chain: Vec::new(),
            orphans: prs.iter().map(|pr| pr.number).collect(),
        };
    };
    let mut seen = std::collections::HashSet::from([anchor]);
    // Down: whose head is my base.
    let mut below = Vec::new();
    let mut base = standing.base.as_str();
    while let Some(parent) = prs
        .iter()
        .find(|pr| pr.head == base && !seen.contains(&pr.number))
    {
        seen.insert(parent.number);
        below.push(parent.number);
        base = parent.base.as_str();
    }
    // Up: whose base is my head.
    let mut above = Vec::new();
    let mut head = standing.head.as_str();
    while let Some(child) = prs
        .iter()
        .find(|pr| pr.base == head && !seen.contains(&pr.number))
    {
        seen.insert(child.number);
        above.push(child.number);
        head = child.head.as_str();
    }
    above.reverse();
    let mut chain = above;
    chain.push(anchor);
    chain.extend(below);
    StackOrder {
        chain,
        orphans: prs
            .iter()
            .map(|pr| pr.number)
            .filter(|number| !seen.contains(number))
            .collect(),
    }
}

/// What CI said, as one comparable string.
///
/// Name, status and conclusion of every row, in the order given: the tally
/// alone is blind to a failure that moved from one check to another, and
/// that move is exactly what an agent that just pushed wants to hear about.
pub fn checks_fingerprint(checks: &[CheckRun]) -> String {
    let mut out = String::new();
    for check in checks {
        out.push_str(&check.name);
        out.push('=');
        out.push_str(match check.status {
            CheckStatus::Queued => "queued",
            CheckStatus::InProgress => "in_progress",
            CheckStatus::Completed => "completed",
        });
        out.push('/');
        out.push_str(match check.effective_conclusion() {
            CheckConclusion::Success => "success",
            CheckConclusion::Failure => "failure",
            CheckConclusion::Cancelled => "cancelled",
            CheckConclusion::TimedOut => "timed_out",
            CheckConclusion::Skipped => "skipped",
            CheckConclusion::Neutral => "neutral",
            CheckConclusion::ActionRequired => "action_required",
            CheckConclusion::Pending => "pending",
        });
        out.push(';');
    }
    out
}

/// The one line an agent is mailed when CI moved — "checks: 2 failed
/// (ci/test, ci/lint), 3 passed". English on purpose: mail to agents is not
/// reader-facing prose, and the window's own panel wears the translated
/// words. Failures are named because the name is what the agent greps for.
pub fn checks_mail_line(checks: &[CheckRun]) -> String {
    let tally = CheckTally::of(checks);
    let mut parts = Vec::new();
    if tally.failing > 0 {
        let names: Vec<&str> = checks
            .iter()
            .filter(|check| check.effective_conclusion().reads_as_failed())
            .map(|check| check.name.as_str())
            .collect();
        parts.push(format!("{} failed ({})", tally.failing, names.join(", ")));
    }
    if tally.pending > 0 {
        parts.push(format!("{} pending", tally.pending));
    }
    if tally.passing > 0 {
        parts.push(format!("{} passed", tally.passing));
    }
    if parts.is_empty() {
        return "checks: none".to_string();
    }
    format!("checks: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, conclusion: CheckConclusion) -> CheckRun {
        CheckRun {
            name: name.into(),
            status: CheckStatus::Completed,
            conclusion: Some(conclusion),
            url: None,
            check_run_id: None,
            workflow_run_id: None,
        }
    }

    #[test]
    fn a_run_that_has_not_finished_has_no_conclusion_yet() {
        // The field can carry a stale word while the run is restarting; the
        // status is the one that renders.
        assert_eq!(
            CheckConclusion::from_check_run("in_progress", Some("failure")),
            Some(CheckConclusion::Pending)
        );
        assert_eq!(
            CheckConclusion::from_check_run("queued", None),
            Some(CheckConclusion::Pending)
        );
        // Finished with nothing said is unknown, not pending.
        assert_eq!(CheckConclusion::from_check_run("completed", None), None);
    }

    #[test]
    fn the_two_words_a_reader_would_not_guess_are_failures() {
        for word in ["stale", "startup_failure", "FAILURE"] {
            assert_eq!(
                CheckConclusion::from_check_run("completed", Some(word)),
                Some(CheckConclusion::Failure),
                "`{word}` stopped reading as a failure"
            );
        }
        // And a word from a future GitHub is not silently a failure.
        assert_eq!(
            CheckConclusion::from_check_run("completed", Some("quantum_flaked")),
            None
        );
    }

    #[test]
    fn a_legacy_commit_status_speaks_its_own_narrower_words() {
        assert_eq!(
            CheckConclusion::from_commit_status("error"),
            Some(CheckConclusion::Failure)
        );
        assert_eq!(
            CheckConclusion::from_commit_status("pending"),
            Some(CheckConclusion::Pending)
        );
        assert_eq!(CheckConclusion::from_commit_status("cancelled"), None);
        // `pending` is queued on this side, where a check run would be running.
        assert_eq!(
            CheckStatus::from_commit_status("pending"),
            CheckStatus::Queued
        );
        assert_eq!(
            CheckStatus::from_commit_status("success"),
            CheckStatus::Completed
        );
    }

    #[test]
    fn an_approval_blocker_reads_as_failed_but_is_not_handed_to_an_agent() {
        // The measured asymmetry between `isFailedCheck` and
        // `getBrokenChecks`. Losing it would have an agent "fix" a run that
        // is only waiting for somebody to press a button.
        let blocked = CheckConclusion::ActionRequired;
        assert!(blocked.reads_as_failed());
        assert!(!blocked.is_fixable_breakage());

        let checks = vec![
            run("lint", CheckConclusion::ActionRequired),
            run("test", CheckConclusion::Failure),
        ];
        assert_eq!(CheckTally::of(&checks).failing, 2);
        let broken: Vec<&str> = broken_checks(&checks)
            .iter()
            .map(|check| check.name.as_str())
            .collect();
        assert_eq!(broken, ["test"]);
    }

    #[test]
    fn failures_sort_first_and_equal_ranks_keep_their_order() {
        let mut checks = vec![
            run("ok", CheckConclusion::Success),
            run("skip", CheckConclusion::Skipped),
            run("late", CheckConclusion::TimedOut),
            run("dead", CheckConclusion::Failure),
            run("wait", CheckConclusion::Pending),
        ];
        sort_for_display(&mut checks);
        let names: Vec<&str> = checks.iter().map(|check| check.name.as_str()).collect();
        // `late` before `dead` — same rank 0, and the input order stands.
        assert_eq!(names, ["late", "dead", "wait", "skip", "ok"]);
    }

    #[test]
    fn the_tally_does_not_pretend_an_unknown_word_is_a_state() {
        let checks = vec![
            run("ok", CheckConclusion::Success),
            CheckRun {
                conclusion: None,
                status: CheckStatus::Completed,
                ..run("mystery", CheckConclusion::Success)
            },
        ];
        // `None` reads as pending everywhere, so it counts as pending — not as
        // a fourth silent bucket.
        let tally = CheckTally::of(&checks);
        assert_eq!((tally.passing, tally.failing, tally.pending), (1, 0, 1));
    }

    #[test]
    fn a_run_id_is_read_out_of_a_details_url() {
        assert_eq!(
            actions_run_id("https://github.com/o/r/actions/runs/1234567890"),
            Some(1234567890)
        );
        assert_eq!(
            actions_run_id("https://github.com/o/r/actions/runs/42/job/7"),
            Some(42)
        );
        // Not an Actions URL, and a segment that only looks like one.
        assert_eq!(actions_run_id("https://ci.example.com/build/9"), None);
        assert_eq!(
            actions_run_id("https://github.com/o/r/actions/runs/12ab"),
            None
        );
        assert_eq!(actions_run_id("https://github.com/o/r/actions/runs"), None);
    }

    #[test]
    fn a_row_identifies_itself_in_the_measured_order() {
        let mut check = CheckRun {
            name: "build".into(),
            status: CheckStatus::Completed,
            conclusion: Some(CheckConclusion::Success),
            url: Some("https://x/y".into()),
            check_run_id: Some(7),
            workflow_run_id: Some(9),
        };
        assert_eq!(check.identity_key(0), "check-run:7");
        check.check_run_id = None;
        assert_eq!(check.identity_key(0), "workflow-run:9");
        check.workflow_run_id = None;
        assert_eq!(check.identity_key(0), "url:https://x/y");
        check.url = None;
        // Two nameless twins stay two rows.
        assert_eq!(check.identity_key(3), "fallback:build:3");
        assert_ne!(check.identity_key(3), check.identity_key(4));
    }

    #[test]
    fn a_jobs_words_are_wider_than_a_check_runs() {
        // `failed` never appears on a check run and does on a job.
        assert!(job_state_failed(Some("failed")));
        assert!(job_state_failed(Some("startup_failure")));
        assert!(!job_state_failed(Some("success")));
        assert!(!job_state_failed(None));

        let job = CheckJob {
            name: "test".into(),
            status: Some("completed".into()),
            conclusion: Some("failure".into()),
            ..CheckJob::default()
        };
        // The conclusion wins over the status.
        assert_eq!(job.state(), Some("failure"));
        assert!(job.failed());
    }

    #[test]
    fn a_prompt_carries_only_the_last_measured_lines() {
        let log = (1..=200)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\r\n");
        let tail = log_tail_for_prompt(&log);
        assert!(!tail.contains('\r'), "CRLF survived into the prompt");
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), PROMPT_LOG_TAIL_LINES);
        assert_eq!(lines.first(), Some(&"line 51"));
        assert_eq!(lines.last(), Some(&"line 200"));

        // A short log travels whole.
        assert_eq!(log_tail_for_prompt("one\ntwo"), "one\ntwo");
    }
    /* ---- t-2733: the numbers table, the stack, and the mail line ---- */

    fn link(number: u64, head: &str, base: &str) -> StackPr {
        StackPr {
            number,
            head: head.into(),
            base: base.into(),
        }
    }

    /// Three reviews chained by `base` read head→base from the one the
    /// panel is standing on, and a review whose base names nothing in the
    /// set is an orphan — never guessed into the chain.
    #[test]
    fn three_prs_chained_by_base_read_head_to_base_and_a_broken_link_is_an_orphan() {
        let prs = [
            link(10, "feat/a", "main"),
            link(11, "feat/b", "feat/a"),
            link(12, "feat/c", "feat/b"),
            link(40, "hotfix/z", "release/9"),
        ];
        // Standing on the middle one: the child above it, itself, its parent.
        let order = stack_order(11, &prs);
        assert_eq!(order.chain, [12, 11, 10]);
        assert_eq!(order.orphans, [40]);
        // Standing on the top: the same chain, the same orphan.
        assert_eq!(stack_order(12, &prs).chain, [12, 11, 10]);
        // A review nobody in the set knows stands alone and everything else
        // is an orphan of it.
        let alone = stack_order(99, &prs);
        assert!(alone.chain.is_empty());
        assert_eq!(alone.orphans, [10, 11, 12, 40]);
        // A cycle cannot spin the walk forever.
        let looped = [link(1, "a", "b"), link(2, "b", "a")];
        assert_eq!(stack_order(1, &looped).chain.len(), 2);
    }

    /// The overlay changes only what the table names, and only through the
    /// prefixed key — a bare `poll_ms` from some other feature is not ours.
    #[test]
    fn the_overlay_only_changes_what_the_table_names() {
        let overlay: BTreeMap<String, u64> = [
            (format!("{OVERLAY_PREFIX}poll_ms"), 9_000),
            ("poll_ms".to_string(), 1),
            (format!("{OVERLAY_PREFIX}unknown"), 5),
            (format!("{OVERLAY_PREFIX}diff_cache_bytes"), 1024),
            (format!("{OVERLAY_PREFIX}gh_call_ms"), 2_500),
        ]
        .into_iter()
        .collect();
        let limits = Limits::default().overlaid(&overlay);
        assert_eq!(limits.poll_ms, 9_000);
        assert_eq!(limits.diff_cache_bytes, 1024);
        assert_eq!(limits.gh_call_ms, 2_500);
        assert_eq!(limits.files_max, Limits::default().files_max);
        // A poll below the floor is the floor: zero would spin the panel — and
        // a zero call budget would refuse every `gh` call before it started.
        let floor: BTreeMap<String, u64> = [
            (format!("{OVERLAY_PREFIX}poll_ms"), 0),
            (format!("{OVERLAY_PREFIX}gh_call_ms"), 0),
        ]
        .into_iter()
        .collect();
        let floored = Limits::default().overlaid(&floor);
        assert_eq!(floored.poll_ms, Limits::default().poll_ms_min);
        assert_eq!(floored.gh_call_ms, Limits::default().poll_ms_min);
    }

    /// The fingerprint moves when a check moves — even when the tally does
    /// not — and the mail line names what failed.
    #[test]
    fn the_fingerprint_moves_with_a_check_and_the_mail_line_names_the_failures() {
        let before = vec![
            run("ci/test", CheckConclusion::Failure),
            run("ci/lint", CheckConclusion::Success),
        ];
        let after = vec![
            run("ci/test", CheckConclusion::Success),
            run("ci/lint", CheckConclusion::Failure),
        ];
        // Same tally, different world.
        assert_eq!(CheckTally::of(&before), CheckTally::of(&after));
        assert_ne!(checks_fingerprint(&before), checks_fingerprint(&after));
        assert_eq!(
            checks_fingerprint(&before),
            checks_fingerprint(&before.clone())
        );
        assert_eq!(
            checks_mail_line(&before),
            "checks: 1 failed (ci/test), 1 passed"
        );
        let waiting = vec![run("ci/test", CheckConclusion::Pending)];
        assert_eq!(checks_mail_line(&waiting), "checks: 1 pending");
        assert_eq!(checks_mail_line(&[]), "checks: none");
    }
}
