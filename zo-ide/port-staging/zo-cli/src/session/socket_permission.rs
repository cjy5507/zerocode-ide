//! Socket permission prompter — forward a turn's permission gate to a remote
//! `zo attach` client and await its decision (serve/attach **F2**).
//!
//! The headless server path normally installs
//! [`runtime::permission::HeadlessPermissionPrompter`] (auto-deny), so an
//! attached client could never approve an edit/bash. This module replaces it
//! for socket-driven turns: each L3 [`PermissionPrompter::decide`] call
//!
//! 1. allocates a server-global `prompt_id`,
//! 2. parks a fresh oneshot under that id in a **shared** responder map,
//! 3. emits a [`RenderBlock::PermissionPrompt`] onto the turn's render channel
//!    (so it streams to the client inline with the rest of the turn — the
//!    serializer projects it to a wire frame carrying `prompt_id` + choices),
//! 4. awaits the oneshot.
//!
//! The client renders a live modal and answers with a `permission.respond`
//! RPC on a *second* connection (the primary one is mid-stream). The server's
//! `dispatch_permission_respond` looks the responder up by `prompt_id` and
//! resolves the oneshot, unblocking the turn.
//!
//! Safety: if the client never answers (closed its modal, vanished), the
//! oneshot is dropped or the [`SocketPermissionPrompter::timeout`] elapses, and
//! the prompter resolves to a **hard deny** — a turn never hangs forever on a
//! missing human.
//!
//! This is the socket analogue of [`super::permission_bridge`] (the *local* TUI
//! bridge); it reuses that module's `map_decision` / `map_decision_forward`
//! vocabulary mapping so the two paths agree on how Allow/Deny variants lower.
//!
//! ## Living standard (mirrors `permission_bridge`)
//!
//! 1. Module layout: one file, one concern.
//! 2. Async is hand-rolled `Pin<Box<dyn Future + Send>>` (no `async-trait`).
//! 3. Every `pub` item carries a `///` doc comment.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use runtime::message_stream::{
    BlockId, PermissionChoice as RenderPermissionChoice,
    PermissionDecision as RenderPermissionDecision, PermissionPrompt, RenderBlock, ToolCallId,
};
use runtime::permission::{
    PermissionDecision as L3Decision, PermissionError, PermissionPrompter, PermissionRequest,
};
use tokio::sync::{mpsc, oneshot};

use super::permission_bridge::{map_decision, map_decision_forward};

/// Default time a parked prompt waits for a client decision before the
/// prompter hard-denies. Generous enough for a human to read and answer, short
/// enough that a vanished client cannot wedge a turn indefinitely.
pub(crate) const DEFAULT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(300);

/// Shared map of in-flight prompts: `prompt_id` → the oneshot that the server's
/// `permission.respond` handler resolves. Cloned (it is an `Arc`) between the
/// per-turn [`SocketPermissionPrompter`] and the server dispatch loop.
pub(crate) type PermissionResponders =
    Arc<Mutex<HashMap<u64, oneshot::Sender<RenderPermissionDecision>>>>;

/// Removes one parked responder when its permission future leaves scope,
/// including cancellation by dropping that future. This is the path a dead
/// serve helm uses to release a prompt immediately instead of waiting for the
/// five-minute human-response timeout.
struct ParkedResponderGuard {
    prompt_id: u64,
    responders: PermissionResponders,
}

impl Drop for ParkedResponderGuard {
    fn drop(&mut self) {
        self.responders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.prompt_id);
    }
}

/// The server-owned state a [`SocketPermissionPrompter`] needs, handed into the
/// turn runner so every turn shares one responder map and one monotonic id
/// space across the whole server.
#[derive(Clone)]
pub(crate) struct SocketPrompterConfig {
    /// Shared `prompt_id` → responder map (also read by `permission.respond`).
    pub responders: PermissionResponders,
    /// Server-global monotonic prompt-id source.
    pub next_id: Arc<AtomicU64>,
    /// How long a parked prompt waits before hard-denying.
    pub timeout: Duration,
}

impl SocketPrompterConfig {
    /// Build a config with the default timeout and fresh, empty shared state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            responders: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            timeout: DEFAULT_PERMISSION_TIMEOUT,
        }
    }
}

