//! The life of a teammate pane after its first turn (t-2513 §2.2).
//!
//! A v1 teammate answered one turn and left, because its parent had no way
//! to say anything else to it. Now the parent does — `session.steer` on the
//! child's own events channel — so the child stays: first turn, `result.json`,
//! then IDLE until one of four things ends it.
//!
//! | end | who decides | `result-final.json.reason` |
//! |---|---|---|
//! | `teammate.close` over the channel | the parent | `closed_by_parent` |
//! | the parent's channel gone past `parent_liveness_grace` | the parent's death | `parent_lost` |
//! | `idle_budget` since the last turn ended | the table ([`Limits`]) | `idle_budget` |
//! | Esc twice / Ctrl-D at the child's keyboard | a person | `user_exit` |
//!
//! Zombie prevention is therefore parent-liveness plus a budget, not "leave
//! after one turn". The pieces here are the pure ones — the clocks, the
//! liveness verdict, the files — so `tui::app::drive_teammate` is only the
//! loop that asks them, and a test can turn each clock by hand.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use runtime::subagent_panes::{
    parent_channel_alive, Brief, ChannelCoordinates, CloseReason, Exit, Limits, TeammateResult,
    Usage, CHANNEL_FILE,
};

/// What a teammate's loop was given: where it writes, who it is, whom it
/// watches, and how long it waits.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    /// The child's directory — brief, results, channel file.
    pub directory: PathBuf,
    pub agent_id: String,
    /// The parent's discovery file. `None` for a parent with no channel (or a
    /// v1 brief): then only the idle budget and the person can end the child.
    pub parent_channel: Option<PathBuf>,
    pub limits: Limits,
    /// The brief's own idle budget, or the table's.
    pub idle_budget: Duration,
    /// The number of the first turn this child answers.
    pub first_turn: u32,
}

impl Lifecycle {
    /// From a brief and the table this process loaded.
    #[must_use]
    pub fn from_brief(directory: &Path, brief: &Brief, limits: Limits) -> Self {
        Self {
            directory: directory.to_path_buf(),
            agent_id: brief.agent_id.clone(),
            parent_channel: brief.parent_channel.clone(),
            idle_budget: brief
                .idle_budget_ms
                .map_or(limits.idle_budget, Duration::from_millis),
            limits,
            first_turn: brief.first_turn(),
        }
    }

    /// Copy this child's channel coordinates beside its brief, where the
    /// parent can find them without knowing the child's pid.
    ///
    /// # Errors
    ///
    /// The discovery file cannot be read or the copy cannot be written.
    pub fn publish_channel(&self, discovery_file: &Path) -> Result<PathBuf, String> {
        let coordinates = ChannelCoordinates::read(discovery_file)?;
        let path = self.directory.join(CHANNEL_FILE);
        coordinates
            .write(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(path)
    }

    /// Remove the channel copy — the child is leaving, and a parent that
    /// found the file would otherwise try a door with nobody behind it.
    pub fn retire_channel(&self) {
        let _ = std::fs::remove_file(self.directory.join(CHANNEL_FILE));
    }
}

/// What one turn came to, as the loop hands it over for `result-<n>.json`.
#[derive(Debug, Clone, Default)]
pub struct TurnReport {
    pub answer: String,
    pub cancelled: bool,
    pub error: Option<String>,
    pub output_tokens: u64,
    pub tool_calls: u64,
}

/// The result document for turn `turn`.
#[must_use]
pub fn result_for_turn(
    agent_id: &str,
    turn: u32,
    report: &TurnReport,
    transcript: &Path,
) -> TeammateResult {
    let mut result = TeammateResult::new(
        agent_id,
        if report.cancelled {
            Exit::Cancelled
        } else if report.error.is_some() {
            Exit::Error
        } else {
            Exit::Ok
        },
    );
    result.final_message.clone_from(&report.answer);
    result.error.clone_from(&report.error);
    result.usage = Usage {
        output_tokens: report.output_tokens,
        tool_calls: report.tool_calls,
    };
    result.transcript = Some(transcript.to_path_buf());
    result.turn = Some(turn);
    result
}

/// The idle clock: how long since the last turn ended, against the budget.
#[derive(Debug, Clone, Copy)]
pub struct IdleClock {
    since: Instant,
    budget: Duration,
}

impl IdleClock {
    #[must_use]
    pub fn start(budget: Duration) -> Self {
        Self {
            since: Instant::now(),
            budget,
        }
    }

