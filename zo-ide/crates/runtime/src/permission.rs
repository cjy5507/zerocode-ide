//! Async permission prompter seam (Lane L3 — living standard).
//!
//! This module defines the **async** [`PermissionPrompter`] trait used by
//! the agent loop when it needs a human (or policy-driven) decision on a
//! tool invocation. It is deliberately separate from
//! `crate::permissions`, which houses the *policy engine* and its
//! legacy synchronous prompter trait used by the blocking `zo-cli`
//! implementation. L7 will migrate the production CLI prompter onto this
//! async trait; until then both traits coexist.
//!
//! ## Design choices (living standard for Phase 3 L2–L7)
//!
//! * **Module layout** — `permission.rs` single file, one concern: the
//!   async decision seam and its in-tree channel bridge. No sub-modules.
//! * **Errors** — one `thiserror`-derived enum per module
//!   ([`PermissionError`]). No `anyhow` leaks into public API.
//! * **Async trait** — hand-rolled
//!   `fn decide(..) -> Pin<Box<dyn Future<Output = ..> + Send + '_>>`,
//!   matching `message_stream::provider::ProviderStream`. We do **not**
//!   pull in the `async-trait` crate.
//! * **Provider neutral** (code-rules R1) — [`PermissionRequest`] uses
//!   tool-agnostic field names (`tool`, `reasoning`, `choices`,
//!   `risk_level`); no Anthropic-specific fields leak across the seam.
//! * **Backpressure honest** — [`ChannelPrompter`] bridges the async
//!   trait onto a bounded `mpsc` channel of
//!   `(PermissionRequest, OneshotResponder)` pairs; the UI (or test
//!   harness) consumes the channel and resolves each responder.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::{mpsc, oneshot};

// ============================================================================
// Provider-neutral request / decision types
// ============================================================================

/// Relative danger classification used to style the prompt UI.
///
/// Provider-neutral: every adapter maps its native concept (e.g. an
/// Anthropic tool descriptor) onto one of these buckets before calling
/// [`PermissionPrompter::decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Read-only or idempotent (e.g. `ls`, `cat`).
    Low,
    /// Local writes or limited side effects (e.g. editing a file).
    Medium,
    /// Destructive, network-bound, or otherwise irreversible.
    High,
}

impl RiskLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// A single choice the user may pick in response to a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionChoice {
    /// Single keyboard key (e.g. `'y'`, `'n'`, `'a'`).
    pub key: char,
    /// Human-readable label (e.g. `"Allow once"`).
    pub label: String,
    /// The decision this choice resolves to.
    pub decision: PermissionDecision,
}

/// Neutral per-call permission request.
///
/// Intentionally **does not** borrow Anthropic tool names or schemas.
/// Adapters lower their native request into this shape before handing it
/// to the prompter. (code-rules R1)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    /// Logical tool name (e.g. `"bash"`, `"write_file"`).
    pub tool: String,
    /// Short display-safe command or path summary. Never the full tool payload.
    pub input_summary: String,
    /// SHA-256 of the full tool input for audit correlation without disclosure.
    pub input_hash: String,
    /// Human-readable rationale: what the agent wants to do and why.
    pub reasoning: String,
    /// Legal user choices (in UI display order).
    pub choices: Vec<PermissionChoice>,
    /// Relative risk classification.
    pub risk_level: RiskLevel,
}

impl PermissionRequest {
    #[must_use]
    pub fn audit_hint(&self) -> String {
        let allow_choices = self
            .choices
            .iter()
            .filter(|choice| {
                matches!(
                    choice.decision,
                    PermissionDecision::Allow | PermissionDecision::AllowOnce
                )
            })
            .map(|choice| format!("[{}] {}", choice.key, choice.label))
            .collect::<Vec<_>>();

        if allow_choices.is_empty() {
            format!(
                "risk: {}; no allow choice is available; change permission policy or rerun with a less restrictive mode",
                self.risk_level.as_str()
            )
        } else {
            format!(
                "risk: {}; explicitly unblock with {}",
                self.risk_level.as_str(),
                allow_choices.join(" or ")
            )
        }
    }
}

