//! A worker's conversation as structured turns — what `worker-transcript`
//! answers (t-6742).
//!
//! `worker-read` hands back a screen: the last lines of a terminal, escape
//! codes and all, which says what a worker is SHOWING. This answers what it
//! DID: who spoke, which tools it called and what came back, and when — out
//! of the transcript its agent reported, read by the one reader the window's
//! conversation view already uses ([`crate::transcript::turns_in`] behind
//! the shell's `transcript_log_at`). Nothing here reads a file: the window
//! reads, this shapes, and the shaping is a pure function so the tests below
//! are the whole contract.
//!
//! Three rules the shape keeps, each with a test of its own:
//! - a TURN is one STEP, not one provider record: a person's prompt, or
//!   what the assistant said together with the calls it made after saying
//!   it and their results, joined by call id. The assistant speaking again
//!   after a call opens the next step. A worker runs for hours on one
//!   prompt, so "up to the next prompt" would make its whole session one
//!   turn and `--turns` meaningless (measured on this machine's workers:
//!   one turn in a megabyte of transcript);
//! - every text is masked ([`crate::credential::mask_values`]) BEFORE it
//!   is cut to its cap, so a cut can never split a credential in half and
//!   leave the half the mask would have caught; and the cut says how long
//!   the text was. The reader hands the texts over whole for that
//!   ([`crate::transcript::Detail::Whole`]): the conversation view's own
//!   16 KiB cut, made before any mask, never stands in front of this one.
//!   A tool's name and call id are masked by the same table — a call and
//!   its result are joined on the ids as written first, so two ids the mask
//!   folds into the same `[redacted]` are never joined for looking alike;
//! - the whole answer has a cap of its own, and past it the OLDEST turns go
//!   first and the answer says how many went — the newest turns are the ones
//!   a coordinator asked for. JSON and text are two renderings of one value,
//!   so they never differ in what they hold or in what was cut.

use crate::credential;
use crate::transcript::{TranscriptTool, TranscriptTurn};

/// How many turns come back when nobody says otherwise: the last few steps
/// of a worker's work, each with what it ran and how that ended. Five, so
/// an ordinary worker's default read is answered from the first chunk:
/// measured on this machine, the last 256 KiB of a worker's transcript held
/// 4 to 8 steps, and a count proves itself with one more step than it
/// shows ([`TranscriptAsk::satisfied_by`]).
pub const TURNS_DEFAULT: usize = 5;

/// The most turns one answer carries. `--turns` above this is read as this,
/// and the answer says how many turns it holds.
pub const TURNS_MAX: usize = 200;

/// How much of a person's or the assistant's text one turn keeps.
pub const TEXT_CHARS: usize = 2_000;

/// How much of a tool's input and of its result one call keeps — the digest
/// a coordinator reads to know WHAT ran and HOW it ended, never the output
/// itself (`worker-read` and the file are there for that).
pub const TOOL_CHARS: usize = 400;

/// The whole answer's ceiling, whichever rendering is asked for. Past it the
/// oldest turns are dropped, and the answer counts them.
pub const ANSWER_BYTES: usize = 64 * 1024;

/// How far back one call may read, in the reader's chunks (256 KiB each):
/// one chunk first, and when the window asked for is not inside it, one
/// read of this many. The two reads — a chunk, then this many, each with
/// the one byte before it that says whether it opens mid-line — are all
/// one call reads, whatever its lines hold: a line longer than the wider
/// read is not read whole, it is said ([`Scan::skipped`]).
pub const SCAN_CHUNKS: u64 = 4;

/// The word that opens every refusal of this verb whose cause is that there
/// is no transcript to read, so a caller can tell "unavailable" from "wrong".
pub const UNAVAILABLE: &str = "transcript unavailable";

/// What part of the conversation is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// The newest `n` turns.
    LastTurns(usize),
    /// Every turn from this moment (epoch ms, inclusive) on.
    Since(i64),
}

/// One `worker-transcript` request, parsed and refused where it contradicts
/// itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptAsk {
    pub window: Window,
    /// `--json`: the structured record. Without it, the text rendering of
    /// the same record.
    pub json: bool,
}

impl TranscriptAsk {
    /// Read the verb's flags. `--turns` and `--since` are two windows and
    /// cannot both be meant; `--turns 0` asks for nothing; a moment is epoch
    /// milliseconds and never negative. A count past [`TURNS_MAX`] is that
    /// many.
    pub fn of(turns: Option<&str>, since: Option<&str>, json: bool) -> Result<Self, String> {
        let window = match (turns, since) {
            (Some(_), Some(_)) => {
                return Err("--turns and --since are two different windows — give one".into());
            }
            (Some(count), None) => {
                let count: usize = count
                    .trim()
                    .parse()
                    .map_err(|_| format!("--turns wants a count, not {count}"))?;
                if count == 0 {
                    return Err("--turns 0 asks for nothing".into());
                }
                Window::LastTurns(count.min(TURNS_MAX))
            }
            (None, Some(moment)) => {
                let moment: i64 = moment
                    .trim()
                    .parse()
                    .map_err(|_| format!("--since wants epoch milliseconds, not {moment}"))?;
                if moment < 0 {
                    return Err("--since is epoch milliseconds, never negative".into());
                }
                Window::Since(moment)
            }
            (None, None) => Window::LastTurns(TURNS_DEFAULT),
        };
        Ok(Self { window, json })
    }

