//! Team-inbox digest injection and delivery settlement for
//! [`ConversationRuntime`], split out of `mod.rs` so the turn loops there read
//! as orchestration. Behaviour-preserving: these were `ConversationRuntime`
//! methods and module-level helpers, now `pub(super)` where the loops in
//! `mod.rs` still call them.

use serde_json::{json, Map, Value};

use crate::team_inbox_digest::{
    ack_team_inbox_turn, fail_team_inbox_turn, load_team_inbox_turn_digest,
    mark_team_inbox_injected, TeamInboxDeliveryBatch, TeamInboxDigestConfig,
    DEFAULT_MAX_DELIVERY_RETRIES, TEAM_INBOX_REMINDER_PREFIX,
};

use super::{trace_attrs, ApiClient, ConversationRuntime, ToolExecutor, TurnSummary};

/// Maximum length of a bounded error `reason` recorded on a `TeamInbox`
/// diagnostics event. Keeps trace attributes small and prevents an unbounded
/// backend error string from bloating the trace.
const TEAM_INBOX_TRACE_REASON_MAX_CHARS: usize = 200;

/// Build the shared safe-metadata attribute set for a `TeamInbox` diagnostics
/// event. Carries only `action`/`status`/`consumer_id` — never raw body,
/// summary, or reminder text.
fn team_inbox_digest_attrs(action: &str, status: &str, consumer_id: &str) -> Map<String, Value> {
    trace_attrs(json!({
        "action": action,
        "status": status,
        "consumer_id": consumer_id,
    }))
}

/// Truncate a backend error string to a bounded, trace-safe `reason`.
fn bounded_team_inbox_reason(reason: &str) -> String {
    let trimmed = reason.trim();
    if trimmed.chars().count() <= TEAM_INBOX_TRACE_REASON_MAX_CHARS {
        return trimmed.to_string();
    }
    let mut truncated = trimmed
        .chars()
        .take(TEAM_INBOX_TRACE_REASON_MAX_CHARS)
        .collect::<String>();
    truncated.push('…');
    truncated
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    fn team_inbox_cwd(&self) -> Option<std::path::PathBuf> {
        match &self.workspace_cwd {
            Some(cwd) => Some(cwd.clone()),
            None => std::env::current_dir().ok(),
        }
    }

    fn team_inbox_turn_id(&self) -> String {
        format!(
            "{}:{}",
            self.session.session_id,
            self.session.messages.len()
        )
    }
    pub(super) fn inject_team_inbox_digest_reminder(&mut self) {
        // Settle (ack) any batch still pending from a previous leg: a
        // Stop-loop (TurnEnd followup) leg only reaches the next injection
        // after completing successfully (error legs break to the outer
        // turn-boundary settle first), so its digest was already consumed by
        // the model — ack it instead of orphaning the rows in `injected` and
        // re-delivering the same updates mid-turn. The only other way a batch
        // survives to here is a dropped/cancelled turn future, where the
        // request carrying the digest was already sent; ack matches that
        // delivered-context reality too.
        self.ack_team_inbox_turn();
        if !self.team_inbox_digest_enabled || self.team_inbox_digest_max_updates == 0 {
            return;
        }
        let Some(cwd) = self.team_inbox_cwd() else {
            return;
        };
        let mut config = TeamInboxDigestConfig::for_session(&cwd, &self.session.session_id);
        config.max_updates = self.team_inbox_digest_max_updates;
        let digest = match load_team_inbox_turn_digest(&config) {
            Ok(Some(digest)) => digest,
            Ok(None) => {
                self.record_team_inbox_digest("load", "absent", &config.consumer_id, None);
                return;
            }
            Err(error) => {
                self.record_team_inbox_digest(
                    "load",
                    "failure",
                    &config.consumer_id,
                    Some(&error),
                );
                return;
            }
        };
        let update_count = digest.delivery_count();
        self.record_team_inbox_digest_loaded("load", "loaded", &config.consumer_id, update_count);
        self.replace_transient_system_reminder_by_prefix(
            TEAM_INBOX_REMINDER_PREFIX,
            Some(&digest.reminder),
        );
        let turn_id = self.team_inbox_turn_id();
        match mark_team_inbox_injected(&config, &turn_id, &digest) {
            Ok(batch) => {
                self.record_team_inbox_digest_loaded(
                    "mark_injected",
                    "success",
                    &config.consumer_id,
                    update_count,
                );
                self.team_inbox_turn = Some(batch);
            }
            Err(error) => {
                self.record_team_inbox_digest(
                    "mark_injected",
                    "failure",
                    &config.consumer_id,
                    Some(&error),
                );
            }
        }
    }

    pub(super) fn ack_team_inbox_turn(&mut self) {
        if let Some(batch) = self.team_inbox_turn.take() {
            let outcome = ack_team_inbox_turn(&batch);
            self.record_team_inbox_settle("ack", &batch, outcome.as_ref().err());
        }
    }

    pub(super) fn fail_team_inbox_turn(&mut self) {
        if let Some(batch) = self.team_inbox_turn.take() {
            let outcome = fail_team_inbox_turn(&batch, DEFAULT_MAX_DELIVERY_RETRIES);
            self.record_team_inbox_settle("fail", &batch, outcome.as_ref().err());
        }
    }

    pub(super) fn settle_team_inbox_turn_for_result<E>(&mut self, result: &Result<TurnSummary, E>) {
        if result.is_ok() {
            self.ack_team_inbox_turn();
        } else {
            self.fail_team_inbox_turn();
        }
    }

    /// Record a `team_inbox_digest` diagnostics event with safe metadata only.
    /// Best-effort observation: never alters read/write/ack/cursor semantics.
    /// Attributes never carry raw body, summary, or reminder text.
    fn record_team_inbox_digest(
        &self,
        action: &str,
        status: &str,
        consumer_id: &str,
        reason: Option<&str>,
    ) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        let mut attributes = team_inbox_digest_attrs(action, status, consumer_id);
        if let Some(reason) = reason {
            attributes.insert(
                "reason".to_string(),
                Value::String(bounded_team_inbox_reason(reason)),
            );
        }
        session_tracer.record("team_inbox_digest", attributes);
    }

    /// Same as [`Self::record_team_inbox_digest`] but for the success paths that
    /// know a delivery/update count to attach.
    fn record_team_inbox_digest_loaded(
        &self,
        action: &str,
        status: &str,
        consumer_id: &str,
        update_count: usize,
    ) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        let mut attributes = team_inbox_digest_attrs(action, status, consumer_id);
        attributes.insert("update_count".to_string(), Value::from(update_count));
        session_tracer.record("team_inbox_digest", attributes);
    }

    /// Record a `team_inbox_delivery_settle` diagnostics event for the terminal
    /// ack/fail write. Best-effort observation with safe metadata only; the
    /// delivery/cursor semantics are already decided before this runs.
    fn record_team_inbox_settle(
        &self,
        action: &str,
        batch: &TeamInboxDeliveryBatch,
        error: Option<&String>,
    ) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };
        let status = if error.is_none() { "success" } else { "failure" };
        let mut attributes = team_inbox_digest_attrs(action, status, batch.consumer_id());
        attributes.insert(
            "update_count".to_string(),
            Value::from(batch.delivery_count()),
        );
        attributes.insert(
            "turn_id".to_string(),
            Value::String(batch.turn_id().to_string()),
        );
        if let Some(error) = error {
            attributes.insert(
                "reason".to_string(),
                Value::String(bounded_team_inbox_reason(error)),
            );
        }
        session_tracer.record("team_inbox_delivery_settle", attributes);
    }
}
