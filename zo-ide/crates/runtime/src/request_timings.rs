//! Per-request timing ledger — where a turn's wait actually went.
//!
//! The first-token axis of the scoreboard is read off pty captures in a
//! bench; a live session has nothing that says whether a slow first token
//! was spent before the request left (assembly, recall, a routing probe), on
//! the wire before the first byte, or in the model before the first visible
//! block. This ledger writes one line per provider request into
//! `<project state>/request-timings/timings.jsonl` with those three waits
//! split, plus the attempt count, so the question can be answered from a
//! file after the fact — the same discipline as the prompt-cache break ledger
//! (`crate::prompt_cache_breaks`): serialize first, one `write_all` per line,
//! and a reader that drops a torn line instead of failing.
//!
//! The stamps come from the probe channel the async client mirrors its
//! blocks through: `StreamPhase::RequestSent` opens an attempt, the first
//! block after it is the first byte, and the first text/reasoning/tool block
//! is the first thing a person could see. The runtime's own clock, not the
//! provider's, so the numbers match what the screen showed.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::message_stream::types::{RenderBlock, StreamPhase};
#[cfg(test)]
use crate::message_stream::types::BlockId;

const TIMINGS_DIR: &str = "request-timings";
const TIMINGS_FILE: &str = "timings.jsonl";

/// The stamps one request collects while its stream is in flight. Monotonic
/// instants; converted to millisecond waits only when the record is written.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamStamps {
    /// When the runtime began assembling this request (before recall,
    /// reminders and the wire build).
    pub assemble_started: Option<Instant>,
    /// When the LAST attempt left — a retry re-opens the attempt, so the
    /// waits below describe the attempt that produced the answer.
    pub sent: Option<Instant>,
    /// First block of any kind after the request left.
    pub first_byte: Option<Instant>,
    /// First text, reasoning or tool-call block — the first visible content.
    pub first_visible: Option<Instant>,
    /// When the stream future resolved.
    pub completed: Option<Instant>,
    /// How many times the request was sent (1 for a clean run).
    pub attempts: u32,
}

impl StreamStamps {
    /// A request whose assembly began at `assemble_started`.
    #[must_use]
    pub fn begun(assemble_started: Instant) -> Self {
        Self {
            assemble_started: Some(assemble_started),
            ..Self::default()
        }
    }

    /// Observe a block on the probe channel, now.
    pub fn observe(&mut self, block: &RenderBlock) {
        self.observe_at(block, Instant::now());
    }

    /// Observe a block on the probe channel at `now`. `RequestSent` opens (or,
    /// on a retry, re-opens) the attempt; other phase blocks are the status
    /// line's own and stamp nothing; every provider block is the first byte
    /// once, and text, reasoning and tool calls are the first visible content
    /// once.
    pub fn observe_at(&mut self, block: &RenderBlock, now: Instant) {
        match block {
            RenderBlock::StreamPhase(StreamPhase::RequestSent { .. }) => {
                self.sent = Some(now);
                self.first_byte = None;
                self.first_visible = None;
                self.attempts = self.attempts.saturating_add(1);
            }
            RenderBlock::StreamPhase(_) => {}
            RenderBlock::TextDelta { .. } | RenderBlock::Reasoning { .. } | RenderBlock::ToolCall { .. } => {
                self.first_byte.get_or_insert(now);
                self.first_visible.get_or_insert(now);
            }
            _ => {
                self.first_byte.get_or_insert(now);
            }
        }
    }

    /// The stream future resolved, now.
    pub fn complete(&mut self) {
        self.complete_at(Instant::now());
    }

    /// The stream future resolved at `now`.
    pub fn complete_at(&mut self, now: Instant) {
        self.completed = Some(now);
    }

    /// Whether the request left at all — a legacy collect-then-replay client
    /// mirrors no blocks, and then there is nothing to record.
    #[must_use]
    pub fn left_the_runtime(&self) -> bool {
        self.sent.is_some()
    }
}

