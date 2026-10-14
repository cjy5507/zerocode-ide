//! The newest request the current judgment client is known to have sent,
//! read off this machine's feature records of the current provider (t-53792)
//! — what the settings screen says beside "running" and "checked": whether
//! the model it shows has actually been asked a real feature's question.

use serde::Serialize;

use super::Wire;

/// Which records an answer was read from: this machine's records of the
/// current provider, not one project's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Machine,
}

/// Whose requests an answer can name: the window's, which record the client
/// they were put to — or none, when nothing could be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    WindowRequests,
    None,
}

/// What the records say of the current client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Observed,
    NoneObserved,
    Unknown,
}

/// Why an answer is [`State::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Unattributed,
    RecordsUnreadable,
    SettingsUnreadable,
}

/// The newest request the current client sent, as its feature recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRequest {
    pub feature: &'static str,
    pub at_ms: Option<i64>,
    pub outcome: String,
    pub elapsed_ms: Option<u64>,
    pub applied: Option<bool>,
}

/// The answer `judgment_latest_request` hands the settings screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestRequest {
    pub provider_identity: Option<String>,
    pub observed_at_ms: i64,
    pub scope: Scope,
    pub coverage: Coverage,
    pub state: State,
    pub reason: Option<Reason>,
    pub unattributed_newer: bool,
    pub last_request: Option<LastRequest>,
}

/// What the records `wire` reads say of the client it would ask now, read at
/// `now_ms`.
#[must_use]
pub fn read(wire: &Wire, now_ms: i64) -> LatestRequest {
    LatestRequest {
        provider_identity: wire.provider().ok().map(|provider| provider.identity()),
        observed_at_ms: now_ms,
        scope: Scope::Machine,
        coverage: Coverage::None,
        state: State::Unknown,
        reason: Some(Reason::Unattributed),
        unattributed_newer: false,
        last_request: None,
    }
}
