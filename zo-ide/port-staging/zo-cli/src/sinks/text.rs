//! Plain-text sink.
//!
//! Emits a human-readable, ANSI-free transcript of a conversation
//! turn. Mirrors the structure the legacy `session::run_turn` text path
//! writes to stdout. During L4 the legacy path in `main.rs`/`session.rs`
//! remains authoritative for `--output-format text` to keep the parity
//! harness byte-equivalent; this sink is the new extraction point and
//! is exercised directly by the sinks integration tests.

use std::io::Write;

use runtime::message_stream::{RenderBlock, ToolResultBody};

use super::{Sink, SinkError};

/// Plain-text streaming sink.
pub struct TextSink<W: Write> {
    writer: W,
    finalized: bool,
}

impl<W: Write> TextSink<W> {
    /// Create a new text sink that writes to `writer`.
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            finalized: false,
        }
    }
}

impl<W: Write> Sink for TextSink<W> {
    fn emit(&mut self, block: &RenderBlock) -> Result<(), SinkError> {
        if self.finalized {
            return Err(SinkError::AlreadyFinalized);
        }
        match block {
            RenderBlock::TextDelta { text, .. } => {
                self.writer.write_all(text.as_bytes())?;
            }
            RenderBlock::Reasoning { text, .. } => {
                // Reasoning is rendered dimmed in the TUI; the text sink
                // prefixes each chunk so machine consumers can strip it.
                self.writer.write_all(b"[thinking] ")?;
                self.writer.write_all(text.as_bytes())?;
                self.writer.write_all(b"\n")?;
            }
            RenderBlock::ToolCall { name, summary, .. } => {
                writeln!(self.writer, "[tool {name}] {summary}")?;
            }
            RenderBlock::ToolResult { is_error, body, .. } => {
                let tag = if *is_error {
                    "tool-error"
                } else {
                    "tool-result"
                };
                writeln!(self.writer, "[{tag}] {}", tool_result_preview(body))?;
            }
            RenderBlock::Image {
                media_type, data, ..
            } => {
                writeln!(self.writer, "[image: {media_type}, {} bytes]", data.len())?;
            }
            RenderBlock::UserMessage { text, .. } => {
                writeln!(self.writer, "[user] {text}")?;
            }
            RenderBlock::UserNotice { message, .. } => {
                writeln!(self.writer, "[to-user] {message}")?;
            }
            RenderBlock::AgentResult {
                label, body, ..
            } => {
                // Headless/plain consumers get the full agent body (the card is a
                // TUI-only affordance); the label keeps provenance.
                writeln!(self.writer, "[agent {label}] {body}")?;
            }
            RenderBlock::System { text, .. } => {
                writeln!(self.writer, "[system] {text}")?;
            }
            RenderBlock::PermissionPrompt(prompt) => {
                writeln!(
                    self.writer,
                    "[permission] {}: {}",
                    prompt.tool_name, prompt.reasoning
                )?;
                if let Some(audit_hint) = &prompt.audit_hint {
                    writeln!(self.writer, "[permission-audit] {audit_hint}")?;
                }
            }
            RenderBlock::UserQuestionPrompt(prompt) => {
                writeln!(self.writer, "[question] {}", prompt.question)?;
            }
            RenderBlock::Card { card, .. } => {
                writeln!(self.writer, "{}", card.plain_text())?;
            }
            // Usage is live-ledger telemetry, not assistant output — emitting it
            // here would corrupt the plain-text stream. Surfaced via ndjson only.
            RenderBlock::Usage { .. }
            | RenderBlock::CompactionProgress { .. }
            | RenderBlock::RateLimit(_) => {}
        }
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

/// Best-effort one-line preview of a [`ToolResultBody`]. Shared by the
/// text sink and the JSON-shaped sinks' serializable projection.
pub(crate) fn tool_result_preview(body: &ToolResultBody) -> String {
    match body {
        ToolResultBody::Text { content, truncated } => truncated_mark(content, *truncated),
        ToolResultBody::Bash(result) => format!(
            "exit={} stdout_len={} stderr_len={}",
            result.exit_code,
            result.stdout.len(),
            result.stderr.len()
        ),
        ToolResultBody::Read {
            path,
            content,
            truncated,
            ..
        } => format!(
            "{path} ({} bytes){}",
            content.len(),
            trunc_suffix(*truncated)
        ),
        ToolResultBody::Diff(diff) => {
            let old = diff.old_path.as_deref().unwrap_or("<new>");
            let new = diff.new_path.as_deref().unwrap_or("<deleted>");
            format!("{old} -> {new} ({} hunks)", diff.hunks.len())
        }
        ToolResultBody::Listing { entries, truncated } => {
            format!("{} entries{}", entries.len(), trunc_suffix(*truncated))
        }
        ToolResultBody::Generic {
            name,
            content,
            truncated,
        } => format!("{name}: {}", truncated_mark(content, *truncated)),
        ToolResultBody::Todos(items) => {
            let done = items
                .iter()
                .filter(|item| {
                    matches!(
                        item.status,
                        runtime::message_stream::TodoResultStatus::Completed
                    )
                })
                .count();
            format!("todos {done}/{} done", items.len())
        }
    }
}

fn truncated_mark(content: &str, truncated: bool) -> String {
    let mut first_line = content.lines().next().unwrap_or("").to_string();
    if truncated {
        first_line.push_str(" …");
    }
    first_line
}

const fn trunc_suffix(truncated: bool) -> &'static str {
    if truncated { " (truncated)" } else { "" }
}