impl Default for SocketPrompterConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// An async [`PermissionPrompter`] that forwards each prompt over a render
/// channel to an attached client and awaits the client's socketed decision.
pub(crate) struct SocketPermissionPrompter {
    /// The turn's render channel; the prompt frame streams to the client here.
    render_tx: mpsc::Sender<RenderBlock>,
    /// Shared parked-responder map.
    responders: PermissionResponders,
    /// Server-global monotonic prompt-id source.
    next_id: Arc<AtomicU64>,
    /// Hard-deny deadline for an unanswered prompt.
    timeout: Duration,
}

impl SocketPermissionPrompter {
    /// Build a prompter bound to one turn's `render_tx`, sharing the server's
    /// responder map and id space via `config`.
    pub(crate) fn new(render_tx: mpsc::Sender<RenderBlock>, config: SocketPrompterConfig) -> Self {
        Self {
            render_tx,
            responders: config.responders,
            next_id: config.next_id,
            timeout: config.timeout,
        }
    }

    /// Build the render-block prompt for `request`, routed by `prompt_id`. The
    /// embedded responder is vestigial (the client answers over the socket); a
    /// throwaway keeps the non-`Clone` `PermissionPrompt` well-formed.
    fn build_prompt(prompt_id: u64, request: &PermissionRequest) -> RenderBlock {
        let (responder, _unused) = oneshot::channel();
        RenderBlock::PermissionPrompt(PermissionPrompt {
            id: BlockId(prompt_id),
            tool_call_id: ToolCallId(String::new()),
            tool_name: request.tool.clone(),
            reasoning: request.reasoning.clone(),
            audit_hint: Some(request.audit_hint()),
            choices: request
                .choices
                .iter()
                .map(|c| RenderPermissionChoice {
                    key: c.key,
                    label: c.label.clone(),
                    decision: map_decision_forward(c.decision),
                })
                .collect(),
            responder,
        })
    }
}

