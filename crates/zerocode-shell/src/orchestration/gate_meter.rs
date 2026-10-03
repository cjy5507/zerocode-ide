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
//!   [`ATTACH_TAIL_BYTES`]** of its file, not from its first byte, and what is
//!   found there is learned from and never counted ([`Looked::known`]): what it
//!   spent before the window looked is not this window's to know — the day's total
//!   already holds what a window that watched it counted — and the board says the
//!   cost is "as far as watched".

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

/// What one look at a transcript found, in the two kinds of call it can hold.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Looked {
    /// Calls that finished while the window watched: spent — in the worker's own
    /// total, its task's and the day's.
    pub(super) counted: Vec<CallCost>,
    /// Calls that finished before the window looked, read from the tail of a file
    /// it met mid-run. They say how dear this worker's calls are and are nobody's
    /// spend here: the day's total already holds what a window that watched them
    /// counted, and one that did not has no claim on them.
    pub(super) known: Vec<CallCost>,
}

/// What one look at the file found.
enum Look {
    /// The file has not grown: the stream of the call being written is over.
    Quiet,
    /// It has grown, but not to a whole line yet.
    Pending,
    /// Whole lines, oldest first — and whether they were already there when the
    /// window first looked.
    Lines { text: String, known: bool },
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
    /// Whether the worker was running before the window began to watch: its first
    /// look is then a look at the past, whatever the size of the file.
    predates: bool,
}

impl Tail {
    fn new(path: &Path, reader: Box<dyn CostReader>, predates: bool) -> Self {
        Self {
            path: path.to_path_buf(),
            offset: 0,
            attached: false,
            reader,
            polled_ms: i64::MIN,
            unreadable: false,
            predates,
        }
    }

    /// The calls the file has finished since the last look.
    fn look(&mut self, now_ms: i64) -> Looked {
        if now_ms.saturating_sub(self.polled_ms) < POLL_MS {
            return Looked::default();
        }
        self.polled_ms = now_ms;
        match self.read_new() {
            Ok(Look::Lines { text, known }) => {
                self.unreadable = false;
                let mut finished = self.reader.feed(&text);
                if known {
                    // The call still open at the end of a look at the past was made
                    // before the window looked too: finished here, and its late
                    // records are no second call.
                    finished.extend(self.reader.flush());
                    Looked {
                        counted: Vec::new(),
                        known: finished,
                    }
                } else {
                    Looked {
                        counted: finished,
                        known: Vec::new(),
                    }
                }
            }
            Ok(Look::Quiet) => {
                self.unreadable = false;
                Looked {
                    counted: self.reader.flush().into_iter().collect(),
                    known: Vec::new(),
                }
            }
            Ok(Look::Pending) => {
                self.unreadable = false;
                Looked::default()
            }
            Err(_) => {
                self.unreadable = true;
                Looked::default()
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
        let known = first && (self.predates || from > 0);
        if first {
            self.predates = false;
        }
        Ok(Look::Lines {
            text: String::from_utf8_lossy(&bytes[skip..end]).into_owned(),
            known,
        })
    }
}

/// What a worker's cost is read through: its CLI's reader, once the CLI has said
/// where it writes.
pub(super) struct Meter {
    agent: String,
    tail: Option<Tail>,
    predates: bool,
}

impl Meter {
    pub(super) fn new(agent: &str) -> Self {
        Self {
            agent: agent.to_string(),
            tail: None,
            predates: false,
        }
    }

    /// The worker was running before the window began to watch: its first look is
    /// a look at the past, whatever the size of the file, and none of what it
    /// finds is counted.
    pub(super) fn predating_the_window(mut self) -> Self {
        self.predates = true;
        self
    }

    /// One look, when it is time: the model calls the transcript at `path`
    /// finished since the last one. A CLI with no reader, or one that has not
    /// said where it writes, answers none.
    pub(super) fn poll(&mut self, path: Option<&str>, now_ms: i64) -> Looked {
        let Some(path) = path.map(Path::new) else {
            return Looked::default();
        };
        if self
            .tail
            .as_ref()
            .is_none_or(|tail| tail.path.as_path() != path)
        {
            let Some(reader) = reader_for(&self.agent) else {
                return Looked::default();
            };
            self.tail = Some(Tail::new(path, reader, self.predates));
            self.predates = false;
        }
        self.tail
            .as_mut()
            .map_or_else(Looked::default, |tail| tail.look(now_ms))
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