/// User-facing decision returned by a [`PermissionPrompter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow this call and remember the approval for the session.
    Allow,
    /// Allow this single call only.
    AllowOnce,
    /// Reject this call.
    Deny,
}

// ============================================================================
// Errors
// ============================================================================

/// Error surfaced by [`PermissionPrompter::decide`] implementations.
///
/// Follows the Phase 3 living-standard error pattern: one
/// `thiserror` enum per module, no `anyhow`.
#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    /// The downstream UI / consumer dropped the request channel before
    /// answering. Agent loop should treat this as a hard deny.
    #[error("permission channel closed before decision")]
    ChannelClosed,

    /// The user-facing layer dropped the one-shot responder without
    /// sending a decision.
    #[error("permission responder dropped without decision")]
    ResponderDropped,

    /// Adapter-specific failure wrapped for propagation.
    #[error("{source_name}: {message}")]
    Adapter {
        /// Short identifier of the failing adapter.
        source_name: &'static str,
        /// Human-readable message.
        message: String,
    },
}

// ============================================================================
// Async prompter trait
// ============================================================================

/// Async permission decision seam.
///
/// Implementors receive a neutral [`PermissionRequest`] and return a
/// [`PermissionDecision`] (or [`PermissionError`]) asynchronously. The
/// hand-rolled `Pin<Box<dyn Future + Send>>` return matches the
/// `message_stream::provider::ProviderStream` living standard — no
/// `async-trait` crate.
pub trait PermissionPrompter: Send + Sync {
    /// Decide a single permission request.
    fn decide<'a>(
        &'a self,
        request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PermissionDecision, PermissionError>> + Send + 'a>>;
}

// ============================================================================
// OneshotResponder
// ============================================================================

/// Thin newtype around a `oneshot::Sender<PermissionDecision>`.
///
/// The agent loop (via [`ChannelPrompter`]) constructs one of these per
/// request and hands it to the UI. The UI calls [`OneshotResponder::respond`]
/// exactly once. Dropping the responder without responding surfaces as
/// [`PermissionError::ResponderDropped`] on the waiting side.
#[derive(Debug)]
pub struct OneshotResponder {
    tx: oneshot::Sender<PermissionDecision>,
}

impl OneshotResponder {
    /// Wrap a raw `oneshot` sender.
    #[must_use]
    pub fn new(tx: oneshot::Sender<PermissionDecision>) -> Self {
        Self { tx }
    }

    /// Send the user's decision back to the waiting prompter.
    ///
    /// Consumes `self`: each responder may answer at most once. Returns
    /// the decision back to the caller if the receiver has already been
    /// dropped, so callers can log / recover without panicking.
    pub fn respond(self, decision: PermissionDecision) -> Result<(), PermissionDecision> {
        self.tx.send(decision)
    }
}

// ============================================================================
// ChannelPrompter — mpsc-bridged implementation
// ============================================================================

/// [`PermissionPrompter`] implementation that forwards each request over
/// a bounded `mpsc` to a downstream consumer (typically the TUI event
/// loop) and awaits the one-shot reply.
///
/// Cloning the prompter is cheap: it only clones the underlying
/// `mpsc::Sender`, which is itself cheap and preserves backpressure.
#[derive(Debug, Clone)]
pub struct ChannelPrompter {
    request_tx: mpsc::Sender<(PermissionRequest, OneshotResponder)>,
}

impl ChannelPrompter {
    /// Build a paired `(prompter, receiver)`.
    ///
    /// `capacity` bounds how many outstanding prompts may queue before
    /// the agent loop blocks — keep it small (1–4) per code-rules R8.
    #[must_use]
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<(PermissionRequest, OneshotResponder)>) {
        let (request_tx, request_rx) = mpsc::channel(capacity);
        (Self { request_tx }, request_rx)
    }

    /// Build a prompter that forwards into an existing sender. Useful
    /// when the receiver is already owned by a long-lived task.
    #[must_use]
    pub fn from_sender(request_tx: mpsc::Sender<(PermissionRequest, OneshotResponder)>) -> Self {
        Self { request_tx }
    }
}