    /// Whether `records` (oldest first, as the reader hands them back)
    /// already hold what the window asks for, so a reader walking back
    /// through a file can stop reading.
    ///
    /// For a count, ONE MORE turn than asked: the turn before the window
    /// proves the window's first turn began inside what was read. For a
    /// moment, the oldest stamped record before it — everything after is
    /// then inside the read.
    #[must_use]
    pub fn satisfied_by(&self, records: &[TranscriptTurn]) -> bool {
        match self.window {
            Window::LastTurns(count) => spans(records).len() > count,
            Window::Since(moment) => records
                .iter()
                .find_map(|record| record.at_ms)
                .is_some_and(|first| first < moment),
        }
    }
}

/// What the window read to answer — said with the answer, so a reader knows
/// whether "no more turns" means the conversation began here or the read
/// did.
///
/// Every field is what the reads DID, not what they were asked to do: the
/// bytes they took, and the stretch of whole lines the answer came out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    /// The transcript's size when the call opened it. Every read of the call
    /// stops there, so a file appended to meanwhile is answered as it stood
    /// then, and every number below is of that one file.
    pub file_bytes: u64,
    /// The bytes the call read, all its reads together — over every open
    /// it made, a read that came up short at a cut included. Never more
    /// than the call's one budget: a file cut in place is read again on what
    /// the first open left of it, not on a budget of its own.
    pub read_bytes: u64,
    /// Where the whole lines the answer was read out of begin in the file…
    pub covered_from: u64,
    /// …and where they end. Short of `file_bytes`, the file's last line had
    /// no end yet: it is still being written, and is not in the answer.
    pub covered_to: u64,
    /// Whether the file goes on above what was covered — the read is a tail.
    pub cut_above: bool,
    /// Whether the newest whole line the reads reached was longer than they
    /// could take, and was not read: a turn is missing where it stood.
    pub skipped: bool,
}

/// A text kept to a cap, masked first, and honest about its length.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Digest {
    pub text: String,
    /// How many characters the text had as the reader read it out of the
    /// transcript — whole, before masking and before any cut (the reader
    /// cuts nothing on this road, [`crate::transcript::Detail::Whole`]). An
    /// image's base64 the reader sets aside is not text and not counted.
    pub chars: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl Digest {
    /// Mask, then cut on a character boundary. The order is the point: a cut
    /// before the mask could end inside a credential and leave a prefix the
    /// mask no longer recognises.
    ///
    /// Masked a line at a time, and only as far as the cut reaches:
    /// [`credential::mask_values`] judges each line on that line alone, so
    /// the lines up to the cut, masked one by one, are exactly the start of
    /// the whole text masked — and a 16 KiB tool input costs its first
    /// lines instead of all of them (a test holds the two equal).
    fn of(raw: &str, cap: usize) -> Self {
        let mut masked = String::new();
        let mut count = 0;
        let mut more = false;
        for (index, line) in raw.split('\n').enumerate() {
            if count > cap {
                more = true;
                break;
            }
            if index > 0 {
                masked.push('\n');
                count += 1;
            }
            let line = credential::mask_values(line);
            count += line.chars().count();
            masked.push_str(&line);
        }
        let chars = raw.chars().count();
        if count <= cap && !more {
            return Self {
                text: masked,
                chars,
                truncated: false,
            };
        }
        Self {
            text: masked.chars().take(cap).collect(),
            chars,
            truncated: true,
        }
    }

    fn empty() -> Self {
        Self {
            text: String::new(),
            chars: 0,
            truncated: false,
        }
    }

    /// The text with its cut said inline, for the text rendering.
    fn said(&self) -> String {
        if self.truncated {
            format!("{}…(of {} chars)", self.text, self.chars)
        } else {
            self.text.clone()
        }
    }
}

/// One tool call inside an assistant turn, with its result when the
/// transcript holds one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub input: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Digest>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,
}

/// One logical turn — one STEP (the module's first rule): a person's
/// prompt, or what the assistant said together with the calls it made
/// after saying it and their results.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    /// `user` or `assistant`.
    pub role: String,
    pub text: Digest,
    /// The moment the turn's first record was written, when the vendor
    /// stamped one; absent is unknown, never now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,
    /// The moment of its last record, when that differs and is stamped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until_ms: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolCall>,
    /// How many reasoning records the turn carried and this answer left
    /// out — said, so nothing is hidden silently.
    #[serde(skip_serializing_if = "is_zero")]
    pub thinking: usize,
    /// A call in this turn still has no result in the transcript: the turn
    /// is under way, or the file was cut before the result was written.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    /// How many of this turn's oldest calls were dropped to fit the answer.
    #[serde(skip_serializing_if = "is_zero")]
    pub tools_dropped: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// The window, as the answer states it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowSaid {
    Turns(usize),
    SinceMs(i64),
}

