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

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::message_stream::types::{RenderBlock, StreamPhase, ToolCallStatus};
#[cfg(test)]
use crate::message_stream::types::BlockId;

const TIMINGS_DIR: &str = "request-timings";
const TIMINGS_FILE: &str = "timings.jsonl";

/// The rows a ledger keeps, newest last: as many as the per-turn trace keeps
/// lines (`crate::turn_trace`) — the same retention for the same kind of
/// per-project evidence file (t-80327).
const RETAINED_ROWS: usize = crate::turn_trace::MAX_TURN_LOG_LINES;

/// The rows this process appended to each ledger since it last cut the
/// ledger back to its newest [`RETAINED_ROWS`].
static APPENDED_SINCE_CUT: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());

/// The stamps one request collects while its stream is in flight. Monotonic
/// instants; converted to millisecond waits only when the record is written.
#[derive(Debug, Clone, Default)]
pub struct StreamStamps {
    /// When the turn this request belongs to began: the runtime took the
    /// person's words, before the turn-start hooks and judgments. `None` on a
    /// later leg's request — see [`Self::leg`].
    pub turn_started: Option<Instant>,
    /// The leg of the turn this request belongs to, from the second on — a
    /// deep-mode turn runs its legs inside one taking of the person's words.
    pub leg: Option<u32>,
    /// When the turn-start judgments began holding this request, and when
    /// they let it go — the turn's first request only.
    pub judge_wait: Option<Range<Instant>>,
    /// When the compaction preflight in front of this request began and
    /// ended — the turn's first request only.
    pub preflight: Option<Range<Instant>>,
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
    /// When each tool block of the answer closed — its input complete — by
    /// the call's id and the tool's name, in the order they closed.
    tool_closed: Vec<(String, String, Instant)>,
    /// The tools whose results this request carries back, and how long each
    /// ran.
    pub tools_ran: ToolRuns,
}

/// One tool on a row, by name only — never its input or its result — with a
/// time in ms, absent for a tool that did not run to its end (t-78938).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMark {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    /// The person stopped the tool while it ran (Esc): it began but did not
    /// finish, so it keeps no time — the wait up to the keypress is not how
    /// long the tool takes (t-80327).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancelled: bool,
}

/// The tools one answer asked for, in the order asked, each with how long it
/// ran: what the request that carries their results back says of them
/// (t-78938). A tool that did not run — refused, or answered by the
/// repetition guard — keeps no time, and neither does one the person
/// stopped while it ran, which is marked `cancelled` (t-80327).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRuns {
    marks: Vec<ToolMark>,
}

