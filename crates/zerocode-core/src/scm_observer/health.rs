use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const RETRY_BASE_MS: u64 = 30_000;
const RETRY_MAX_MS: u64 = 15 * 60_000;
const SERVER_RETRY_MAX_MS: u64 = 24 * 60 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Discovery,
    Checks,
    Details,
    Reviews,
    Stack,
}

impl Operation {
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Checks => "CI",
            Self::Details => "CI details",
            Self::Reviews => "reviews",
            Self::Stack => "stack",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    MissingTool,
    Authentication,
    Forbidden,
    NotFound,
    RateLimited,
    Timeout,
    Unavailable,
    InvalidResponse,
    Refused,
}

impl FailureKind {
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::MissingTool => "missing_tool",
            Self::Authentication => "authentication",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::InvalidResponse => "invalid_response",
            Self::Refused => "refused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub retry_after_ms: Option<u64>,
}

impl From<FailureKind> for Failure {
    fn from(kind: FailureKind) -> Self {
        Self {
            kind,
            retry_after_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthChange {
    Failed(FailureKind),
    Recovered,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthState {
    pub last_attempt_ms: Option<i64>,
    pub last_success_ms: Option<i64>,
    pub failed_since_ms: Option<i64>,
    pub consecutive_failures: u32,
    pub next_retry_ms: Option<i64>,
    pub failure: Option<FailureKind>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Health {
    context: String,
    state: HealthState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthRow {
    pub root: String,
    pub operation: Operation,
    #[serde(flatten)]
    pub state: HealthState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthBook {
    scopes: BTreeMap<String, BTreeMap<Operation, Health>>,
}

impl HealthBook {
    #[must_use]
    pub fn due(&self, root: &str, operation: Operation, context: &str, now_ms: i64) -> bool {
        self.scopes
            .get(root)
            .and_then(|scope| scope.get(&operation))
            .is_none_or(|held| {
                held.context != context
                    || held.state.last_attempt_ms.is_some_and(|at| now_ms < at)
                    || held.state.next_retry_ms.is_none_or(|at| now_ms >= at)
            })
    }

    pub fn record(
        &mut self,
        root: &str,
        operation: Operation,
        context: &str,
        result: Result<(), Failure>,
        now_ms: i64,
    ) -> Option<HealthChange> {
        let held = self
            .scopes
            .entry(root.to_owned())
            .or_default()
            .entry(operation)
            .or_default();
        let previous = held.state.failure;
        if held.context != context {
            held.context = context.to_owned();
            held.state.consecutive_failures = 0;
            held.state.failed_since_ms = None;
        }
        held.state.last_attempt_ms = Some(now_ms);
        match result {
            Ok(()) => {
                held.state.last_success_ms = Some(now_ms);
                held.state.failed_since_ms = None;
                held.state.consecutive_failures = 0;
                held.state.next_retry_ms = None;
                held.state.failure = None;
                previous.map(|_| HealthChange::Recovered)
            }
            Err(failure) => {
                if previous != Some(failure.kind) {
                    held.state.consecutive_failures = 0;
                }
                held.state.failed_since_ms.get_or_insert(now_ms);
                held.state.consecutive_failures = held.state.consecutive_failures.saturating_add(1);
                let multiplier = 1_u64 << held.state.consecutive_failures.saturating_sub(1).min(16);
                let delay = RETRY_BASE_MS
                    .saturating_mul(multiplier)
                    .min(RETRY_MAX_MS)
                    .max(failure.retry_after_ms.unwrap_or(0).min(SERVER_RETRY_MAX_MS));
                held.state.next_retry_ms =
                    Some(now_ms.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX)));
                held.state.failure = Some(failure.kind);
                (previous != Some(failure.kind)).then_some(HealthChange::Failed(failure.kind))
            }
        }
    }

    pub fn retry(&mut self, root: &str, now_ms: i64) -> bool {
        let Some(scope) = self.scopes.get_mut(root) else {
            return false;
        };
        let mut changed = false;
        for held in scope
            .values_mut()
            .filter(|held| held.state.failure.is_some())
        {
            changed |= held.state.next_retry_ms != Some(now_ms);
            held.state.next_retry_ms = Some(now_ms);
        }
        changed
    }

    pub fn forget(&mut self, root: &str, operation: Operation) {
        if let Some(scope) = self.scopes.get_mut(root) {
            scope.remove(&operation);
        }
    }

    #[must_use]
    pub fn rows(&self) -> Vec<HealthRow> {
        self.scopes
            .iter()
            .flat_map(|(root, scope)| {
                scope.iter().map(|(operation, held)| HealthRow {
                    root: root.clone(),
                    operation: *operation,
                    state: held.state.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