impl PermissionPrompter for SocketPermissionPrompter {
    fn decide<'a>(
        &'a self,
        request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<L3Decision, PermissionError>> + Send + 'a>> {
        Box::pin(async move {
            let prompt_id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (decision_tx, decision_rx) = oneshot::channel::<RenderPermissionDecision>();
            self.responders
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(prompt_id, decision_tx);
            let _parked = ParkedResponderGuard {
                prompt_id,
                responders: Arc::clone(&self.responders),
            };

            if self
                .render_tx
                .send(Self::build_prompt(prompt_id, &request))
                .await
                .is_err()
            {
                // Client/stream gone before the prompt could be shown.
                return Err(PermissionError::ChannelClosed);
            }

            match tokio::time::timeout(self.timeout, decision_rx).await {
                // Client answered: map the render vocabulary onto L3.
                Ok(Ok(decision)) => Ok(map_decision(decision)),
                // Responder dropped (client disconnected, server cleared it):
                // treat as a hard deny per the prompter contract.
                Ok(Err(_)) => Err(PermissionError::ResponderDropped),
                // No answer in time: hard-deny so the turn never wedges.
                Err(_elapsed) => Ok(L3Decision::Deny),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::permission::{PermissionChoice as L3Choice, RiskLevel};

    fn sample_request() -> PermissionRequest {
        PermissionRequest {
            tool: "bash".to_string(),
            input_summary: "cargo test".to_string(),
            input_hash: "abc123".to_string(),
            reasoning: "run a command".to_string(),
            choices: vec![L3Choice {
                key: 'y',
                label: "Allow".to_string(),
                decision: L3Decision::Allow,
            }],
            risk_level: RiskLevel::Medium,
        }
    }

    /// A parked prompt streams a frame carrying the routing id and resolves to
    /// the client's decision once the matching responder is fired.
    #[tokio::test]
    async fn forwards_prompt_and_resolves_clients_decision() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(4);
        let config = SocketPrompterConfig::new();
        let responders = config.responders.clone();
        let prompter = SocketPermissionPrompter::new(render_tx, config);

        let decide = tokio::spawn(async move { prompter.decide(sample_request()).await });

        // Server side: the frame arrives carrying the routing id (== block id).
        let block = render_rx.recv().await.expect("prompt frame");
        let prompt_id = match block {
            RenderBlock::PermissionPrompt(p) => {
                assert_eq!(p.tool_name, "bash");
                assert_eq!(p.choices.len(), 1);
                assert!(p
                    .audit_hint
                    .as_deref()
                    .is_some_and(|hint| hint.contains("[y] Allow")));
                p.id.0
            }
            other => panic!("unexpected block: {other:?}"),
        };

        // `permission.respond` arrives on a second connection: resolve the map.
        let tx = responders
            .lock()
            .unwrap()
            .remove(&prompt_id)
            .expect("responder parked");
        tx.send(RenderPermissionDecision::AllowAlways)
            .expect("send");

        let decision = decide.await.expect("join").expect("decision");
        assert_eq!(decision, L3Decision::Allow);
        // Map is empty again — the prompt unparked itself.
        assert!(responders.lock().unwrap().is_empty());
    }

    /// A dropped responder (client vanished) is a hard deny, and the prompt is
    /// removed from the shared map.
    #[tokio::test]
    async fn dropped_responder_is_a_hard_deny() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(4);
        let config = SocketPrompterConfig::new();
        let responders = config.responders.clone();
        let prompter = SocketPermissionPrompter::new(render_tx, config);

        let decide = tokio::spawn(async move { prompter.decide(sample_request()).await });
        let block = render_rx.recv().await.expect("prompt frame");
        let prompt_id = match block {
            RenderBlock::PermissionPrompt(p) => p.id.0,
            other => panic!("unexpected block: {other:?}"),
        };
        // Drop the responder without answering.
        drop(
            responders
                .lock()
                .unwrap()
                .remove(&prompt_id)
                .expect("parked"),
        );

        let result = decide.await.expect("join");
        assert!(matches!(result, Err(PermissionError::ResponderDropped)));
    }

    /// Dropping the in-flight permission future is the serve host-abort path.
    /// Its parked responder must disappear immediately rather than surviving
    /// until the human-response timeout.
    #[tokio::test]
    async fn dropping_permission_future_unparks_responder() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(4);
        let config = SocketPrompterConfig::new();
        let responders = config.responders.clone();
        let prompter = SocketPermissionPrompter::new(render_tx, config);

        let decide = tokio::spawn(async move { prompter.decide(sample_request()).await });
        let _ = render_rx.recv().await.expect("prompt frame");
        assert_eq!(responders.lock().unwrap().len(), 1);

        decide.abort();
        assert!(decide.await.is_err());
        assert!(responders.lock().unwrap().is_empty());
    }

    /// A timed-out prompt hard-denies rather than hanging the turn. Uses a tiny
    /// real timeout (no paused clock, so no `test-util` dependency).
    #[tokio::test]
    async fn timeout_hard_denies() {
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(4);
        let mut config = SocketPrompterConfig::new();
        config.timeout = Duration::from_millis(20);
        let responders = config.responders.clone();
        let prompter = SocketPermissionPrompter::new(render_tx, config);

        let decide = tokio::spawn(async move { prompter.decide(sample_request()).await });
        // Consume the frame but never respond: the prompter's deadline elapses.
        let _ = render_rx.recv().await.expect("prompt frame");

        let decision = decide.await.expect("join").expect("decision");
        assert_eq!(decision, L3Decision::Deny);
        assert!(responders.lock().unwrap().is_empty());
    }

    /// A closed render channel (client gone before the prompt shows) is a hard
    /// deny via `ChannelClosed`.
    #[tokio::test]
    async fn closed_render_channel_denies() {
        let (render_tx, render_rx) = mpsc::channel::<RenderBlock>(1);
        drop(render_rx);
        let config = SocketPrompterConfig::new();
        let responders = config.responders.clone();
        let prompter = SocketPermissionPrompter::new(render_tx, config);

        let result = prompter.decide(sample_request()).await;
        assert!(matches!(result, Err(PermissionError::ChannelClosed)));
        assert!(responders.lock().unwrap().is_empty());
    }
}
