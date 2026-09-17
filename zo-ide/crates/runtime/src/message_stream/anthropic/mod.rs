//! Anthropic [`ProviderStream`] adapter.
//!
//! This is the only module in the crate allowed to name Anthropic
//! wire types (`StreamEvent`, `ContentBlockDelta`, …). Everything it
//! exposes upwards is already provider-neutral: [`RenderBlock`],
//! [`crate::message_stream::provider::TurnSummary`], etc.
//!
//! The adapter is split into:
//!
//! * [`parser`] — stateful SSE → [`RenderBlock`] translator.
//! * [`tools`] — 15 pure, structured tool formatters (R4).
//! * this file — the [`AnthropicStream`] handle that implements
//!   [`ProviderStream`].

pub mod parser;
pub mod source;
pub mod tools;

use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::provider::{ProviderId, ProviderRequest, ProviderStream, StreamError, TurnSummary};
use super::types::{BlockIdGen, RenderBlock};

use api::StreamEvent;

use self::source::{EventSource, HttpSource};

/// Boxed [`EventSource`] returned from a [`StreamSourceFactory`].
pub type BoxedEventSource = Box<dyn EventSource + Send>;

/// Future returned from [`StreamSourceFactory::create`].
pub type EventSourceFuture<'a> =
    Pin<Box<dyn std::future::Future<Output = Result<BoxedEventSource, StreamError>> + Send + 'a>>;

/// Async factory that produces a fresh [`EventSource`] for one turn.
///
/// L7b's HTTP path supplies a closure that calls
/// `api::ProviderClient::stream_message(..)` and wraps the resulting
/// `api::MessageStream` in [`HttpSource`]. Stored as a trait object
/// behind `Arc` so the [`AnthropicStream`] adapter can implement
/// `Send + Sync` while owning provider-specific state (HTTP client,
/// auth, request builder).
pub trait StreamSourceFactory: Send + Sync {
    /// Produce a fresh event source for the given request.
    fn create(&self, request: ProviderRequest) -> EventSourceFuture<'_>;
}

/// Stable provider id for the Anthropic adapter.
pub const ANTHROPIC_PROVIDER_ID: ProviderId = ProviderId("anthropic");

/// The Anthropic [`ProviderStream`] adapter.
///
/// L1 shipped with an in-process "pre-collected events" constructor
/// ([`AnthropicStream::from_events`]) used by snapshot tests. L7b adds
/// the production HTTP path: a [`StreamSourceFactory`] that builds a
/// fresh [`HttpSource`] (wrapping a live `api::MessageStream`) per
/// turn. The two paths are mutually exclusive; the factory wins when
/// both are present.
#[derive(Default)]
pub struct AnthropicStream {
    preloaded: std::sync::Mutex<Vec<StreamEvent>>,
    factory: Option<Arc<dyn StreamSourceFactory>>,
}

impl std::fmt::Debug for AnthropicStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicStream")
            .field("has_factory", &self.factory.is_some())
            .finish_non_exhaustive()
    }
}

impl AnthropicStream {
    /// Create an empty adapter. `stream_turn` will yield an empty
    /// [`TurnSummary`] until either preloaded events are supplied via
    /// [`AnthropicStream::from_events`] or a streaming source is
    /// supplied via [`AnthropicStream::with_source_factory`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct an adapter that replays a captured sequence of
    /// events. Used by snapshot tests and L1's bootstrap path.
    #[must_use]
    pub fn from_events(events: Vec<StreamEvent>) -> Self {
        Self {
            preloaded: std::sync::Mutex::new(events),
            factory: None,
        }
    }

    /// Construct an adapter wired to a real streaming source.
    ///
    /// L7b production path: pass a factory that builds an
    /// [`HttpSource`] over `api::ProviderClient::stream_message`. The
    /// adapter calls the factory once per `stream_turn` invocation and
    /// drives the resulting source through the parser.
    #[must_use]
    pub fn with_source_factory(factory: Arc<dyn StreamSourceFactory>) -> Self {
        Self {
            preloaded: std::sync::Mutex::new(Vec::new()),
            factory: Some(factory),
        }
    }

    /// Parse a raw sequence of events directly, without going through
    /// the trait object path. Useful for tests.
    pub async fn parse(
        events: Vec<StreamEvent>,
        out: mpsc::Sender<RenderBlock>,
        ids: BlockIdGen,
    ) -> Result<TurnSummary, StreamError> {
        parser::parse_stream(events, out, ids).await
    }

