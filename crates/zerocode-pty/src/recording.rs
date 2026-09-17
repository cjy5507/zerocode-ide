//! A program's output as its pty delivered it — every read, with when it
//! arrived — and the replay that turns a [`PromptDelivery`] against it the way
//! the pump does.
//!
//! Whether a readiness signal is right is a question about ORDER: which bytes
//! had arrived when the delivery decided to write. A screenshot cannot answer
//! it, and a live run answers it differently under load. A recording with its
//! clock answers it the same way every time, at whatever cadence it is turned.
//! The case that asked for one (t-4530, 2026-09-17): Claude Code 2.1.274 turned
//! bracketed paste on 0.3 s into its start, said nothing for three seconds,
//! then took that input handler down — `?2004l` — with the briefing a quiet
//! wait had pasted into it, and mounted its real composer a moment later.
//!
//! The text form, one fact per line:
//!
//! ```text
//! # comment lines say where the recording came from
//! size <rows> <cols>
//! length <ms>          (how long the capture ran, silence at the end included)
//! <ms> <base64>        (one read, in arrival order)
//! ```

use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::grid::Terminal;
use crate::ready::{Observed, Outcome, PromptDelivery, Step};

/// Why a recording's text could not be read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BadRecording {
    #[error("line {line}: {reason}")]
    Line { line: usize, reason: &'static str },
    #[error("the recording never says its {0}")]
    Missing(&'static str),
}

/// One captured start of one program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    rows: usize,
    cols: usize,
    length: Duration,
    reads: Vec<(Duration, Vec<u8>)>,
}

/// One write a replayed delivery asked for, with what the program had shown
/// by then — the facts a reader needs to say whether it landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// When, measured from the spawn.
    pub at: Duration,
    pub bytes: Vec<u8>,
    /// The Enter that submits, rather than the paste.
    pub submit: bool,
    /// The screen as the write went.
    pub screen: String,
    /// Whether the program had bracketed paste on as the write went.
    pub bracketed_paste: bool,
}

/// What a delivery did against a recording.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replayed {
    /// Every write, in order.
    pub writes: Vec<Written>,
    /// How the delivery ended, or `None` when the recording ran out first.
    pub outcome: Option<Outcome>,
    /// Each moment the program took an input handler down: bracketed paste
    /// going off after it had been on, judged read by read. Bytes written
    /// before such a moment went to a reader that no longer exists.
    pub handlers_down: Vec<Duration>,
}

impl Replayed {
    /// The paste writes — the Enters left out.
    pub fn pastes(&self) -> impl Iterator<Item = &Written> {
        self.writes.iter().filter(|write| !write.submit)
    }
}

impl Recording {
    /// Read a recording from its text form. See the module docs.
    ///
    /// # Errors
    ///
    /// A line that is none of the four shapes, or a recording that never
    /// states its size or its length.
    pub fn parse(text: &str) -> Result<Self, BadRecording> {
        let mut size = None;
        let mut length = None;
        let mut reads = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let bad = |reason| BadRecording::Line { line, reason };
            let raw = raw.trim();
            if raw.is_empty() || raw.starts_with('#') {
                continue;
            }
            let mut words = raw.split_whitespace();
            match (words.next(), words.next(), words.next(), words.next()) {
                (Some("size"), Some(rows), Some(cols), None) => {
                    let rows = rows.parse().map_err(|_| bad("rows is not a count"))?;
                    let cols = cols.parse().map_err(|_| bad("cols is not a count"))?;
                    size = Some((rows, cols));
                }
                (Some("length"), Some(ms), None, None) => {
                    length = Some(millis(ms).ok_or_else(|| bad("length is not milliseconds"))?);
                }
                (Some(ms), Some(bytes), None, None) => {
                    let at = millis(ms).ok_or_else(|| bad("a read's time is not milliseconds"))?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(bytes)
                        .map_err(|_| bad("a read's bytes are not base64"))?;
                    if reads.last().is_some_and(|(last, _)| *last > at) {
                        return Err(bad("reads are out of arrival order"));
                    }
                    reads.push((at, bytes));
                }
                _ => return Err(bad("not a size, a length or a read")),
            }
        }
        let (rows, cols) = size.ok_or(BadRecording::Missing("size"))?;
        let length = length.ok_or(BadRecording::Missing("length"))?;
        Ok(Self {
            rows,
            cols,
            length,
            reads,
        })
    }

    /// How long the capture ran, silence at its end included.
    #[must_use]
    pub const fn length(&self) -> Duration {
        self.length
    }

    /// Turn a delivery against this output, one pump round every `round`.
    ///
    /// The delivery is built by `deliver` at `from` — measured from the spawn
    /// and handed the replay's clock at that moment — so a launch briefing
    /// (`from` zero) and a send to a composer that is already up (`from` after
    /// it stood) are both expressible. Each round is the pump's: every read
    /// that had arrived by the round's end is fed to the grid, the glyph the
    /// delivery listens for is asked about whether or not anything waits (a
    /// round left untaken would hand its glyph to the next asker), and the
    /// step the delivery answers is obeyed. The program never hears the
    /// writes — it is a recording — so what matters is WHEN they went and
    /// what the program had shown by then.
    #[must_use]
    pub fn replay(
        &self,
        round: Duration,
        from: Duration,
        deliver: impl FnOnce(Instant) -> PromptDelivery,
    ) -> Replayed {
        let origin = Instant::now();
        let mut terminal = Terminal::new(self.rows, self.cols);
        let mut deliver = Some(deliver);
        let mut delivery: Option<PromptDelivery> = None;
        let mut replayed = Replayed::default();
        let mut pending = self.reads.iter().peekable();
        let mut elapsed = Duration::ZERO;
        while elapsed < self.length {
            if elapsed >= from
                && let Some(build) = deliver.take()
            {
                delivery = Some(build(origin + elapsed));
            }
            elapsed += round;
            let mut wrote = false;
            while let Some((at, bytes)) = pending.next_if(|(at, _)| *at <= elapsed) {
                let listening = terminal.grid().bracketed_paste();
                terminal.feed(bytes);
                if listening && !terminal.grid().bracketed_paste() {
                    replayed.handlers_down.push(*at);
                }
                wrote = true;
            }
            let grid = terminal.grid_mut();
            // The pump writes these back to the child; a recording has no
            // child, and its answers were given when it was captured.
            drop(grid.take_replies());
            let waiting = delivery.as_ref().filter(|_| replayed.outcome.is_none());
            let drawn = grid.take_glyph_drawn(waiting.and_then(PromptDelivery::marker));
            let seen = Observed {
                wrote,
                bracketed_paste: grid.bracketed_paste(),
                cursor_shows: grid.cursor_shows(),
                marker_written: drawn.anywhere,
                marker_in_alt: drawn.in_alt_screen,
                alt_screen: grid.alt_screen(),
            };
            let Some(delivery) = delivery.as_mut().filter(|_| replayed.outcome.is_none()) else {
                continue;
            };
            let (bytes, submit) = match delivery.poll(seen, origin + elapsed) {
                Step::Waiting => continue,
                Step::Done(outcome) => {
                    replayed.outcome = Some(outcome);
                    continue;
                }
                Step::Write(bytes) => (bytes, false),
                Step::Submit(bytes) => (bytes, true),
            };
            replayed.writes.push(Written {
                at: elapsed,
                bytes,
                submit,
                screen: grid.visible_text(),
                bracketed_paste: grid.bracketed_paste(),
            });
        }
        replayed
    }
}

