//! The newest request the current judgment client is known to have sent,
//! read off this machine's feature records of the current provider (t-53792)
//! — what the settings screen says beside "running" and "checked": whether
//! the model it shows has actually been asked a real feature's question.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use zerocode_core::jev::summary::{
    AT, CACHED, CLIENT_ROUTE, ELAPSED_MS, REQUESTS, applied_of, asked_something,
};
use zerocode_core::jev::{JEV_USES, count};

use super::{Wire, observed};

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
///
/// One reading of the provider names the client, its identity and the
/// folder its records are filed in, so the three come from one moment of the
/// settings. Each feature's record is read only as far back as its newest
/// bytes ([`zerocode_core::transcript::tail_lines`]). A request is one that
/// left: a count above zero and no memo answer. It is this client's when its
/// row names this client's route ([`CLIENT_ROUTE`]), another's when it names
/// another, and nobody's to say when it names none or carries no count — a
/// row written before rows named the route, or by a writer that does not
/// name it. Nothing is asked and nothing is written.
#[must_use]
pub fn read(wire: &Wire, now_ms: i64) -> LatestRequest {
    let unknown = |identity: Option<String>, coverage: Coverage, reason: Reason| LatestRequest {
        provider_identity: identity,
        observed_at_ms: now_ms,
        scope: Scope::Machine,
        coverage,
        state: State::Unknown,
        reason: Some(reason),
        unattributed_newer: false,
        last_request: None,
    };
    let Ok(provider) = wire.provider() else {
        return unknown(None, Coverage::None, Reason::SettingsUnreadable);
    };
    let identity = provider.identity();
    let Some(home) = wire.config_home() else {
        return unknown(Some(identity), Coverage::None, Reason::SettingsUnreadable);
    };
    let route = provider.route_fingerprint();
    let folder = provider.evidence_dir(&home.join(count::REQUESTS_DIR));
    let mut newest: Option<(i64, LastRequest)> = None;
    let mut unattributed: Option<i64> = None;
    for seat in &JEV_USES {
        let ledger = folder.join(seat.ledger);
        let Some(rows) = tail_rows(&ledger) else {
            return unknown(
                Some(identity),
                Coverage::WindowRequests,
                Reason::RecordsUnreadable,
            );
        };
        for row in observed(&ledger, rows) {
            let Some(outcome) = asked_something(&row) else {
                continue;
            };
            let requests = REQUESTS.read(&row).and_then(Value::as_u64);
            if requests == Some(0) || CACHED.read(&row).and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let at = AT.read(&row).and_then(Value::as_i64);
            let ordered = at.unwrap_or(i64::MIN);
            match (requests, CLIENT_ROUTE.read(&row).and_then(Value::as_str)) {
                (Some(_), Some(sent)) if sent == route => {
                    if newest.as_ref().is_none_or(|(held, _)| ordered >= *held) {
                        newest = Some((
                            ordered,
                            LastRequest {
                                feature: seat.id,
                                at_ms: at,
                                outcome: outcome.to_string(),
                                elapsed_ms: ELAPSED_MS.read(&row).and_then(Value::as_u64),
                                applied: applied_of(&row),
                            },
                        ));
                    }
                }
                (Some(_), Some(_)) => {}
                _ => unattributed = Some(unattributed.map_or(ordered, |held| held.max(ordered))),
            }
        }
    }
    match newest {
        Some((at, last)) => LatestRequest {
            provider_identity: Some(identity),
            observed_at_ms: now_ms,
            scope: Scope::Machine,
            coverage: Coverage::WindowRequests,
            state: State::Observed,
            reason: None,
            unattributed_newer: unattributed.is_some_and(|held| held > at),
            last_request: Some(last),
        },
        None if unattributed.is_some() => unknown(
            Some(identity),
            Coverage::WindowRequests,
            Reason::Unattributed,
        ),
        None => LatestRequest {
            provider_identity: Some(identity),
            observed_at_ms: now_ms,
            scope: Scope::Machine,
            coverage: Coverage::WindowRequests,
            state: State::NoneObserved,
            reason: None,
            unattributed_newer: false,
            last_request: None,
        },
    }
}

/// The rows in the newest bytes of `ledger`: none for a record that is not
/// there or holds nothing, `None` for one that is there and does not read.
fn tail_rows(ledger: &Path) -> Option<Vec<Value>> {
    match std::fs::metadata(ledger) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
        Ok(meta) if !meta.is_file() => return None,
        Ok(meta) if meta.len() == 0 => return Some(Vec::new()),
        Ok(_) => {}
    }
    let lines = zerocode_core::transcript::tail_lines(ledger)?;
    if lines.is_empty() {
        return None;
    }
    lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .filter(Value::is_object)
        })
        .collect()
}