    /// Parse an arbitrary [`EventSource`] directly. Used by L7b's
    /// integration tests to feed fixture-derived events through the
    /// exact production code path without needing a real HTTP server.
    pub async fn parse_source<S: EventSource>(
        source: S,
        out: mpsc::Sender<RenderBlock>,
        ids: BlockIdGen,
    ) -> Result<TurnSummary, StreamError> {
        parser::parse_stream_async(source, out, ids).await
    }

    /// L7c-1b helper: parse an [`EventSource`] while co-emitting both
    /// the TUI-facing [`RenderBlock`] stream (through `out`) **and** the
    /// [`crate::conversation::AssistantEvent`] sequence required by
    /// `ConversationRuntime::run_turn_streaming`'s bookkeeping path.
    ///
    /// The L7c `AsyncApiClient` implementation calls this so a single
    /// SSE consumption feeds both downstream consumers without
    /// duplicating the transport or building an inverse
    /// `RenderBlock → AssistantEvent` adapter. See
    /// [`parser::parse_stream_async_with_events`] for the guarantees.
    pub async fn parse_source_with_events<S: EventSource>(
        source: S,
        out: mpsc::Sender<RenderBlock>,
        ids: BlockIdGen,
    ) -> Result<parser::StreamOutputs, StreamError> {
        parser::parse_stream_async_with_events(source, out, ids).await
    }
}

impl ProviderStream for AnthropicStream {
    fn id(&self) -> ProviderId {
        ANTHROPIC_PROVIDER_ID
    }

    fn stream_turn<'a>(
        &'a self,
        request: ProviderRequest,
        out: mpsc::Sender<RenderBlock>,
        ids: BlockIdGen,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<TurnSummary, StreamError>> + Send + 'a>,
    > {
        let factory = self.factory.clone();
        let events = self
            .preloaded
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default();
        Box::pin(async move {
            if let Some(factory) = factory {
                let source = factory.create(request).await?;
                parser::parse_stream_async(BoxedSource(source), out, ids).await
            } else {
                parser::parse_stream(events, out, ids).await
            }
        })
    }
}

/// Newtype wrapper to forward [`EventSource`] across a `Box<dyn ..>`
/// without forcing the trait itself to be object-safe at every call
/// site.
struct BoxedSource(Box<dyn EventSource + Send>);

impl EventSource for BoxedSource {
    fn next_event<'a>(
        &'a mut self,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Option<StreamEvent>, StreamError>> + Send + 'a>,
    > {
        self.0.next_event()
    }
}

// ============================================================================
// Production HTTP factory
// ============================================================================

/// Request-builder closure type alias used by [`AnthropicHttpFactory`].
pub type RequestBuilder = Arc<dyn Fn(&ProviderRequest) -> api::MessageRequest + Send + Sync>;

/// Factory that produces an [`HttpSource`] from a shared
/// `api::ProviderClient` plus a request-builder closure.
///
/// The closure converts a provider-neutral [`ProviderRequest`] into
/// the wire-level `api::MessageRequest`. Keeping it as a closure lets
/// the runtime call site (which already owns history, tool schemas,
/// system prompt, etc.) supply the full request without leaking those
/// concerns into the `message_stream` layer.
pub struct AnthropicHttpFactory {
    client: Arc<api::ProviderClient>,
    build_request: RequestBuilder,
}

impl AnthropicHttpFactory {
    /// Build a factory.
    #[must_use]
    pub fn new(client: Arc<api::ProviderClient>, build_request: RequestBuilder) -> Self {
        Self {
            client,
            build_request,
        }
    }
}

impl std::fmt::Debug for AnthropicHttpFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicHttpFactory")
            .finish_non_exhaustive()
    }
}

impl StreamSourceFactory for AnthropicHttpFactory {
    fn create(&self, request: ProviderRequest) -> EventSourceFuture<'_> {
        Box::pin(async move {
            let wire = (self.build_request)(&request);
            let stream = self.client.stream_message(&wire).await.map_err(|err| {
                StreamError::classified_transport(err.to_string(), err.provider_error_class())
            })?;
            Ok(Box::new(HttpSource::new(stream)) as BoxedEventSource)
        })
    }
}