    /// A turn ended: the budget starts over.
    pub fn reset(&mut self) {
        self.since = Instant::now();
    }

    /// When the budget runs out, as an instant a `select!` can sleep until.
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.since + self.budget
    }

    #[must_use]
    pub fn expired_at(&self, now: Instant) -> bool {
        now >= self.deadline()
    }
}

/// The parent-liveness verdict, from what a poll saw and when.
///
/// A parent's channel that is gone once is not a dead parent — a restart, a
/// slow disk, a busy accept loop — so the child waits out
/// `parent_liveness_grace` before it believes it. The verdict is pure: the
/// loop feeds it observations, and a test feeds it a clock.
#[derive(Debug, Clone, Copy)]
pub struct ParentWatch {
    grace: Duration,
    lost_since: Option<Instant>,
}

impl ParentWatch {
    #[must_use]
    pub fn new(grace: Duration) -> Self {
        Self {
            grace,
            lost_since: None,
        }
    }

    /// Record one observation at `now`. Answers whether the parent has now
    /// been gone past the grace.
    pub fn observe(&mut self, alive: bool, now: Instant) -> bool {
        if alive {
            self.lost_since = None;
            return false;
        }
        let since = *self.lost_since.get_or_insert(now);
        now.saturating_duration_since(since) >= self.grace
    }

    /// The blocking probe itself: is anybody answering at the parent's
    /// discovery file? Run it off the UI thread — a dead parent costs the
    /// connect timeout.
    #[must_use]
    pub fn probe(discovery: &Path, timeout: Duration) -> bool {
        parent_channel_alive(discovery, timeout)
    }
}

/// The closing document, written last.
pub fn write_closing(lifecycle: &Lifecycle, reason: CloseReason, turns_answered: u32) -> std::io::Result<PathBuf> {
    let mut closing = TeammateResult::closed(&lifecycle.agent_id, reason);
    closing.turn = Some(turns_answered);
    closing.write_final(&lifecycle.directory)
}