/// One line of the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestTimingRecord {
    /// Unix seconds when the record was written.
    pub recorded_at: u64,
    pub session_id: String,
    /// The wire model the request went to, when the runtime knew it.
    pub model: Option<String>,
    /// 1-based iteration within the turn.
    pub iteration: usize,
    /// Messages in the request.
    pub request_messages: usize,
    /// Times the request was sent.
    pub attempts: u32,
    /// Assembly began → last attempt left.
    pub assemble_ms: Option<u64>,
    /// Last attempt left → first block.
    pub ttfb_ms: Option<u64>,
    /// Last attempt left → first visible block.
    pub ttfv_ms: Option<u64>,
    /// Last attempt left → stream resolved.
    pub stream_ms: Option<u64>,
    /// How the request ended, as the turn loop named it.
    pub outcome: String,
    /// The ATTEMPT this request was spent on
    /// (`crate::model_router::attempt`) — the one key this ledger shares with
    /// `requests.jsonl` and `route-outcomes.jsonl`, so a turn's waits can be
    /// put next to its tokens and its verdict without inferring the pairing
    /// from timestamps. `<sessionId>@<turnOrdinal>` on a main-session turn,
    /// `<agentId>#<runGeneration>` inside a spawned agent. Empty on every row
    /// written before this column and wherever no attempt was declared.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attempt: String,
}

/// The turn context a record carries next to its waits.
#[derive(Debug, Clone, Copy)]
pub struct TimingContext<'a> {
    pub session_id: &'a str,
    pub model: Option<&'a str>,
    pub iteration: usize,
    pub request_messages: usize,
    pub recorded_at: u64,
    pub outcome: &'a str,
    /// The attempt the turn loop is on — see [`RequestTimingRecord::attempt`].
    pub attempt: &'a str,
}

fn millis_between(from: Option<Instant>, to: Option<Instant>) -> Option<u64> {
    let (from, to) = (from?, to?);
    Some(u64::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX))
}

/// Fold the stamps into a record.
#[must_use]
pub fn timing_record(context: &TimingContext<'_>, stamps: &StreamStamps) -> RequestTimingRecord {
    RequestTimingRecord {
        recorded_at: context.recorded_at,
        session_id: context.session_id.to_string(),
        model: context.model.map(str::to_string),
        iteration: context.iteration,
        request_messages: context.request_messages,
        attempts: stamps.attempts,
        assemble_ms: millis_between(stamps.assemble_started, stamps.sent),
        ttfb_ms: millis_between(stamps.sent, stamps.first_byte),
        ttfv_ms: millis_between(stamps.sent, stamps.first_visible),
        stream_ms: millis_between(stamps.sent, stamps.completed),
        outcome: context.outcome.to_string(),
        attempt: context.attempt.to_string(),
    }
}

/// Where a project's ledger lives.
#[must_use]
pub fn request_timings_path(cwd: &Path) -> PathBuf {
    crate::zo_project_state_dir(cwd)
        .join(TIMINGS_DIR)
        .join(TIMINGS_FILE)
}

/// Append one record to the project's ledger.
pub fn record_request_timing(cwd: &Path, record: &RequestTimingRecord) -> io::Result<()> {
    record_request_timing_at_path(&request_timings_path(cwd), record)
}

/// Append one record at an explicit path — one serialized line, one write.
pub fn record_request_timing_at_path(path: &Path, record: &RequestTimingRecord) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

/// Read a project's ledger, dropping any line that does not parse.
pub fn read_request_timings(cwd: &Path) -> io::Result<Vec<RequestTimingRecord>> {
    read_request_timings_at_path(&request_timings_path(cwd))
}