/// The answer: a worker's turns and everything about what was not shown.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTranscript {
    pub worker: String,
    pub agent: String,
    pub window: WindowSaid,
    pub turns: Vec<Turn>,
    /// Logical turns found in what was read.
    pub found: usize,
    /// Found, but before the window.
    pub earlier: usize,
    /// Inside the window, but dropped so the answer fits [`ANSWER_BYTES`].
    pub dropped: usize,
    /// Turns with no stamp at all, which a `--since` could not place.
    pub unplaced: usize,
    pub scan: Scan,
    /// Whether anything was cut: a text past its cap, a turn or a call
    /// dropped for the answer's size.
    pub truncated: bool,
}

/// Where one logical turn stands among the reader's records, and the
/// first and last moments its records were stamped with. Found without
/// building a turn — no text is masked to count turns or place a moment —
/// so the window is chosen first and only its turns are built.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    user: bool,
    first_ms: Option<i64>,
    last_ms: Option<i64>,
}

/// The logical turns' places in `records`, oldest first — see the module's
/// first rule. A `user` record is a turn of its own. Anything else joins
/// the assistant's current step, and the assistant speaking or thinking
/// again after a call in that step opens the next one.
fn spans(records: &[TranscriptTurn]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut called = false;
    for (at, record) in records.iter().enumerate() {
        let user = record.role == "user";
        let speaks = matches!(record.role.as_str(), "assistant" | "thinking");
        let opens = match spans.last() {
            None => true,
            Some(last) => user || last.user || (speaks && called),
        };
        if opens {
            spans.push(Span {
                start: at,
                end: at,
                user,
                first_ms: None,
                last_ms: None,
            });
            called = false;
        }
        let current = spans.last_mut().expect("a span was just opened");
        current.end = at + 1;
        if let Some(stamp) = record.at_ms {
            current.first_ms.get_or_insert(stamp);
            current.last_ms = Some(stamp);
        }
        called |= matches!(record.role.as_str(), "tool" | "tool_result");
    }
    spans
}

impl ToolCall {
    /// A call as the answer carries it: its name and id masked by the table
    /// every other text is masked by, never copied as written.
    fn of(
        tool: &TranscriptTool,
        input: Digest,
        result: Option<Digest>,
        at_ms: Option<i64>,
    ) -> Self {
        Self {
            call_id: credential::mask_values(&tool.call_id),
            name: credential::mask_values(&tool.name),
            input,
            is_error: result.is_some() && tool.is_error,
            result,
            at_ms,
        }
    }
}

/// Build one turn out of its records. A result is joined to the call with
/// its id inside the turn; one whose call is not there (the read began
/// between the two) stands as a call of its own with the result's name.
///
/// The join reads the ids AS WRITTEN, kept beside the calls and never in
/// them: a call carries its id out masked ([`ToolCall::of`]), and two ids
/// the mask folds into the same `[redacted]` must not join a result to a
/// call that merely looks alike once masked.
fn turn_of(records: &[TranscriptTurn], span: &Span) -> Turn {
    let mut texts: Vec<&str> = Vec::new();
    let mut tools: Vec<ToolCall> = Vec::new();
    // The id each of `tools` was written with, at the same index.
    let mut written: Vec<&str> = Vec::new();
    let mut thinking = 0;
    for record in &records[span.start..span.end] {
        match record.role.as_str() {
            "user" | "assistant" => {
                if !record.text.is_empty() {
                    texts.push(&record.text);
                }
            }
            "thinking" => thinking += 1,
            "tool" => {
                let Some(tool) = record.tool.as_ref() else {
                    continue;
                };
                tools.push(ToolCall::of(
                    tool,
                    Digest::of(&tool.input, TOOL_CHARS),
                    None,
                    record.at_ms,
                ));
                written.push(&tool.call_id);
            }
            "tool_result" => {
                let Some(tool) = record.tool.as_ref() else {
                    continue;
                };
                let result = Digest::of(&record.text, TOOL_CHARS);
                match tools
                    .iter()
                    .zip(&written)
                    .position(|(call, id)| call.result.is_none() && *id == tool.call_id)
                {
                    Some(at) => {
                        tools[at].result = Some(result);
                        tools[at].is_error = tool.is_error;
                    }
                    None => {
                        tools.push(ToolCall::of(
                            tool,
                            Digest::empty(),
                            Some(result),
                            record.at_ms,
                        ));
                        written.push(&tool.call_id);
                    }
                }
            }
            _ => {}
        }
    }
    let open = tools.iter().any(|call| call.result.is_none());
    Turn {
        role: if span.user { "user" } else { "assistant" }.into(),
        text: if texts.is_empty() {
            Digest::empty()
        } else {
            Digest::of(&texts.join("\n"), TEXT_CHARS)
        },
        at_ms: span.first_ms,
        until_ms: span.last_ms.filter(|last| Some(*last) != span.first_ms),
        tools,
        thinking,
        open,
        tools_dropped: 0,
    }
}

