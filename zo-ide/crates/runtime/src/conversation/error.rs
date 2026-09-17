//! Error types returned by the conversation runtime.
//!
//! Three error surfaces live here:
//!
//! - [`ToolError`] — a tool invocation failed locally. Pure value type so
//!   the executor trait stays object-safe.
//! - [`RuntimeError`] — a turn could not be completed (API failure, bad
//!   assistant message, exceeded iterations, …).
//! - [`StreamingTurnError`] — the async/streaming variant. Adds a
//!   `Cancelled` arm for the case where the render-block receiver was
//!   dropped, and a `Permission` arm for async prompt failures.

use std::fmt::{Display, Formatter};

use api::{ApiError, ProviderErrorClass};

use crate::permission::PermissionError as AsyncPermissionError;

/// Error returned when a tool invocation fails locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
    /// A declined verdict rather than a failure: the dispatch policy kept the
    /// work inline and the message says what to do instead. Nothing broke.
    declined: bool,
}

impl ToolError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            declined: false,
        }
    }

    /// The policy's verdict on a call it will not run — not a failure.
    #[must_use]
    pub fn declined(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            declined: true,
        }
    }

    #[must_use]
    pub const fn is_declined(&self) -> bool {
        self.declined
    }

    /// How the text this error carries is to be read.
    #[must_use]
    pub const fn kind(&self) -> ToolTextKind {
        if self.declined {
            ToolTextKind::Declined
        } else {
            ToolTextKind::Failure
        }
    }
}

/// How a dispatched tool's text is to be read: an answer, a failure, or the
/// dispatch policy's declined verdict. The wire has only `is_error` (the
/// provider contract has no third state); the screen has three headings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTextKind {
    Answer,
    Failure,
    Declined,
}

impl ToolTextKind {
    /// What the provider's tool-result contract is told: a verdict is an
    /// error there, so the model reads the instruction as one.
    #[must_use]
    pub const fn is_error(self) -> bool {
        !matches!(self, Self::Answer)
    }
}

impl Display for ToolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

/// Error returned when a conversation turn cannot be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    message: String,
    provider_error_class: Option<ProviderErrorClass>,
}

impl RuntimeError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            provider_error_class: None,
        }
    }

    #[must_use]
    pub fn with_provider_error_class(
        message: impl Into<String>,
        provider_error_class: ProviderErrorClass,
    ) -> Self {
        Self {
            message: message.into(),
            provider_error_class: Some(provider_error_class),
        }
    }

    #[must_use]
    pub fn from_api_error(error: &ApiError) -> Self {
        Self::with_provider_error_class(error.to_string(), error.provider_error_class())
    }

    #[must_use]
    pub fn provider_error_class(&self) -> Option<ProviderErrorClass> {
        self.provider_error_class
    }

    #[must_use]
    pub fn failure_signature(&self) -> &'static str {
        match self.provider_error_class {
            Some(ProviderErrorClass::RateLimit { .. }) => "provider_rate_limit",
            Some(ProviderErrorClass::Transient) => "provider_transient",
            Some(ProviderErrorClass::AuthExpired) => "provider_auth_expired",
            Some(ProviderErrorClass::ContextOverflow) => "provider_context_overflow",
            Some(ProviderErrorClass::InvalidToolProtocol) => "provider_invalid_tool_protocol",
            Some(ProviderErrorClass::InvalidToolSchema) => "provider_invalid_tool_schema",
            Some(ProviderErrorClass::SafetyBlocked) => "provider_safety_blocked",
            Some(ProviderErrorClass::ContentRefused) => "provider_content_refused",
            Some(ProviderErrorClass::NonRetryable) => "provider_non_retryable",
            None => "runtime_error",
        }
    }
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// Errors surfaced by the streaming turn variant.
///
/// One enum per module per the living standard. Wraps the legacy
/// [`RuntimeError`] so callers driving the streaming path from async
/// code can `?` both sources without pulling in `anyhow`.
#[derive(Debug, thiserror::Error)]
pub enum StreamingTurnError {
    /// The agent loop failed in the same way the synchronous `run_turn`
    /// would fail (API error, bad assistant message, max iterations, …).
    #[error("runtime: {0}")]
    Runtime(RuntimeError),

    /// The consumer dropped the `RenderBlock` receiver before the turn
    /// finished. The agent loop treats this as a clean cancellation.
    #[error("render block receiver dropped — turn cancelled")]
    Cancelled,

    /// The async permission prompter returned an error.
    #[error("permission: {0}")]
    Permission(#[from] AsyncPermissionError),
}

impl StreamingTurnError {
    #[must_use]
    pub fn runtime(message: impl Into<String>) -> Self {
        StreamingTurnError::Runtime(RuntimeError::new(message))
    }

    #[must_use]
    pub fn provider_error_class(&self) -> Option<ProviderErrorClass> {
        match self {
            StreamingTurnError::Runtime(error) => error.provider_error_class(),
            StreamingTurnError::Cancelled | StreamingTurnError::Permission(_) => None,
        }
    }
}

impl From<RuntimeError> for StreamingTurnError {
    fn from(error: RuntimeError) -> Self {
        StreamingTurnError::Runtime(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn runtime_error_preserves_provider_error_class_without_changing_display() {
        let err = RuntimeError::with_provider_error_class(
            "api returned 429",
            ProviderErrorClass::account_rate_limit(Some(Duration::from_secs(3))),
        );
        assert_eq!(err.to_string(), "api returned 429");
        assert_eq!(
            err.provider_error_class(),
            Some(ProviderErrorClass::account_rate_limit(Some(Duration::from_secs(3))))
        );
    }

    #[test]
    fn runtime_error_failure_signature_is_structured_and_coarse() {
        let transient = RuntimeError::with_provider_error_class(
            "http error: error decoding response body at /private/path",
            ProviderErrorClass::Transient,
        );
        assert_eq!(transient.failure_signature(), "provider_transient");

        let local = RuntimeError::new("failed to parse /tmp/secret/session.json");
        assert_eq!(local.failure_signature(), "runtime_error");
    }

    #[test]
    fn streaming_turn_error_keeps_existing_display_for_classified_runtime_error() {
        let err = StreamingTurnError::from(RuntimeError::with_provider_error_class(
            "provider transport: overloaded",
            ProviderErrorClass::account_rate_limit(None),
        ));
        assert_eq!(err.to_string(), "runtime: provider transport: overloaded");
        assert_eq!(
            err.provider_error_class(),
            Some(ProviderErrorClass::account_rate_limit(None))
        );
    }
}
