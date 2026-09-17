//! Provider seam (`ProviderStream` trait + [`ProviderRegistry`]).
//!
//! The entire point of [`ProviderStream`] is to let `agent_loop` stay
//! provider-agnostic. Today the only implementation is
//! [`crate::message_stream::anthropic::AnthropicStream`]. Tomorrow a
//! `CodexStream` (`OpenAI` Responses API) plugs into the same registry
//! without the TUI, the channel layer, or the agent loop learning
//! anything new.
//!
//! ## Design choices (living standard for L2–L7)
//!
//! * **Native async trait** — we target stable Rust ≥ 1.75, so
//!   `async fn` in traits is legal without the `async-trait` crate.
//!   See the handoff note for the factcheck.
//! * **Stream currency** is `Result<RenderBlock, StreamError>` pushed
//!   through an `mpsc::Sender`, *not* a returned
//!   `impl Stream<Item = …>`. This keeps backpressure honest (the
//!   channel is bounded per `code-rules.md` R8) and lets adapters
//!   interleave multiple in-flight tool calls without wrestling with
//!   pinned self-referential streams.
//! * **Registry lookup only** — no discovery, no side effects in
//!   `get`. Construction happens once at startup in the binary crate.

use std::collections::HashMap;
use std::sync::Arc;

use api::ProviderErrorClass;
use tokio::sync::mpsc;

use super::types::{BlockIdGen, RenderBlock};

// ============================================================================
// Provider identity
// ============================================================================

/// Stable provider identifier.
///
/// Newtype around `&'static str` to prevent accidental mixing with
/// arbitrary user input. The only legal values today are
/// `"anthropic"`; future adapters register `"codex"`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderId(pub &'static str);

impl ProviderId {
    /// Return the underlying static string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Error type surfaced by [`ProviderStream`] implementations.
///
/// Follows the living-standard error pattern for Phase 3: one
/// `thiserror`-derived enum per module. Adapters wrap their native
/// errors via the catch-all [`StreamError::Adapter`] variant.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// The render-block channel was closed before the adapter
    /// finished emitting. Typically means the TUI dropped.
    #[error("render block channel closed")]
    ChannelClosed,

    /// The upstream provider returned a transport-level failure.
    #[error("transport error: {0}")]
    Transport(String),

    /// The upstream provider returned a transport-level failure with
    /// structured provider semantics preserved from `api::ApiError`.
    #[error("transport error: {message}")]
    ClassifiedTransport {
        message: String,
        provider_error_class: ProviderErrorClass,
    },

    /// The upstream provider sent a malformed event.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Provider-specific failure wrapped for propagation.
    #[error("{provider}: {message}")]
    Adapter {
        /// Provider id, for log correlation.
        provider: &'static str,
        /// Human-readable message.
        message: String,
    },
}

impl StreamError {
    #[must_use]
    pub fn classified_transport(
        message: impl Into<String>,
        provider_error_class: ProviderErrorClass,
    ) -> Self {
        Self::ClassifiedTransport {
            message: message.into(),
            provider_error_class,
        }
    }

    #[must_use]
    pub fn provider_error_class(&self) -> Option<ProviderErrorClass> {
        match self {
            Self::ClassifiedTransport {
                provider_error_class,
                ..
            } => Some(*provider_error_class),
            Self::ChannelClosed | Self::Transport(_) | Self::Protocol(_) | Self::Adapter { .. } => {
                None
            }
        }
    }

    #[must_use]
    pub fn transport_message(&self) -> Option<&str> {
        match self {
            Self::Transport(message) | Self::ClassifiedTransport { message, .. } => Some(message),
            Self::ChannelClosed | Self::Protocol(_) | Self::Adapter { .. } => None,
        }
    }
}

impl<T> From<mpsc::error::SendError<T>> for StreamError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        Self::ChannelClosed
    }
}

// ============================================================================
// Provider request + summary
// ============================================================================

/// Neutral per-turn request description.
///
/// Each adapter converts this into its native wire format internally.
/// The struct is intentionally small in L1 — L3 (agent loop) will
/// extend it with message history and tool schemas.
#[derive(Debug, Clone)]
pub struct ProviderRequest {
    /// Canonical model alias (e.g. `"opus"`, `"sonnet"`, `"openai-latest"`).
    pub model: String,
    /// System prompt text, if any.
    pub system: Option<String>,
    /// Maximum output tokens the provider should emit.
    pub max_tokens: u32,
}

/// Neutral per-turn summary emitted when a stream completes.
#[derive(Debug, Clone, Default)]
pub struct TurnSummary {
    /// Provider-declared stop reason (e.g. `"end_turn"`, `"tool_use"`).
    pub stop_reason: Option<String>,
    /// Tokens consumed on input.
    pub input_tokens: u32,
    /// Tokens consumed on output.
    pub output_tokens: u32,
}

// ============================================================================
// Provider stream trait
// ============================================================================

/// Streaming provider adapter.
///
/// Implementors consume their own wire format and push
/// [`RenderBlock`]s through `out` until the turn completes.
pub trait ProviderStream: Send + Sync {
    /// Stable provider id.
    fn id(&self) -> ProviderId;

    /// Drive a single turn to completion.
    ///
    /// The adapter **must**:
    ///
    /// 1. Push every renderable piece of the turn through `out`.
    /// 2. Preserve reasoning deltas as [`RenderBlock::Reasoning`]
    ///    (per `code-rules.md` R6).
    /// 3. Return `Err(StreamError::ChannelClosed)` without panicking
    ///    if the receiver has dropped.
    fn stream_turn<'a>(
        &'a self,
        request: ProviderRequest,
        out: mpsc::Sender<RenderBlock>,
        ids: BlockIdGen,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<TurnSummary, StreamError>> + Send + 'a>,
    >;
}

// ============================================================================
// Registry
// ============================================================================

/// Boxed adapter handle stored in the registry.
pub type BoxedProvider = Arc<dyn ProviderStream>;

/// Lookup table of registered providers.
///
/// Construction happens once at startup; `get` / `list_models` are
/// pure reads.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<ProviderId, BoxedProvider>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ProviderRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider. Returns the previous entry for `id`, if any.
    pub fn register(&mut self, provider: BoxedProvider) -> Option<BoxedProvider> {
        let id = provider.id();
        self.providers.insert(id, provider)
    }

    /// Look up a provider by id.
    #[must_use]
    pub fn get(&self, id: ProviderId) -> Option<BoxedProvider> {
        self.providers.get(&id).cloned()
    }

    /// Iterate over all registered provider ids.
    pub fn list_providers(&self) -> impl Iterator<Item = ProviderId> + '_ {
        self.providers.keys().copied()
    }

    /// Number of registered providers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// `true` if no providers are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}