/// Read a parent's word for why it closed the child back into the reason
/// vocabulary; an unknown word is the parent's decision all the same.
#[must_use]
pub fn close_reason_from(word: &str) -> CloseReason {
    match word.trim() {
        "parent_lost" => CloseReason::ParentLost,
        "idle_budget" => CloseReason::IdleBudget,
        "user_exit" => CloseReason::UserExit,
        "lane_done" => CloseReason::LaneDone,
        _ => CloseReason::ClosedByParent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lifecycle_takes_the_briefs_budget_over_the_table_and_the_table_when_absent() {
        let limits = Limits {
            idle_budget: Duration::from_secs(600),
            ..Limits::default()
        };
        let brief = Brief {
            agent_id: "agent-7".to_string(),
            idle_budget_ms: Some(1_500),
            parent_channel: Some(PathBuf::from("/tmp/zo-events-1.addr")),
            first_turn: Some(4),
            ..Brief::default()
        };
        let lifecycle = Lifecycle::from_brief(Path::new("/tmp/agent-7"), &brief, limits);
        assert_eq!(lifecycle.idle_budget, Duration::from_millis(1_500));
        assert_eq!(lifecycle.first_turn, 4);
        assert_eq!(lifecycle.parent_channel.as_deref(), Some(Path::new("/tmp/zo-events-1.addr")));
        let bare = Lifecycle::from_brief(Path::new("/tmp/agent-7"), &Brief::default(), limits);
        assert_eq!(bare.idle_budget, Duration::from_secs(600));
        assert_eq!(bare.first_turn, 1);
        assert!(bare.parent_channel.is_none());
    }

    #[test]
    fn a_turn_report_becomes_the_turns_result_document() {
        let report = TurnReport {
            answer: "three edges".to_string(),
            cancelled: false,
            error: None,
            output_tokens: 12,
            tool_calls: 3,
        };
        let result = result_for_turn("agent-7", 2, &report, Path::new("/tmp/s.jsonl"));
        assert_eq!(result.exit, Exit::Ok);
        assert_eq!(result.turn, Some(2));
        assert_eq!(result.final_message, "three edges");
        assert_eq!(result.usage.tool_calls, 3);
        assert_eq!(result.transcript.as_deref(), Some(Path::new("/tmp/s.jsonl")));
        let cancelled = result_for_turn(
            "agent-7",
            3,
            &TurnReport {
                cancelled: true,
                ..TurnReport::default()
            },
            Path::new("/tmp/s.jsonl"),
        );
        assert_eq!(cancelled.exit, Exit::Cancelled);
        let failed = result_for_turn(
            "agent-7",
            3,
            &TurnReport {
                error: Some("boom".to_string()),
                ..TurnReport::default()
            },
            Path::new("/tmp/s.jsonl"),
        );
        assert_eq!(failed.exit, Exit::Error);
    }

    /// One missed poll is not a dead parent; the grace is.
    #[test]
    fn the_parent_watch_believes_a_loss_only_past_the_grace() {
        let start = Instant::now();
        let mut watch = ParentWatch::new(Duration::from_secs(10));
        assert!(!watch.observe(true, start));
        assert!(!watch.observe(false, start + Duration::from_secs(1)));
        assert!(!watch.observe(false, start + Duration::from_secs(9)));
        // The parent came back: the clock resets.
        assert!(!watch.observe(true, start + Duration::from_secs(10)));
        assert!(!watch.observe(false, start + Duration::from_secs(11)));
        assert!(!watch.observe(false, start + Duration::from_secs(20)));
        assert!(watch.observe(false, start + Duration::from_secs(21)));
    }

    #[test]
    fn the_idle_clock_resets_on_a_turn_and_expires_on_the_budget() {
        let born = Instant::now();
        let mut clock = IdleClock::start(Duration::from_secs(30));
        assert!(!clock.expired_at(born + Duration::from_secs(29)));
        assert!(clock.expired_at(born + Duration::from_secs(31)));
        // A turn ended: the budget starts over from NOW, whatever `born` was.
        std::thread::sleep(Duration::from_millis(5));
        clock.reset();
        let reset_at = Instant::now();
        assert!(clock.deadline() > born + Duration::from_secs(30), "the reset did not move the deadline");
        assert!(!clock.expired_at(reset_at + Duration::from_secs(29)));
        assert!(clock.expired_at(reset_at + Duration::from_secs(31)));
    }

    #[test]
    fn the_closing_document_and_the_channel_copy_live_beside_the_brief() {
        let directory = tempfile::tempdir().expect("tempdir");
        let lifecycle = Lifecycle::from_brief(
            directory.path(),
            &Brief {
                agent_id: "agent-7".to_string(),
                ..Brief::default()
            },
            Limits::default(),
        );
        let discovery = directory.path().join("zo-events-9.addr");
        std::fs::write(&discovery, "127.0.0.1:4321\ntok\nsession-child\n").unwrap();
        let channel = lifecycle.publish_channel(&discovery).expect("publish");
        assert_eq!(channel, directory.path().join(CHANNEL_FILE));
        let copied = ChannelCoordinates::read(&channel).expect("read copy");
        assert_eq!(copied.addr, "127.0.0.1:4321");
        assert_eq!(copied.token.as_deref(), Some("tok"));
        assert_eq!(copied.session_id, "session-child");
        write_closing(&lifecycle, CloseReason::IdleBudget, 2).expect("write closing");
        let closing = TeammateResult::read_final(directory.path()).expect("closing");
        assert_eq!(closing.exit, Exit::Closed);
        assert_eq!(closing.reason, Some(CloseReason::IdleBudget));
        assert_eq!(closing.turn, Some(2));
        lifecycle.retire_channel();
        assert!(!channel.exists());
        assert_eq!(close_reason_from("parent_lost"), CloseReason::ParentLost);
        assert_eq!(close_reason_from("lane_done"), CloseReason::LaneDone);
        assert_eq!(close_reason_from("whatever"), CloseReason::ClosedByParent);
    }
}
