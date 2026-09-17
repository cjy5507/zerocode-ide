//! Shared wire-facing projection of render-stream events.

use super::{RenderBlock, ToolCallStatus, ToolResultBody};

/// The render-stream events consumed by external agent protocols.
#[derive(Debug, Clone, Copy)]
pub enum ProjectedRenderBlock<'a> {
    /// An assistant text delta.
    TextDelta {
        /// Stable render-block identifier.
        id: u64,
        /// Newly appended text.
        text: &'a str,
        /// Whether the logical text block is complete.
        done: bool,
    },
    /// A tool-call lifecycle update.
    ToolCall {
        /// Stable render-block identifier.
        id: u64,
        /// Provider-neutral tool-call identifier.
        tool_call_id: &'a str,
        /// Canonical tool name.
        name: &'a str,
        /// Short input summary.
        summary: &'a str,
        /// Current lifecycle status.
        status: ToolCallStatus,
    },
    /// A tool result correlated with a prior call.
    ToolResult {
        /// Stable render-block identifier.
        id: u64,
        /// Provider-neutral tool-call identifier.
        tool_call_id: &'a str,
        /// Whether execution failed.
        is_error: bool,
        /// Structured result body.
        body: &'a ToolResultBody,
    },
    /// A render event without an editor-protocol representation.
    Other,
}

/// Projects a [`RenderBlock`] into the shared external-protocol event vocabulary.
#[must_use]
pub fn project_render_block(block: &RenderBlock) -> ProjectedRenderBlock<'_> {
    match block {
        RenderBlock::TextDelta { id, text, done } => ProjectedRenderBlock::TextDelta {
            id: id.0,
            text,
            done: *done,
        },
        RenderBlock::ToolCall {
            id,
            tool_call_id,
            name,
            summary,
            status,
            ..
        } => ProjectedRenderBlock::ToolCall {
            id: id.0,
            tool_call_id: &tool_call_id.0,
            name,
            summary,
            status: *status,
        },
        RenderBlock::ToolResult {
            id,
            tool_call_id,
            is_error,
            body,
        } => ProjectedRenderBlock::ToolResult {
            id: id.0,
            tool_call_id: &tool_call_id.0,
            is_error: *is_error,
            body,
        },
        _ => ProjectedRenderBlock::Other,
    }
}