impl ToolRuns {
    /// The tools an answer asked for, by name, none of them run yet.
    #[must_use]
    pub fn asked<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            marks: names
                .into_iter()
                .map(|name| ToolMark { name: name.to_string(), ms: None, cancelled: false })
                .collect(),
        }
    }

    /// The `index`th tool asked for ran from `started` to `finished`.
    pub fn ran(&mut self, index: usize, started: Instant, finished: Instant) {
        if let Some(mark) = self.marks.get_mut(index) {
            mark.ms = millis_between(Some(started), Some(finished));
        }
    }

    /// The `index`th tool asked for began and the person stopped it before
    /// it finished: it keeps no time.
    pub fn stopped(&mut self, index: usize) {
        if let Some(mark) = self.marks.get_mut(index) {
            mark.ms = None;
            mark.cancelled = true;
        }
    }
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

    /// A request of the turn `clock` keeps, whose assembly began at
    /// `assemble_started` after the compaction `preflight`, when one ran in
    /// front of it. The judgments' wait goes with the first request that
    /// takes it off the clock: only the turn's first request waited for them.
    /// A request of a later leg names its leg instead of the turn's start:
    /// the legs before it ran in between (t-80327).
    #[must_use]
    pub fn in_turn(assemble_started: Instant, clock: &mut TurnClock, preflight: Option<Range<Instant>>) -> Self {
        let later_leg = clock.legs > 1;
        Self {
            turn_started: if later_leg { None } else { clock.started },
            leg: later_leg.then_some(clock.legs),
            judge_wait: clock.judge_wait.take(),
            preflight,
            ..Self::begun(assemble_started)
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
    /// once; a tool call announced `Running` is its block's close, once per
    /// call, and a re-opened attempt forgets the closes of the one before.
    pub fn observe_at(&mut self, block: &RenderBlock, now: Instant) {
        match block {
            RenderBlock::StreamPhase(StreamPhase::RequestSent { .. }) => {
                self.sent = Some(now);
                self.first_byte = None;
                self.first_visible = None;
                self.tool_closed.clear();
                self.attempts = self.attempts.saturating_add(1);
            }
            RenderBlock::StreamPhase(_) => {}
            RenderBlock::TextDelta { .. } | RenderBlock::Reasoning { .. } => {
                self.first_byte.get_or_insert(now);
                self.first_visible.get_or_insert(now);
            }
            RenderBlock::ToolCall { tool_call_id, name, status, .. } => {
                self.first_byte.get_or_insert(now);
                self.first_visible.get_or_insert(now);
                // A call is announced `Running` once its input is complete:
                // its block has closed, and its tool could have started.
                if *status == ToolCallStatus::Running && !self.tool_closed.iter().any(|(id, ..)| *id == tool_call_id.0) {
                    self.tool_closed.push((tool_call_id.0.clone(), name.clone(), now));
                }
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

/// What a turn stamps before any of its requests is assembled: when it
/// began, and how long its turn-start judgments held it — the waits in front
/// of its first request that the request's own stamps begin too late to see
/// (t-78938).
#[derive(Debug, Clone, Default)]
pub struct TurnClock {
    /// When the runtime took the person's words.
    pub started: Option<Instant>,
    /// When the turn-start judgments began holding the turn's first
    /// request, and when they let it go; `None` when none was asked.
    pub judge_wait: Option<Range<Instant>>,
    /// How many legs of the turn have begun: a plain turn is one leg, and a
    /// deep-mode turn runs one leg after another — each starting its
    /// iterations over — inside one taking of the person's words (t-80327).
    pub legs: u32,
}

impl TurnClock {
    /// A turn that began at `now`, before its first leg.
    #[must_use]
    pub fn began(now: Instant) -> Self {
        Self { started: Some(now), judge_wait: None, legs: 0 }
    }

    /// Another leg of the turn began.
    pub fn leg_began(&mut self) {
        self.legs = self.legs.saturating_add(1);
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
    /// Assembly began → last attempt left. Takes in the recall seat's wait
    /// when the seat is waited for; a project whose last waited judgment
    /// missed the wall takes the record-only road with no wait until a
    /// recorded judgment answers inside it, a chance about once per turn
    /// since t-78938 rather than once per request (t-80327).
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
    /// The turn began → last attempt left: the whole wait from the person's
    /// words to this request, of which `judge_wait_ms`, `preflight_ms` and
    /// `assemble_ms` are parts. Absent on every row written before this
    /// column, on a request no turn start was stamped for, and on a later
    /// leg's request (`leg`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_start_ms: Option<u64>,
    /// The leg of the turn this request belongs to, from the second on. A
    /// deep-mode turn takes the person's words once and runs its legs (PLAN,
    /// EXEC, verify) one after another, each starting `iteration` over at 1;
    /// a later leg's rows carry no `turn_start_ms`, which would hold every
    /// leg before it, so "the words to the first send" is read off the rows
    /// with `iteration` 1 and no `leg` (t-80327). Absent on a turn's first
    /// leg and on every row written before this column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leg: Option<u32>,
    /// How long the turn-start judgments held this request — the turn's
    /// first request only, and only when one was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_wait_ms: Option<u64>,
    /// How long the compaction preflight in front of this request took —
    /// the turn's first request only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_ms: Option<u64>,
    /// When each tool block of this answer closed — its input complete, the
    /// moment its tool could have started — counted from the last attempt's
    /// send, in the order they closed, by tool name only. Absent when the
    /// answer called no tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_closed_ms: Vec<ToolMark>,
    /// How long each tool whose result this request carries back ran, in the
    /// order the answer before it asked for them, by tool name only; a tool
    /// that did not run has no `ms`, and one the person stopped while it ran
    /// has none either and says `"cancelled": true`. Read beside the row of
    /// the answer that asked for them — the same `attempt`, the iteration
    /// before — so a row with no attempt to be joined by never carries it.
    /// Only a request that is sent and completes carries them: when the turn
    /// ends after its tools — a hard stop, the iteration limit, Esc, a next
    /// request that fails — the last tools' times are on no row, so a week's
    /// figures lean away from the tools that end turns (t-80327).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_ran_ms: Vec<ToolMark>,
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

fn span_millis(span: Option<&Range<Instant>>) -> Option<u64> {
    span.and_then(|span| millis_between(Some(span.start), Some(span.end)))
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
        turn_start_ms: millis_between(stamps.turn_started, stamps.sent),
        leg: stamps.leg,
        judge_wait_ms: span_millis(stamps.judge_wait.as_ref()),
        preflight_ms: span_millis(stamps.preflight.as_ref()),
        tool_closed_ms: stamps
            .tool_closed
            .iter()
            .map(|(_, name, closed)| ToolMark {
                name: name.clone(),
                ms: millis_between(stamps.sent, Some(*closed)),
                cancelled: false,
            })
            .collect(),
        tool_ran_ms: if context.attempt.is_empty() { Vec::new() } else { stamps.tools_ran.marks.clone() },
    }
}

#[cfg(test)]
thread_local! {
    /// The stamps of each request this thread's turns would have written a
    /// row for. A test runtime arms no durable traces and writes no row, so a
    /// test reads them back here ([`take_recorded`]).
    static RECORDED: std::cell::RefCell<Vec<StreamStamps>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Keep `stamps` for [`take_recorded`] when the ledger would write a row for
/// them.
#[cfg(test)]
pub(crate) fn note_recorded(stamps: &StreamStamps) {
    if stamps.left_the_runtime() {
        RECORDED.with(|recorded| recorded.borrow_mut().push(stamps.clone()));
    }
}

/// The stamps this thread's turns recorded since the last call, oldest first.
#[cfg(test)]
pub(crate) fn take_recorded() -> Vec<StreamStamps> {
    RECORDED.with(std::cell::RefCell::take)
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

/// Append one record at an explicit path — one serialized line, one write —
/// and cut the ledger back to its newest [`RETAINED_ROWS`] when a cut is due
/// ([`cut_is_due`]). The cut is best effort, as the per-turn trace's: the row
/// is already written, and a ledger left long is no reason to fail.
pub fn record_request_timing_at_path(path: &Path, record: &RequestTimingRecord) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    if cut_is_due(path) {
        let _ = crate::jsonl_log::prune_jsonl_lines(path, RETAINED_ROWS);
    }
    Ok(())
}

/// Whether this process's append to `path` is the one that cuts the ledger
/// back: its first, and then each one after it has appended as many rows
/// again as the ledger keeps. A ledger stays within twice the rows it keeps,
/// and is read whole once per that many appends rather than on every one.
fn cut_is_due(path: &Path) -> bool {
    let mut appended = APPENDED_SINCE_CUT.lock().unwrap_or_else(PoisonError::into_inner);
    let since_cut = appended.entry(path.to_path_buf()).or_insert(RETAINED_ROWS);
    *since_cut += 1;
    let due = *since_cut > RETAINED_ROWS;
    if due {
        *since_cut = 0;
    }
    due
}

/// Read a project's ledger, dropping any line that does not parse.
pub fn read_request_timings(cwd: &Path) -> io::Result<Vec<RequestTimingRecord>> {
    read_request_timings_at_path(&request_timings_path(cwd))
}

/// Read a ledger at an explicit path; a missing file is an empty ledger. A
/// ledger written by this module holds at most twice [`RETAINED_ROWS`] rows.
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

    fn tool_call(id: &str, name: &str, status: crate::message_stream::types::ToolCallStatus) -> RenderBlock {
        use crate::message_stream::types::{ToolCallId, ToolCallStatus, ToolPreview};
        RenderBlock::ToolCall {
            id: BlockId(2),
            tool_call_id: ToolCallId(id.to_string()),
            name: name.to_string(),
            summary: String::new(),
            preview: ToolPreview::Generic { name: name.to_string(), input_summary: String::new() },
            input_print: (status == ToolCallStatus::Running).then(String::new),
            status,
        }
    }

    /// Z13: each tool block of an answer closes at its own time — its input
    /// complete, the moment its tool could have started — and the row says
    /// when, counted from the send, by the tool's name alone. A block
    /// announced while its input still streams has not closed.
    #[test]
    fn a_row_says_when_each_tool_block_of_the_answer_closed() {
        use crate::message_stream::types::ToolCallStatus::{Pending, Running};
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut stamps = StreamStamps::begun(t0);
        stamps.observe_at(&sent(1), t0 + ms(5));
        stamps.observe_at(&tool_call("toolu_a", "read_file", Pending), t0 + ms(40));
        stamps.observe_at(&tool_call("toolu_a", "read_file", Running), t0 + ms(100));
        stamps.observe_at(&tool_call("toolu_b", "Bash", Pending), t0 + ms(120));
        stamps.observe_at(&tool_call("toolu_b", "Bash", Running), t0 + ms(305));
        stamps.complete_at(t0 + ms(400));

        let row = serde_json::to_value(timing_record(&context("completed"), &stamps)).expect("a row");
        assert_eq!(
            row["tool_closed_ms"],
            serde_json::json!([{"name": "read_file", "ms": 95}, {"name": "Bash", "ms": 300}]),
            "the first block's close and the second's: {row}"
        );
    }

    /// A retry re-opens the attempt, and the blocks of the attempt before it
    /// closed in an answer nobody read: only the answering attempt's closes
    /// count, from its own send.
    #[test]
    fn a_retry_forgets_the_tool_blocks_of_the_attempt_before() {
        use crate::message_stream::types::ToolCallStatus::Running;
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut stamps = StreamStamps::begun(t0);
        stamps.observe_at(&sent(1), t0 + ms(5));
        stamps.observe_at(&tool_call("toolu_a", "read_file", Running), t0 + ms(50));
        stamps.observe_at(&sent(2), t0 + ms(100));
        stamps.observe_at(&tool_call("toolu_a", "read_file", Running), t0 + ms(180));
        stamps.observe_at(&tool_call("toolu_a", "read_file", Running), t0 + ms(190));
        let row = serde_json::to_value(timing_record(&context("completed"), &stamps)).expect("a row");
        assert_eq!(row["tool_closed_ms"], serde_json::json!([{"name": "read_file", "ms": 80}]), "{row}");
    }

    /// Z13: the request that carries an answer's tool results back says how
    /// long each tool ran, in the order the answer asked for them, by name; a
    /// tool that did not run has no time; and a row with no attempt to join
    /// it to its answer's row says none of it.
    #[test]
    fn a_row_says_how_long_each_tool_whose_result_it_carries_ran() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut runs = ToolRuns::asked(["read_file", "Bash", "write_file"]);
        runs.ran(0, t0, t0 + ms(12));
        runs.ran(2, t0 + ms(12), t0 + ms(40));
        runs.ran(7, t0, t0 + ms(1));
        let mut stamps = StreamStamps::begun(t0 + ms(50));
        stamps.tools_ran = runs;
        stamps.observe_at(&sent(1), t0 + ms(52));

        let row = serde_json::to_value(timing_record(&context("completed"), &stamps)).expect("a row");
        assert_eq!(
            row["tool_ran_ms"],
            serde_json::json!([{"name": "read_file", "ms": 12}, {"name": "Bash"}, {"name": "write_file", "ms": 28}]),
            "{row}"
        );
        let unjoinable = TimingContext { attempt: "", ..context("completed") };
        let row = serde_json::to_value(timing_record(&unjoinable, &stamps)).expect("a row");
        assert!(row.get("tool_ran_ms").is_none(), "a row with no attempt carries no tool times: {row}");
    }

    /// Every row written before the turn's columns parses, and a request no
    /// turn start was stamped for spells none of them: a file kept forever
    /// pays no bytes for a wait nobody measured.
    #[test]
    fn a_row_without_the_turn_columns_parses_and_spells_none_of_them() {
        let old_line = r#"{"recorded_at":1789000000,"session_id":"s","model":null,"iteration":1,"request_messages":2,"attempts":1,"assemble_ms":1,"ttfb_ms":2,"ttfv_ms":3,"stream_ms":4,"outcome":"completed","attempt":"s@1"}"#;
        let parsed: RequestTimingRecord = serde_json::from_str(old_line).expect("an older row must still parse");
        assert_eq!((parsed.turn_start_ms, parsed.leg, parsed.judge_wait_ms, parsed.preflight_ms), (None, None, None, None));
        assert!(parsed.tool_closed_ms.is_empty() && parsed.tool_ran_ms.is_empty());

        let t0 = Instant::now();
        let mut stamps = StreamStamps::begun(t0);
        stamps.observe_at(&sent(1), t0 + Duration::from_millis(5));
        let written = serde_json::to_string(&timing_record(&context("completed"), &stamps)).expect("serialize");
        for column in ["turn_start_ms", "\"leg\"", "judge_wait_ms", "preflight_ms", "tool_closed_ms", "tool_ran_ms"] {
            assert!(!written.contains(column), "{column} was spelled for nothing: {written}");
        }
    }

    /// The turn-start judgments held only the turn's first request: the
    /// first request a turn's clock stamps takes their wait off it, and every
    /// request after carries the turn's start alone.
    #[test]
    fn the_judgments_wait_rides_the_turns_first_request_only() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut clock = TurnClock::began(t0);
        clock.judge_wait = Some(t0 + ms(1)..t0 + ms(9));
        let first = StreamStamps::in_turn(t0 + ms(20), &mut clock, Some(t0 + ms(9)..t0 + ms(20)));
        let second = StreamStamps::in_turn(t0 + ms(900), &mut clock, None);
        assert_eq!(first.judge_wait, Some(t0 + ms(1)..t0 + ms(9)));
        assert_eq!(second.judge_wait, None);
        assert_eq!((first.turn_started, second.turn_started), (Some(t0), Some(t0)));
        assert_eq!(second.preflight, None);
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

    /// P3-2 (t-80327): the ledger keeps its newest rows — as many as the
    /// per-turn trace keeps lines, at most twice that between two cuts — so
    /// a project's file, which its readers take whole, stops growing.
    #[test]
    fn the_ledger_keeps_its_newest_rows_within_the_trace_line_cap() {
        let cap = crate::turn_trace::MAX_TURN_LOG_LINES;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("request-timings").join("timings.jsonl");
        let mut stamps = StreamStamps::begun(Instant::now());
        stamps.observe(&sent(1));
        stamps.complete();
        let rows = 3 * cap;
        for written in 0..rows {
            let record = RequestTimingRecord {
                recorded_at: u64::try_from(written).unwrap(),
                ..timing_record(&context("completed"), &stamps)
            };
            record_request_timing_at_path(&path, &record).unwrap();
        }
        let kept: Vec<u64> = read_request_timings_at_path(&path).unwrap().iter().map(|row| row.recorded_at).collect();
        assert!(kept.len() <= 2 * cap, "{} rows kept of {rows}, cap {cap}: {kept:?}", kept.len());
        let newest = u64::try_from(rows).unwrap();
        let expected: Vec<u64> = (newest - u64::try_from(kept.len()).unwrap()..newest).collect();
        assert_eq!(kept, expected, "the newest rows, in the order written");
    }
}