impl PermissionPrompter for ChannelPrompter {
    fn decide<'a>(
        &'a self,
        request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            let responder = OneshotResponder::new(tx);
            self.request_tx
                .send((request, responder))
                .await
                .map_err(|_| PermissionError::ChannelClosed)?;
            rx.await.map_err(|_| PermissionError::ResponderDropped)
        })
    }
}

/// How a [`HeadlessPermissionPrompter`] resolves the prompts the policy could
/// not decide on its own (i.e. the residual "ask a human" cases).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessDecision {
    /// Approve each residual prompt for that single call (no durable rule).
    /// Use for trusted CI / `--yes`-style unattended runs.
    AutoApprove,
    /// Deny each residual prompt — the safe default for unattended runs where
    /// no one can vet the action.
    DenyAll,
}

/// Async [`PermissionPrompter`] for non-interactive runs (cron, remote, CI,
/// `--output json|ndjson`). With no human to ask, it resolves prompts from a
/// fixed [`HeadlessDecision`] without ever blocking on I/O — so a turn can be
/// driven through the streaming path (`run_turn_streaming_with_images`) and
/// emit live `RenderBlock`s headlessly instead of a one-shot document.
///
/// Deny/allow/ask *rules* are still applied by the policy first; this prompter
/// only fires for the residual cases that would otherwise prompt a human.
#[derive(Debug, Clone, Copy)]
pub struct HeadlessPermissionPrompter {
    decision: HeadlessDecision,
}

impl HeadlessPermissionPrompter {
    /// Construct a headless prompter with the given residual-decision policy.
    #[must_use]
    pub fn new(decision: HeadlessDecision) -> Self {
        Self { decision }
    }
}

impl PermissionPrompter for HeadlessPermissionPrompter {
    fn decide<'a>(
        &'a self,
        _request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PermissionDecision, PermissionError>> + Send + 'a>>
    {
        let decision = match self.decision {
            HeadlessDecision::AutoApprove => PermissionDecision::AllowOnce,
            HeadlessDecision::DenyAll => PermissionDecision::Deny,
        };
        Box::pin(async move { Ok(decision) })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HeadlessDecision, HeadlessPermissionPrompter, PermissionChoice, PermissionDecision,
        PermissionPrompter, PermissionRequest, RiskLevel,
    };

    fn request() -> PermissionRequest {
        PermissionRequest {
            tool: "bash".to_string(),
            input_summary: "cargo test".to_string(),
            input_hash: "abc123".to_string(),
            reasoning: "run a command".to_string(),
            choices: Vec::new(),
            risk_level: RiskLevel::Medium,
        }
    }

    #[tokio::test]
    async fn headless_auto_approve_allows_once() {
        let prompter = HeadlessPermissionPrompter::new(HeadlessDecision::AutoApprove);
        let decision = prompter.decide(request()).await.expect("decision");
        assert_eq!(decision, PermissionDecision::AllowOnce);
    }

    #[tokio::test]
    async fn headless_deny_all_denies() {
        let prompter = HeadlessPermissionPrompter::new(HeadlessDecision::DenyAll);
        let decision = prompter.decide(request()).await.expect("decision");
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[test]
    fn audit_hint_names_risk_and_explicit_allow_choices() {
        let request = PermissionRequest {
            tool: "bash".to_string(),
            input_summary: "cargo test".to_string(),
            input_hash: "abc123".to_string(),
            reasoning: "requires approval".to_string(),
            choices: vec![
                PermissionChoice {
                    key: 'y',
                    label: "Allow".to_string(),
                    decision: PermissionDecision::Allow,
                },
                PermissionChoice {
                    key: 'o',
                    label: "Allow once".to_string(),
                    decision: PermissionDecision::AllowOnce,
                },
                PermissionChoice {
                    key: 'n',
                    label: "Deny".to_string(),
                    decision: PermissionDecision::Deny,
                },
            ],
            risk_level: RiskLevel::High,
        };
        let hint = request.audit_hint();
        assert!(hint.contains("risk: high"));
        assert!(hint.contains("[y] Allow"));
        assert!(hint.contains("[o] Allow once"));
        assert!(!hint.contains("[n] Deny"));
    }
}