/// Read a ledger at an explicit path; a missing file is an empty ledger.
pub fn read_request_timings_at_path(path: &Path) -> io::Result<Vec<RequestTimingRecord>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_stream::types::SystemLevel;
    use std::time::Duration;

    fn context(outcome: &str) -> TimingContext<'_> {
        TimingContext {
            session_id: "session-1",
            model: Some("claude-opus-5"),
            iteration: 1,
            request_messages: 3,
            recorded_at: 1_789_000_000,
            outcome,
            attempt: "session-1@4",
        }
    }

    fn sent(attempt: u32) -> RenderBlock {
        RenderBlock::StreamPhase(StreamPhase::RequestSent { attempt })
    }

    fn system() -> RenderBlock {
        RenderBlock::System {
            id: BlockId(1),
            level: SystemLevel::Info,
            text: "hello".to_string(),
        }
    }

    /// The attempt is the key this ledger shares with `requests.jsonl` and
    /// `route-outcomes.jsonl`. Without it a turn's waits cannot be put beside
    /// its tokens, which is the whole reason the column exists.
    #[test]
    fn a_record_carries_the_attempt_its_turn_declared() {
        let t0 = Instant::now();
        let mut stamps = StreamStamps::begun(t0);
        stamps.observe_at(&sent(1), t0 + Duration::from_millis(5));
        stamps.complete_at(t0 + Duration::from_millis(50));

        let record = timing_record(&context("completed"), &stamps);
        assert_eq!(record.attempt, "session-1@4");
    }

    /// Every row written before this column parses, and an empty attempt is
    /// not spelled onto the wire.
    #[test]
    fn a_row_without_an_attempt_still_parses_and_stays_off_the_wire() {
        let old_line = r#"{"recorded_at":1789000000,"session_id":"s","model":null,"iteration":1,"request_messages":2,"attempts":1,"assemble_ms":1,"ttfb_ms":2,"ttfv_ms":3,"stream_ms":4,"outcome":"completed"}"#;
        let parsed: RequestTimingRecord =
            serde_json::from_str(old_line).expect("a pre-attempt row must still parse");
        assert_eq!(parsed.attempt, "");
        let written = serde_json::to_string(&parsed).expect("serialize");
        assert!(
            // `"attempts"` — the retry count — is a different column whose
            // name this one is a prefix of.
            !written.contains(r#""attempt":"#),
            "an empty attempt costs no bytes on a file kept forever: {written}"
        );
    }

    /// The three waits are read off the probe channel in order: the phase
    /// block opens the attempt, the first block of any kind is the first byte,
    /// the first text is the first visible content, and the resolved future
    /// closes the stream.
    #[test]
    fn the_waits_split_at_sent_first_byte_first_visible_and_done() {
        let t0 = Instant::now();
        let mut stamps = StreamStamps::begun(t0);
        stamps.observe_at(&sent(1), t0 + Duration::from_millis(30));
        stamps.observe_at(&system(), t0 + Duration::from_millis(130));
        stamps.observe_at(
            &RenderBlock::TextDelta {
                id: BlockId(7),
                text: "I".to_string(),
                done: false,
            },
            t0 + Duration::from_millis(430),
        );
        stamps.observe_at(
            &RenderBlock::TextDelta {
                id: BlockId(7),
                text: "'ll".to_string(),
                done: false,
            },
            t0 + Duration::from_millis(500),
        );
        stamps.complete_at(t0 + Duration::from_millis(2_030));
        let record = timing_record(&context("completed"), &stamps);
        assert_eq!(record.attempts, 1);
        assert_eq!(record.assemble_ms, Some(30));
        assert_eq!(record.ttfb_ms, Some(100));
        assert_eq!(record.ttfv_ms, Some(400));
        assert_eq!(record.stream_ms, Some(2_000));
        assert_eq!(record.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(record.outcome, "completed");
    }

    /// A retry re-opens the attempt: the waits describe the attempt that
    /// answered, and the count says how many it took. The status line's own
    /// phase blocks stamp nothing.
    #[test]
    fn a_retry_reopens_the_attempt_and_phase_blocks_stamp_nothing() {
        let t0 = Instant::now();
        let mut stamps = StreamStamps::begun(t0);
        stamps.observe_at(&sent(1), t0 + Duration::from_millis(10));
        stamps.observe_at(
            &RenderBlock::StreamPhase(StreamPhase::Retrying {
                attempt: 1,
                delay_secs: 2,
            }),
            t0 + Duration::from_millis(1_010),
        );
        stamps.observe_at(&sent(2), t0 + Duration::from_millis(3_010));
        stamps.observe_at(&system(), t0 + Duration::from_millis(3_210));
        stamps.complete_at(t0 + Duration::from_millis(4_010));
        let record = timing_record(&context("completed"), &stamps);
        assert_eq!(record.attempts, 2);
        assert_eq!(record.assemble_ms, Some(3_010));
        assert_eq!(record.ttfb_ms, Some(200));
        assert_eq!(record.ttfv_ms, None);
        assert_eq!(record.stream_ms, Some(1_000));
    }

    /// A client that mirrors no blocks never says the request left; the
    /// caller records nothing rather than a line of `None`s.
    #[test]
    fn a_request_that_never_left_is_not_a_record() {
        let stamps = StreamStamps::begun(Instant::now());
        assert!(!stamps.left_the_runtime());
    }

    #[test]
    fn the_ledger_round_trips_and_drops_a_torn_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("request-timings").join("timings.jsonl");
        let mut stamps = StreamStamps::begun(Instant::now());
        stamps.observe(&sent(1));
        stamps.observe(&system());
        stamps.complete();
        let record = timing_record(&context("completed"), &stamps);
        record_request_timing_at_path(&path, &record).unwrap();
        record_request_timing_at_path(&path, &record).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"torn\":\n")
            .unwrap();
        let read = read_request_timings_at_path(&path).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0], record);
        assert!(read_request_timings_at_path(&dir.path().join("absent.jsonl"))
            .unwrap()
            .is_empty());
    }
}