/// The logical turns in `records`, oldest first — every one of them built.
/// [`shape`] builds only the window's; this is the whole list, for a reader
/// that wants it.
#[must_use]
pub fn logical_turns(records: &[TranscriptTurn]) -> Vec<Turn> {
    spans(records)
        .iter()
        .map(|span| turn_of(records, span))
        .collect()
}

/// Shape a read into the answer for `ask`: the window is chosen on the
/// turns' places and stamps, and only the turns inside it are built (and
/// masked).
#[must_use]
pub fn shape(
    worker: &str,
    agent: &str,
    records: &[TranscriptTurn],
    scan: Scan,
    ask: &TranscriptAsk,
) -> WorkerTranscript {
    let all = spans(records);
    let found = all.len();
    let (chosen, earlier, unplaced, window): (Vec<Span>, usize, usize, WindowSaid) =
        match ask.window {
            Window::LastTurns(count) => {
                let earlier = found.saturating_sub(count);
                (
                    all[earlier..].to_vec(),
                    earlier,
                    0,
                    WindowSaid::Turns(count),
                )
            }
            Window::Since(moment) => {
                let mut earlier = 0;
                let mut unplaced = 0;
                let kept = all
                    .into_iter()
                    .filter(|span| match span.last_ms {
                        None => {
                            unplaced += 1;
                            false
                        }
                        Some(last) if last >= moment => true,
                        Some(_) => {
                            earlier += 1;
                            false
                        }
                    })
                    .collect();
                (kept, earlier, unplaced, WindowSaid::SinceMs(moment))
            }
        };
    let mut answer = WorkerTranscript {
        worker: worker.to_string(),
        agent: agent.to_string(),
        window,
        turns: chosen.iter().map(|span| turn_of(records, span)).collect(),
        found,
        earlier,
        dropped: 0,
        unplaced,
        scan,
        truncated: false,
    };
    fit(&mut answer);
    answer.truncated = answer.dropped > 0
        || answer.turns.iter().any(|turn| {
            turn.text.truncated
                || turn.tools_dropped > 0
                || turn.tools.iter().any(|call| {
                    call.input.truncated || call.result.as_ref().is_some_and(|r| r.truncated)
                })
        });
    answer
}

/// The bytes one element adds to a JSON array: itself and its comma.
fn json_len<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_string(value).map_or(0, |json| json.len()) + 1
}

/// Drop the oldest turns — then, for the one turn left, its oldest calls —
/// until the JSON rendering fits [`ANSWER_BYTES`]. JSON is the larger of
/// the two renderings, so a value that fits as JSON fits as text too, and
/// the two never hold different turns. Each turn and call is measured once
/// and the drops are counted off those measures; the whole answer is then
/// measured again, and the rare answer the counts under-measured (a count's
/// own digits growing) goes round once more.
fn fit(answer: &mut WorkerTranscript) {
    loop {
        let total = serde_json::to_string(answer).map_or(0, |json| json.len());
        if total <= ANSWER_BYTES {
            return;
        }
        let mut over = total - ANSWER_BYTES;
        if answer.turns.len() > 1 {
            let mut drop = 0;
            for turn in &answer.turns[..answer.turns.len() - 1] {
                if over == 0 {
                    break;
                }
                over = over.saturating_sub(json_len(turn));
                drop += 1;
            }
            answer.turns.drain(..drop.max(1));
            answer.dropped += drop.max(1);
            continue;
        }
        let Some(only) = answer.turns.first_mut() else {
            return;
        };
        if only.tools.is_empty() {
            return;
        }
        let mut drop = 0;
        for call in &only.tools {
            if over == 0 {
                break;
            }
            over = over.saturating_sub(json_len(call));
            drop += 1;
        }
        let drop = drop.clamp(1, only.tools.len());
        only.tools.drain(..drop);
        only.tools_dropped += drop;
    }
}

