//! Provider-agnostic message stream layer.
//!
//! This module is the Phase-3 boundary between `runtime::conversation`
//! (which drives turns) and the future `tui::app` (which renders them).
//! Its only currency across that boundary is [`RenderBlock`] — see
//! `code-rules.md` R1.
//!
//! ## Layout
//!
//! * [`types`] — provider-neutral [`RenderBlock`] + supporting types.
//! * [`provider`] — [`ProviderStream`] trait, [`ProviderRegistry`],
//!   [`ProviderId`], and the shared [`StreamError`] enum.
//! * [`anthropic`] — the first (and today only) adapter. Owns all
//!   Anthropic-specific wire format handling. Nothing leaks upward.
//!
//! ## Living standard (L1 pilot)
//!
//! L1 establishes the patterns L2–L7 must follow. See
//! `.zo/tasks/L1-message-stream.handoff.md` for the full list; the
//! short version:
//!
//! 1. Each adapter lives in its own submodule with at least `mod.rs`,
//!    `parser.rs`, and `tools.rs`.
//! 2. Errors use `thiserror`-derived enums, one per module
//!    ([`StreamError`] is the cross-module catch-all).
//! 3. Async traits use the native `async fn` → `Pin<Box<dyn Future>>`
//!    pattern (no `async-trait` crate) because the workspace targets
//!    stable Rust ≥ 1.75.
//! 4. Tests are named `mod_<scenario>` and live under
//!    `crates/runtime/tests/message_stream_*.rs`.
//! 5. Every `pub` item carries a doc comment whose first sentence is a
//!    one-line summary.

pub mod anthropic;
mod projection;
pub mod provider;
pub mod types;

pub use anthropic::AnthropicStream;
pub use projection::{ProjectedRenderBlock, project_render_block};
pub use provider::{
    ProviderId, ProviderRegistry, ProviderRequest, ProviderStream, StreamError, TurnSummary,
};
pub use types::{
    ActiveModel, AgentResultStatus, BashResult, BlockId, BlockIdGen, DiffHunk, DiffLine,
    DiffLineKind, DiffView, NotificationRoad, PermissionChoice, PermissionDecision,
    PermissionPrompt, QuestionOption, RenderBlock, SystemLevel, TodoResultItem, TodoResultStatus, ToolCallId, ToolCallStatus,
    ToolPresentation, ToolPreview, ToolResultBody, UserQuestionPrompt, WireModel,
    WireModelSource,
};
