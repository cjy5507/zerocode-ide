//! A worker's model-call costs, read as its CLI's transcript grows (t-26583).
//!
//! The one fact of the gate the hook server cannot carry: what each model call
//! cost lives in the CLI's own record. The window reads that record the way it
//! reads any growing log — never the file again, only what it has grown by —
//! and hands the whole lines to the CLI's format reader
//! ([`zerocode_core::continue_gate::spend`]), which answers the calls they
//! finish. Three things bound the cost on a machine that cannot spare it:
//!
//! - **a look is a stat** unless the file grew: one `open` and one `metadata`
//!   every [`POLL_MS`], and bytes only for what was appended;
//! - **a look reads at most [`READ_MAX_BYTES`]**; a backlog is read over
//!   several looks and a line longer than one is stepped over, never held;
//! - **a worker the window starts watching mid-run is read from the last
//!   [`ATTACH_TAIL_BYTES`]** of its file, not from its first byte: what it spent
//!   before the window looked is not this window's to know, and the board says
//!   the cost is "as far as watched".

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;
use zerocode_core::continue_gate::spend::{CallCost, CostReader, reader_for};

/// How often one transcript is looked at: three seconds — about what one model
/// call takes, and the beat that asks comes every second.
pub(super) const POLL_MS: i64 = 3_000;

/// How much of a transcript's end a worker first seen mid-run is read from:
/// a mebibyte, which is the last few hundred model calls of a long session.
pub(super) const ATTACH_TAIL_BYTES: u64 = 1024 * 1024;

/// The most one look reads: two mebibytes.
pub(super) const READ_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Why a worker's cost is, or is not, a number — what the board says beside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CostNote {
    /// Read from the CLI's transcript, as far as the window watched.
    Read,
    /// This CLI's transcript format has no reader yet.
    #[default]
    NoReader,
    /// The CLI has not said where it writes its conversation.
    NoTranscript,
    /// The transcript could not be opened.
    Unreadable,
}

/// What one look at the file found.
enum Look {
    /// The file has not grown: the stream of the call being written is over.
    Quiet,
    /// It has grown, but not to a whole line yet.
    Pending,
    /// Whole lines, oldest first.
    Lines(String),
}

/// One transcript being followed.
struct Tail {
    path: PathBuf,
    /// Where the next whole line begins.
    offset: u64,
    /// Whether the window has read this file before — the first look reads from
    /// the tail, the rest from `offset`.
    attached: bool,
    reader: Box<dyn CostReader>,
    polled_ms: i64,
    unreadable: bool,
}

impl Tail {
    fn new(path: &Path, reader: Box<dyn CostReader>) -> Self {
        Self {
            path: path.to_path_buf(),
            offset: 0,
            attached: false,
            reader,
            polled_ms: i64::MIN,
            unreadable: false,
        }
    }

    /// The calls the file has finished since the last look.
    fn look(&mut self, now_ms: i64) -> Vec<CallCost> {
        if now_ms.saturating_sub(self.polled_ms) < POLL_MS {
            return Vec::new();
        }
        self.polled_ms = now_ms;
        match self.read_new() {
            Ok(Look::Lines(text)) => {
                self.unreadable = false;
                self.reader.feed(&text)
            }
            Ok(Look::Quiet) => {
                self.unreadable = false;
                self.reader.flush().into_iter().collect()
            }
            Ok(Look::Pending) => {
                self.unreadable = false;
                Vec::new()
            }
            Err(_) => {
                self.unreadable = true;
                Vec::new()
            }
        }
    }

    /// What the file has grown by, as whole lines.
    fn read_new(&mut self) -> std::io::Result<Look> {
        let mut file = File::open(&self.path)?;
        let length = file.metadata()?.len();
        if length < self.offset {
            // The file was replaced by a shorter one: a new conversation under
            // the same name, read from its end like any first sight.
            self.offset = 0;
            self.attached = false;
        }
        if self.attached && length == self.offset {
            return Ok(Look::Quiet);
        }
        let first = !self.attached;
        let from = if first {
            length.saturating_sub(ATTACH_TAIL_BYTES)
        } else {
            self.offset
        };
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        file.take(READ_MAX_BYTES).read_to_end(&mut bytes)?;
        self.attached = true;
        // A first look that starts inside the file starts inside a line: that
        // line belongs to what came before the window looked.
        let skip = if first && from > 0 {
            bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |at| at + 1)
        } else {
            0
        };
        let end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        if end <= skip {
            // No whole line: one still being written (wait), or one longer than
            // a look (step over it, so that it is never held).
            let full = u64::try_from(bytes.len()).is_ok_and(|read| read >= READ_MAX_BYTES);
            let stepped = if full || (first && from > 0) {
                u64::try_from(bytes.len()).unwrap_or_default()
            } else {
                0
            };
            self.offset = from + stepped;
            return Ok(Look::Pending);
        }
        self.offset = from + u64::try_from(end).unwrap_or_default();
        Ok(Look::Lines(
            String::from_utf8_lossy(&bytes[skip..end]).into_owned(),
        ))
    }
}

/// What a worker's cost is read through: its CLI's reader, once the CLI has said
/// where it writes.
pub(super) struct Meter {
    agent: String,
    tail: Option<Tail>,
}

impl Meter {
    pub(super) fn new(agent: &str) -> Self {
        Self {
            agent: agent.to_string(),
            tail: None,
        }
    }

    /// One look, when it is time: the model calls the transcript at `path`
    /// finished since the last one. A CLI with no reader, or one that has not
    /// said where it writes, answers none.
    pub(super) fn poll(&mut self, path: Option<&str>, now_ms: i64) -> Vec<CallCost> {
        let Some(path) = path.map(Path::new) else {
            return Vec::new();
        };
        if self
            .tail
            .as_ref()
            .is_none_or(|tail| tail.path.as_path() != path)
        {
            let Some(reader) = reader_for(&self.agent) else {
                return Vec::new();
            };
            self.tail = Some(Tail::new(path, reader));
        }
        self.tail
            .as_mut()
            .map_or_else(Vec::new, |tail| tail.look(now_ms))
    }

    /// Why the cost is, or is not, a number.
    pub(super) fn note(&self, path_known: bool) -> CostNote {
        if reader_for(&self.agent).is_none() {
            return CostNote::NoReader;
        }
        match &self.tail {
            Some(tail) if tail.unreadable => CostNote::Unreadable,
            Some(_) => CostNote::Read,
            None if path_known => CostNote::Read,
            None => CostNote::NoTranscript,
        }
    }
}

#[cfg(test)]
mod tests;