impl WorkerTranscript {
    /// The answer as the verb prints it: the JSON record, or the text
    /// rendering of the same record.
    #[must_use]
    pub fn render(&self, json: bool) -> String {
        if json {
            return serde_json::to_string(self).unwrap_or_default();
        }
        let mut out = String::new();
        let window = match self.window {
            WindowSaid::Turns(count) => format!("last {count}"),
            WindowSaid::SinceMs(moment) => format!("since {}", crate::civil::iso_utc_of(moment)),
        };
        out.push_str(&format!(
            "worker {} · {} · {} turns of {} found (window {}; {} earlier{}) · file {} B, read {} B, lines {}–{} B{}{}{}{}",
            self.worker,
            self.agent,
            self.turns.len(),
            self.found,
            window,
            self.earlier,
            if self.unplaced > 0 {
                format!(", {} unplaced", self.unplaced)
            } else {
                String::new()
            },
            self.scan.file_bytes,
            self.scan.read_bytes,
            self.scan.covered_from,
            self.scan.covered_to,
            if self.scan.cut_above {
                ", more above"
            } else {
                ""
            },
            if self.scan.skipped {
                ", a line skipped"
            } else {
                ""
            },
            if self.scan.covered_to < self.scan.file_bytes {
                ", the last line still being written"
            } else {
                ""
            },
            if self.dropped > 0 {
                format!(" · {} turns dropped to fit", self.dropped)
            } else {
                String::new()
            },
        ));
        out.push('\n');
        for turn in &self.turns {
            let when = match (turn.at_ms, turn.until_ms) {
                (Some(from), Some(until)) => format!(
                    "[{} → {}] ",
                    crate::civil::iso_utc_of(from),
                    crate::civil::iso_utc_of(until)
                ),
                (Some(from), None) => format!("[{}] ", crate::civil::iso_utc_of(from)),
                (None, _) => String::new(),
            };
            out.push_str(&format!("{when}{}: {}\n", turn.role, turn.text.said()));
            if turn.tools_dropped > 0 {
                out.push_str(&format!(
                    "  · {} earlier calls dropped to fit\n",
                    turn.tools_dropped
                ));
            }
            for call in &turn.tools {
                let ended = match &call.result {
                    Some(result) => format!(
                        " → {}{}",
                        if call.is_error { "error · " } else { "" },
                        result.said()
                    ),
                    None => " → (no result yet)".to_string(),
                };
                out.push_str(&format!(
                    "  ↳ {} · {}{ended}\n",
                    call.name,
                    call.input.said()
                ));
            }
            if turn.thinking > 0 {
                out.push_str(&format!("  · thinking ×{} not shown\n", turn.thinking));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::MASK;
    use crate::transcript::{Detail, turns_in, turns_in_with};

    fn scan() -> Scan {
        Scan {
            file_bytes: 1_000,
            read_bytes: 1_000,
            covered_from: 0,
            covered_to: 1_000,
            cut_above: false,
            skipped: false,
        }
    }

    fn ask(turns: usize) -> TranscriptAsk {
        TranscriptAsk {
            window: Window::LastTurns(turns),
            json: true,
        }
    }

    /// The Claude Code shape: a prompt, the assistant's words, a call, its
    /// result on a `user` line, more words — one person's turn and one
    /// assistant turn, the result joined to its call.
    fn a_claude_exchange() -> String {
        [
            r#"{"type":"user","timestamp":"2026-09-24T12:00:00.000Z","message":{"role":"user","content":"run the tests"}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-24T12:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"plan"},{"type":"text","text":"Running them."},{"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"cargo test -p x"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-09-24T12:00:09.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"ok · 12 passed"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-24T12:00:10.000Z","message":{"role":"assistant","content":[{"type":"text","text":"All green."}]}}"#,
        ]
        .join("\n")
    }

    #[test]
    fn a_turn_is_a_step_what_was_said_and_the_calls_after_it_with_their_results() {
        let records = turns_in(&a_claude_exchange());
        assert_eq!(
            records.len(),
            6,
            "records: user, thinking, assistant, tool, result, assistant"
        );
        let turns = logical_turns(&records);
        assert_eq!(turns.len(), 3, "{turns:#?}");
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text.text, "run the tests");
        assert_eq!(turns[0].at_ms, Some(1_790_251_200_000));
        let step = &turns[1];
        assert_eq!(step.role, "assistant");
        assert_eq!(step.text.text, "Running them.");
        assert_eq!(step.thinking, 1, "the reasoning is counted, not shown");
        assert_eq!(step.at_ms, Some(1_790_251_201_000));
        assert_eq!(step.until_ms, Some(1_790_251_209_000), "until its result");
        assert_eq!(step.tools.len(), 1);
        assert_eq!(step.tools[0].name, "Bash");
        assert_eq!(step.tools[0].call_id, "call-1");
        assert_eq!(step.tools[0].input.text, "cargo test -p x");
        assert_eq!(
            step.tools[0].result.as_ref().map(|r| r.text.as_str()),
            Some("ok · 12 passed")
        );
        assert!(!step.open, "every call has its result");
        // Speaking again after a call is the next step.
        assert_eq!(turns[2].role, "assistant");
        assert_eq!(turns[2].text.text, "All green.");
        assert_eq!(turns[2].at_ms, Some(1_790_251_210_000));
        assert_eq!(turns[2].until_ms, None, "one stamp is a moment, not a span");
        assert!(turns[2].tools.is_empty());

        // A worker's hours on one prompt are many steps, not one turn.
        let mut long = vec![records[0].clone()];
        for _ in 0..30 {
            long.extend_from_slice(&records[1..5]);
        }
        assert_eq!(logical_turns(&long).len(), 31);
    }

    #[test]
    fn an_open_turn_is_a_call_still_without_its_result_and_an_orphan_result_stands_alone() {
        let records = turns_in(
            &[
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-0","content":"an earlier result"}]}}"#,
                r#"{"type":"user","message":{"role":"user","content":"go"}}"#,
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-2","name":"Read","input":{"file_path":"/w/a.rs"}}]}}"#,
            ]
            .join("\n"),
        );
        let turns = logical_turns(&records);
        assert_eq!(turns.len(), 3, "{turns:#?}");
        assert_eq!(turns[0].role, "assistant");
        assert_eq!(
            turns[0].tools.len(),
            1,
            "the orphan result is a call of its own"
        );
        assert!(turns[0].tools[0].input.text.is_empty());
        assert_eq!(
            turns[0].tools[0].result.as_ref().map(|r| r.text.as_str()),
            Some("an earlier result")
        );
        assert!(!turns[0].open, "a call with its result is not open");
        assert!(
            turns[2].open,
            "a call with no result yet leaves the last turn open"
        );
        assert_eq!(turns[2].tools[0].result, None);
        assert_eq!(turns[2].at_ms, None, "no stamp is unknown, never now");
    }

    /// The digest half of the brief's `a_transcript_result_is_cut_at_the_digest_cap_and_masked`
    /// (which runs end to end, through the verb, in the shell's tests).
    #[test]
    fn a_digest_is_masked_before_its_cut_and_says_how_long_its_text_was() {
        let filler = "x".repeat(TOOL_CHARS - 10);
        let records = turns_in(
            &[
                r#"{"type":"user","message":{"role":"user","content":"deploy with --password hunter2 please"}}"#.to_string(),
                r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Using Bearer abc.def now"},{"type":"tool_use","id":"c","name":"Bash","input":{"command":"curl -H 'Authorization: Bearer abc.def' https://user:tok3n@host.test/x"}}]}}"#.to_string(),
                format!(
                    r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"c","content":"{filler} DB_PASSWORD=sekretvalue1234567890 tail"}}]}}}}"#
                ),
            ]
            .join("\n"),
        );
        let answer = shape("w-1", "claude", &records, scan(), &ask(20));
        for json in [true, false] {
            let out = answer.render(json);
            for secret in ["hunter2", "abc.def", "tok3n", "sekretvalue"] {
                assert!(!out.contains(secret), "{secret} escaped ({json}): {out}");
            }
            assert!(out.contains(MASK), "nothing was masked ({json}): {out}");
        }
        let result = answer.turns[1].tools[0].result.as_ref().expect("a result");
        assert!(result.truncated, "the result was past the cap");
        assert_eq!(result.text.chars().count(), TOOL_CHARS);
        assert!(result.chars > TOOL_CHARS);
        // Masked BEFORE the cut: the cut falls inside `[redacted]`, never
        // inside the value — no prefix of the secret survives.
        assert!(
            result.text.contains(MASK.trim_end_matches(']')),
            "the cut fell inside the mask, not the value: {}",
            result.text
        );
        assert!(answer.truncated);
        assert!(
            answer.render(false).contains("…(of "),
            "the text says how long the source was"
        );
        // Both renderings carry the same cut state.
        let json: serde_json::Value = serde_json::from_str(&answer.render(true)).unwrap();
        assert_eq!(json["turns"][1]["tools"][0]["result"]["truncated"], true);
        assert_eq!(json["truncated"], true);
    }

    /// A call's name and id are texts too (t-6742 R1): masked by the same
    /// table in both renderings. The join does not go through the mask —
    /// two ids that both fold into `[redacted]` keep their own results,
    /// whatever order the results come back in.
    #[test]
    fn a_calls_name_and_id_are_masked_and_joined_on_the_ids_as_written() {
        let first = format!("ghp_{}", "a1".repeat(18));
        let second = format!("ghp_{}", "b2".repeat(18));
        let named = format!("deploy_sk-{}", "c3".repeat(12));
        let records = turns_in_with(
            &[
                r#"{"type":"user","message":{"role":"user","content":"go"}}"#.to_string(),
                format!(
                    r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"{first}","name":"{named}","input":{{"command":"one"}}}},{{"type":"tool_use","id":"{second}","name":"Bash","input":{{"command":"two"}}}}]}}}}"#
                ),
                format!(
                    r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{second}","content":"result of two"}},{{"type":"tool_result","tool_use_id":"{first}","content":"result of one","is_error":true}}]}}}}"#
                ),
            ]
            .join("\n"),
            Detail::Whole,
        );
        let answer = shape("w-1", "claude", &records, scan(), &ask(20));
        let calls = &answer.turns[1].tools;
        assert_eq!(calls.len(), 2, "{calls:#?}");
        assert_eq!(calls[0].call_id, MASK);
        assert_eq!(calls[1].call_id, MASK, "both ids fold into the mask");
        assert_eq!(calls[0].name, MASK);
        assert_eq!(
            calls[1].name, "Bash",
            "a name that is not a credential stays"
        );
        assert_eq!(
            calls[0].result.as_ref().map(|r| r.text.as_str()),
            Some("result of one"),
            "a result joined on the masked id"
        );
        assert!(calls[0].is_error);
        assert_eq!(
            calls[1].result.as_ref().map(|r| r.text.as_str()),
            Some("result of two")
        );
        assert!(!calls[1].is_error);
        for json in [true, false] {
            let out = answer.render(json);
            for secret in [first.as_str(), second.as_str(), named.as_str(), "a1a1a1"] {
                assert!(!out.contains(secret), "{secret} escaped: {out}");
            }
        }
        // And nothing held for the join outlives the turn: a call's Debug is
        // what its answer shows.
        assert!(!format!("{calls:?}").contains(&first));
    }

    /// Masking only the lines a cut reaches keeps exactly what masking the
    /// whole text and then cutting keeps — the mask judges a line on that
    /// line alone, including a glued password whose program is named later
    /// on its line and a header whose value runs to the line's end.
    #[test]
    fn a_digest_masks_as_far_as_its_cut_and_keeps_what_masking_everything_keeps() {
        let filler = |n: usize| "word ".repeat(n);
        let texts = [
            String::new(),
            "short".to_string(),
            format!("{}--password hunter2 tail", filler(78)),
            format!("{}-phunter2 goes to mysql today", filler(79)),
            format!(
                "{}\nAuthorization: Bearer abc.def ghi\n{}",
                filler(70),
                filler(200)
            ),
            format!("{}\nexport TOKEN=ghp_{}\n", filler(10), "z9".repeat(40)),
            format!(
                "line one\n{}\nDB_PASSWORD='alpha beta' end\n{}",
                filler(60),
                filler(300)
            ),
            format!("{}é {}", "ü".repeat(398), "Bearer sekret"),
            "a\n\n\n".repeat(300),
        ];
        for text in &texts {
            for cap in [1, 7, 399, 400, 401, TOOL_CHARS, TEXT_CHARS] {
                let whole = credential::mask_values(text);
                let digest = Digest::of(text, cap);
                let expected: String = whole.chars().take(cap).collect();
                assert_eq!(digest.text, expected, "cap {cap}: {text:?}");
                assert_eq!(digest.truncated, whole.chars().count() > cap, "cap {cap}");
                assert_eq!(digest.chars, text.chars().count());
                for secret in ["hunter2", "abc.def", "sekret", "alpha beta", "z9z9z9"] {
                    assert!(!digest.text.contains(secret), "{secret} kept at cap {cap}");
                }
            }
        }
    }

    #[test]
    fn the_window_is_the_last_n_turns_or_since_a_moment_and_unplaced_turns_are_counted() {
        let stamped = |at: u64, role: &str, text: &str| {
            format!(
                r#"{{"type":"{role}","timestamp":"2026-09-24T12:00:{at:02}.000Z","message":{{"role":"{role}","content":"{text}"}}}}"#
            )
        };
        let lines = [
            stamped(0, "user", "one"),
            stamped(1, "assistant", "a1"),
            stamped(2, "user", "two"),
            stamped(3, "assistant", "a2"),
            r#"{"type":"user","message":{"role":"user","content":"unstamped"}}"#.to_string(),
            stamped(6, "assistant", "a3"),
        ]
        .join("\n");
        let records = turns_in(&lines);
        let base = 1_790_251_200_000_i64;

        let last = shape("w", "claude", &records, scan(), &ask(2));
        assert_eq!(last.found, 6);
        assert_eq!(last.earlier, 4);
        assert_eq!(
            last.turns
                .iter()
                .map(|t| t.text.text.as_str())
                .collect::<Vec<_>>(),
            vec!["unstamped", "a3"]
        );

        let since = shape(
            "w",
            "claude",
            &records,
            scan(),
            &TranscriptAsk {
                window: Window::Since(base + 2_000),
                json: true,
            },
        );
        assert_eq!(
            since
                .turns
                .iter()
                .map(|t| t.text.text.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "a2", "a3"],
            "the moment is inclusive; the unstamped turn cannot be placed"
        );
        assert_eq!(since.earlier, 2);
        assert_eq!(since.unplaced, 1);
        assert_eq!(since.window, WindowSaid::SinceMs(base + 2_000));

        // A moment in the future is an honest nothing, not a refusal.
        let none = shape(
            "w",
            "claude",
            &records,
            scan(),
            &TranscriptAsk {
                window: Window::Since(base + 3_600_000),
                json: false,
            },
        );
        assert!(none.turns.is_empty());
        assert_eq!(none.earlier, 5);
        assert!(
            none.render(false)
                .starts_with("worker w · claude · 0 turns of 6 found")
        );
    }

    #[test]
    fn an_ask_refuses_two_windows_zero_turns_and_negative_moments() {
        assert!(TranscriptAsk::of(Some("3"), Some("5"), false).is_err());
        assert!(TranscriptAsk::of(Some("0"), None, false).is_err());
        assert!(TranscriptAsk::of(Some("many"), None, false).is_err());
        assert!(TranscriptAsk::of(None, Some("-1"), false).is_err());
        assert!(TranscriptAsk::of(None, Some("soon"), false).is_err());
        assert_eq!(
            TranscriptAsk::of(None, None, true).unwrap().window,
            Window::LastTurns(TURNS_DEFAULT)
        );
        assert_eq!(
            TranscriptAsk::of(Some("9999"), None, false).unwrap().window,
            Window::LastTurns(TURNS_MAX),
            "a count past the ceiling is the ceiling"
        );
        assert_eq!(
            TranscriptAsk::of(None, Some("0"), false).unwrap().window,
            Window::Since(0)
        );
    }

    #[test]
    fn a_reader_walking_back_stops_one_turn_past_the_count_or_before_the_moment() {
        let records = turns_in(&a_claude_exchange());
        assert!(
            ask(2).satisfied_by(&records),
            "three turns hold two and the one before them"
        );
        assert!(
            !ask(3).satisfied_by(&records),
            "three turns do not prove the third began inside the read"
        );
        let since = |moment: i64| TranscriptAsk {
            window: Window::Since(moment),
            json: false,
        };
        assert!(since(1_790_251_200_001).satisfied_by(&records));
        assert!(!since(1_790_251_200_000).satisfied_by(&records));
        assert!(!since(0).satisfied_by(&[]));
    }

    #[test]
    fn a_whole_answer_past_the_cap_drops_the_oldest_turns_and_both_renderings_agree() {
        let mut records = Vec::new();
        for at in 0..400 {
            records.push(TranscriptTurn {
                role: "user".into(),
                text: format!("prompt {at} {}", "p".repeat(TEXT_CHARS)),
                at_ms: Some(1_000 + at),
                tool: None,
                images: Vec::new(),
            });
            records.push(TranscriptTurn {
                role: "assistant".into(),
                text: format!("answer {at}"),
                at_ms: Some(1_001 + at),
                tool: None,
                images: Vec::new(),
            });
        }
        let answer = shape("w", "zo", &records, scan(), &ask(TURNS_MAX));
        assert!(answer.dropped > 0, "the cap did not bite");
        assert!(answer.render(true).len() <= ANSWER_BYTES);
        assert!(answer.render(false).len() <= ANSWER_BYTES);
        assert!(answer.truncated);
        assert_eq!(
            answer.turns.last().map(|t| t.text.text.as_str()),
            Some("answer 399"),
            "the newest turn is the one kept"
        );
        assert_eq!(
            answer.turns.len() + answer.dropped + answer.earlier,
            answer.found
        );
        let text = answer.render(false);
        assert!(text.contains(&format!("{} turns dropped to fit", answer.dropped)));
        let json: serde_json::Value = serde_json::from_str(&answer.render(true)).unwrap();
        assert_eq!(json["dropped"], answer.dropped);
        assert_eq!(json["turns"].as_array().unwrap().len(), answer.turns.len());

        // One turn too big for the cap on its own loses its oldest calls.
        let mut one = vec![TranscriptTurn {
            role: "user".into(),
            text: "go".into(),
            at_ms: None,
            tool: None,
            images: Vec::new(),
        }];
        for at in 0..400 {
            one.push(TranscriptTurn {
                role: "tool".into(),
                text: String::new(),
                at_ms: None,
                tool: Some(TranscriptTool {
                    call_id: format!("c{at}"),
                    name: "Bash".into(),
                    input: "i".repeat(TOOL_CHARS),
                    is_error: false,
                    edits: Vec::new(),
                    file: None,
                }),
                images: Vec::new(),
            });
        }
        let answer = shape("w", "codex", &one, scan(), &ask(1));
        assert_eq!(answer.turns.len(), 1);
        assert!(answer.turns[0].tools_dropped > 0);
        assert_eq!(
            answer.turns[0].tools.last().map(|c| c.call_id.as_str()),
            Some("c399")
        );
        assert!(answer.render(true).len() <= ANSWER_BYTES);
    }

    #[test]
    fn the_scan_and_the_agent_are_said_with_the_answer() {
        let records = turns_in(&a_claude_exchange());
        let answer = shape(
            "w-7",
            "claude",
            &records,
            Scan {
                file_bytes: 5_000_000,
                read_bytes: 1_310_722,
                covered_from: 3_951_424,
                covered_to: 4_999_000,
                cut_above: true,
                skipped: true,
            },
            &ask(20),
        );
        let json: serde_json::Value = serde_json::from_str(&answer.render(true)).unwrap();
        assert_eq!(json["worker"], "w-7");
        assert_eq!(json["agent"], "claude");
        assert_eq!(json["scan"]["fileBytes"], 5_000_000);
        assert_eq!(json["scan"]["readBytes"], 1_310_722);
        assert_eq!(json["scan"]["coveredFrom"], 3_951_424);
        assert_eq!(json["scan"]["coveredTo"], 4_999_000);
        assert_eq!(json["scan"]["cutAbove"], true);
        assert_eq!(json["scan"]["skipped"], true);
        assert_eq!(json["window"], serde_json::json!({"turns": 20}));
        assert!(json.get("path").is_none() && !json.to_string().contains("/Users/"));
        let text = answer.render(false);
        assert!(
            text.contains("file 5000000 B, read 1310722 B, lines 3951424–4999000 B")
                && text.contains("more above")
                && text.contains("a line skipped")
                && text.contains("the last line still being written"),
            "{text}"
        );
        assert!(
            text.contains("[2026-09-24T12:00:00.000Z] user: run the tests"),
            "{text}"
        );
        assert!(
            text.contains("↳ Bash · cargo test -p x → ok · 12 passed"),
            "{text}"
        );
        assert!(text.contains("thinking ×1 not shown"), "{text}");
    }
}
