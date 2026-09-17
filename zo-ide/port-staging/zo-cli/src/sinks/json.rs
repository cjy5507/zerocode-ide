//! Buffered JSON array sink.
//!
//! Collects every [`RenderBlock`] that arrives during a turn and emits
//! a single JSON array on [`Sink::finalize`]. This is the new
//! sinks-module extraction of the single-object JSON output historically
//! produced by `session::run_prompt_json`. Existing non-interactive
//! `--output-format json` behavior in `main.rs` continues to use the
//! legacy path during this lane to preserve parity-harness
//! byte-equivalence; this sink is the future home of that logic and is
//! exercised by the sinks integration tests.

use std::io::Write;

use runtime::message_stream::RenderBlock;

use super::serializable::SerializableRenderBlock;
use super::{Sink, SinkError};

/// Buffered JSON array sink.
pub struct JsonSink<W: Write> {
    writer: W,
    buffered: Vec<SerializableRenderBlock>,
    finalized: bool,
}

impl<W: Write> JsonSink<W> {
    /// Create a new JSON sink that writes to `writer`.
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            buffered: Vec::new(),
            finalized: false,
        }
    }

    /// Number of blocks currently buffered (for tests).
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.buffered.len()
    }
}

impl<W: Write> Sink for JsonSink<W> {
    fn emit(&mut self, block: &RenderBlock) -> Result<(), SinkError> {
        if self.finalized {
            return Err(SinkError::AlreadyFinalized);
        }
        self.buffered
            .push(SerializableRenderBlock::from_block(block));
        Ok(())
    }

    fn finalize(mut self: Box<Self>) -> Result<(), SinkError> {
        if self.finalized {
            return Err(SinkError::AlreadyFinalized);
        }
        serde_json::to_writer(&mut self.writer, &self.buffered)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        self.finalized = true;
        Ok(())
    }
}
