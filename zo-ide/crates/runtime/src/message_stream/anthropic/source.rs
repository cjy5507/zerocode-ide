//! Async event sources feeding the Anthropic `parser`.
//!
//! L1 only had a synchronous (pre-loaded) source: tests pushed a
//! `Vec<StreamEvent>` through [`super::parser::parse_stream`] and the
//! adapter drained it. L7b adds a real HTTP path on top of the same
//! parser by routing every event through this async [`EventSource`]
//! seam.
//!
//! Living standard:
//!
//! * R1 — nothing above this module names Anthropic wire types.
//! * R6 — sources surface transport / protocol failures via
//!   `super::provider::StreamError`; they never panic, never silently
//!   drop frames.
//! * R8 — sources are pull-based (`next_event().await`) so backpressure
//!   on the downstream `RenderBlock` channel naturally pauses HTTP body
//!   reads.

use std::pin::Pin;

use api::StreamEvent;
use core_types::RateLimitSnapshot;

use crate::message_stream::provider::StreamError;

/// Pull-based async event source.
///
/// Implementors return one [`StreamEvent`] per `next_event().await`
/// call until the upstream signals completion (`Ok(None)`). The parser
/// drives this trait until completion or until the consumer drops the
/// `RenderBlock` channel.
pub trait EventSource: Send {
    /// Fetch the next event, or `Ok(None)` when the stream has
    /// finished cleanly.
    fn next_event<'a>(
        &'a mut self,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Option<StreamEvent>, StreamError>> + Send + 'a>,
    >;

    /// Unified rate-limit snapshot from the transport's response headers, if
    /// any. Default `None` — in-memory and test sources have no headers; only
    /// [`HttpSource`] overrides this.
    fn rate_limit(&self) -> Option<RateLimitSnapshot> {
        None
    }
}

// ============================================================================
// In-memory iterator source (used by L1 + tests)
// ============================================================================

/// In-memory [`EventSource`] backed by a `Vec<StreamEvent>`.
///
/// Used by the legacy preloaded constructor and by every fixture-based
/// test that does not need a real HTTP transport.
#[derive(Debug)]
pub struct VecSource {
    events: std::collections::VecDeque<StreamEvent>,
}

impl VecSource {
    /// Wrap an in-memory event sequence.
    #[must_use]
    pub fn new(events: Vec<StreamEvent>) -> Self {
        Self {
            events: events.into(),
        }
    }
}

impl EventSource for VecSource {
    fn next_event<'a>(
        &'a mut self,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Option<StreamEvent>, StreamError>> + Send + 'a>,
    > {
        Box::pin(async move { Ok(self.events.pop_front()) })
    }
}

// ============================================================================
// HTTP source (production wiring)
// ============================================================================

/// [`EventSource`] backed by a live `api::MessageStream` (the real HTTP
/// SSE pipe coming out of `AnthropicClient::stream_message`).
///
/// Each `next_event` call awaits one decoded SSE frame. Transport and
/// parse failures from the `api` crate are mapped onto
/// [`StreamError::Transport`]. Cancellation is observed at the
/// downstream channel boundary inside the parser — when the
/// `RenderBlock` receiver drops, the parser stops calling
/// `next_event`, the `MessageStream` falls out of scope, and reqwest
/// aborts the in-flight body read.
pub struct HttpSource {
    inner: api::MessageStream,
}

impl HttpSource {
    /// Wrap a live `api::MessageStream`.
    #[must_use]
    pub fn new(inner: api::MessageStream) -> Self {
        Self { inner }
    }
}

impl std::fmt::Debug for HttpSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSource").finish_non_exhaustive()
    }
}

impl EventSource for HttpSource {
    fn next_event<'a>(
        &'a mut self,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Option<StreamEvent>, StreamError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.inner.next_event().await.map_err(|err| {
                StreamError::classified_transport(err.to_string(), err.provider_error_class())
            })
        })
    }

    fn rate_limit(&self) -> Option<RateLimitSnapshot> {
        self.inner.rate_limit()
    }
}
