//! Shared domain types extracted from `runtime` for use across multiple crates.
//!
//! This crate owns session persistence types, token-usage accounting,
//! lane-event enumerations, SSE parsing, the minimal JSON value type
//! used for session serialization, and the canonical zo-home path
//! resolution shared by every layer (see [`paths`]).

pub mod card;
pub mod command_category;
pub mod council;
pub mod date;
pub mod helper_run;
pub mod hex;
pub mod json;
pub mod lane_events;
pub mod memory;
pub mod oauth;
pub mod paths;
pub mod permissions;
pub mod retry_signal;
pub mod session;
pub mod sse;
pub mod text;
pub mod usage;

pub use card::{CardElement, CardModel, CardTone};
pub use command_category::CommandCategory;
pub use council::CouncilOutcome;
pub use json::{JsonError, JsonValue};
pub use lane_events::{
    LaneEvent, LaneEventBlocker, LaneEventName, LaneEventStatus, LaneFailureClass,
};
pub use memory::{MemoryEntry, MemoryHit, MemoryRetriever};
pub use oauth::{
    OAuthAuthorizationRequest, OAuthCallbackParams, OAuthConfig, OAuthRefreshRequest,
    OAuthTokenExchangeRequest, OAuthTokenSet, OpenAiOAuthTokens, PkceChallengeMethod, PkceCodePair,
    parse_oauth_callback_query, parse_oauth_callback_request_target,
};
pub use permissions::PermissionMode;
pub use session::{
    AnchorSummary, ContentBlock, ConversationMessage, MessageRole, Session, SessionCompaction,
    SessionError, SessionFork, VaultRecord,
};
pub use retry_signal::{
    parse_quota_fallback_model, StreamNoticeKind, StreamRetryNotice, QUIET_REASONING_LABEL,
    QUOTA_FALLBACK_ACTIVE_NOTICE_PREFIX, QUOTA_HOLD_NOTICE_PREFIX, REFUSAL_FALLBACK_WARN,
};
pub use sse::{IncrementalSseParser, SseEvent};
pub use usage::{
    ModelPricing, OutputTokensDetails, RateLimitSnapshot, RateLimitWindow, RateLimitWindowKind,
    TokenUsage, UsageCostEstimate, UsageDashboardRecord, UsageDashboardSnapshot, UsageModelRow,
    UsagePeriodRow, UsageSavingsSummary, UsageTokenTotals, UsageTracker, format_usd,
    pricing_for_model,
};