/// A recording's millisecond figure, fractions allowed.
fn millis(text: &str) -> Option<Duration> {
    let ms: f64 = text.parse().ok()?;
    (ms.is_finite() && ms >= 0.0).then(|| Duration::from_secs_f64(ms / 1000.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ready::ReadySignal;

    /// The four shapes read, and anything else is named by its line.
    #[test]
    fn a_recording_reads_its_four_shapes_and_names_a_bad_line() {
        let recording =
            Recording::parse("# made by hand\nsize 2 10\nlength 50\n0.5 G1s/MjAwNGg=\n12 aGk=\n")
                .expect("a well-formed recording");
        assert_eq!(recording.length(), Duration::from_millis(50));
        assert_eq!(recording.reads.len(), 2);
        assert_eq!(recording.reads[0].1, b"\x1b[?2004h");
        assert_eq!(
            Recording::parse("size 2 10\nlength 50\nsoon aGk=\n"),
            Err(BadRecording::Line {
                line: 3,
                reason: "a read's time is not milliseconds"
            })
        );
        assert_eq!(
            Recording::parse("size 2 10\n"),
            Err(BadRecording::Missing("length"))
        );
        assert_eq!(
            Recording::parse("size 2 10\nlength 50\n9 aGk=\n3 aGk=\n"),
            Err(BadRecording::Line {
                line: 4,
                reason: "reads are out of arrival order"
            })
        );
    }

    /// A handler taken down is recorded, and a paste that went before it is
    /// visibly a paste into that handler.
    #[test]
    fn a_replay_says_when_a_handler_came_down_and_what_the_screen_held() {
        // Handshake at 10 ms, silence, the handler down at 900 ms, a new
        // handshake and a `❯` composer at 950 ms.
        let bytes = |raw: &[u8]| base64::engine::general_purpose::STANDARD.encode(raw);
        let text = format!(
            "size 4 20\nlength 1200\n10 {}\n900 {}\n950 {}\n",
            bytes(&b"\x1b[?2004h"[..]),
            bytes(&b"\x1b[?2004l"[..]),
            bytes("\x1b[?2004h\u{276f} ".as_bytes()),
        );
        let recording = Recording::parse(&text).expect("recording");
        let round = Duration::from_millis(1);
        let quiet = Duration::from_millis(300);
        let timeout = Duration::from_secs(8);
        let silence = recording.replay(round, Duration::ZERO, |now| {
            PromptDelivery::with_deadlines(
                "brief".into(),
                false,
                ReadySignal::Quiet,
                now,
                quiet,
                timeout,
            )
        });
        assert_eq!(silence.handlers_down, [Duration::from_millis(900)]);
        let pasted = silence.pastes().next().expect("the quiet wait pasted");
        assert!(pasted.at < silence.handlers_down[0], "{pasted:?}");
        assert!(pasted.bracketed_paste && !pasted.screen.contains('\u{276f}'));
        assert_eq!(silence.outcome, Some(Outcome::Delivered));

        let glyph = recording.replay(round, Duration::ZERO, |now| {
            PromptDelivery::with_deadlines(
                "brief".into(),
                false,
                ReadySignal::Prompt('\u{276f}'),
                now,
                quiet,
                timeout,
            )
        });
        let pasted = glyph.pastes().next().expect("the glyph wait pasted");
        assert!(pasted.at > glyph.handlers_down[0], "{pasted:?}");
        assert!(pasted.screen.contains('\u{276f}'));
    }
}
