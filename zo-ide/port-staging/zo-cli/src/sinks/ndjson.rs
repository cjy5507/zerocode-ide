//! Newline-delimited JSON sink.
//!
//! Mirrors the Claude Code `stream-json` output format: one JSON object
//! per line, written as-events-arrive. Consumers can `jq -c` over the
//! stream without buffering.

use std::io::Write;

use runtime::message_stream::RenderBlock;

use super::serializable::SerializableRenderBlock;
use super::{Sink, SinkError};

/// Streaming NDJSON sink.
///
/// Serializes every [`RenderBlock`] to a single line of JSON on the
/// wrapped writer, followed by `\n`. Flushing is explicit via
/// [`Sink::finalize`].
pub struct NdjsonSink<W: Write> {
    writer: W,
    finalized: bool,
}

impl<W: Write> NdjsonSink<W> {
    /// Create a new NDJSON sink that writes to `writer`.
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            finalized: false,
        }
    }

    /// Consume the sink and return the wrapped writer. Primarily used
    /// by tests that need to inspect captured bytes.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> Sink for NdjsonSink<W> {
    fn emit(&mut self, block: &RenderBlock) -> Result<(), SinkError> {
        if self.finalized {
            return Err(SinkError::AlreadyFinalized);
        }
        let projection = SerializableRenderBlock::from_block(block);
        serde_json::to_writer(&mut self.writer, &projection)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }

    fn finalize(mut self: Box<Self>) -> Result<(), SinkError> {
        if self.finalized {
            return Err(SinkError::AlreadyFinalized);
        }
        self.writer.flush()?;
        self.finalized = true;
        Ok(())
    }
}
