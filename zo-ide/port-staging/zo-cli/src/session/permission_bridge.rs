//! Permission prompt bridge — L3 `ChannelPrompter` ↔ TUI `RenderBlock::PermissionPrompt`.
//!
//! The agent loop uses L3's async [`PermissionPrompter`] trait to ask
//! for permission. The canonical implementation
//! [`runtime::permission::ChannelPrompter`] pushes
//! `(PermissionRequest, OneshotResponder)` pairs onto a bounded mpsc
//! channel for a downstream consumer to drain. The TUI, however,
//! renders permission prompts as [`RenderBlock::PermissionPrompt`]
//! values on the main `RenderBlock` channel (so the modal shares the
//! same event loop as the rest of the transcript).
//!
//! This module owns the translation task that sits between those two
//! shapes:
//!
//! 1. Drain `(PermissionRequest, OneshotResponder)` pairs from the
//!    `ChannelPrompter` receiver.
//! 2. For each pair, allocate a fresh [`oneshot`] channel, build a
//!    [`RenderBlock::PermissionPrompt`] that carries the sender, and
//!    forward it onto the render channel.
//! 3. Await the decision on the fresh oneshot.
//! 4. Map the [`runtime::message_stream::PermissionDecision`] value
//!    the TUI returns back onto the L3
//!    [`runtime::permission::PermissionDecision`] shape and resolve
//!    the original [`runtime::permission::OneshotResponder`].
//!
//! The bridge preserves every error surface the async path cares
//! about: if either channel closes or the user drops the modal
//! without answering, the L3 responder is dropped without a value,
//! which the runtime loop interprets as [`PermissionError::ResponderDropped`]
//! (hard deny).
//!
//! ## Living standard (mirrors L1)
//!
//! 1. Module layout: one file, one concern.
//! 2. Errors: one `thiserror` enum ([`PermissionBridgeError`]). No
//!    `anyhow`.
//! 3. Async is hand-rolled `async fn` (no `async-trait`).
//! 4. Tests live at
//!    `crates/zo-cli/tests/session_integration.rs` alongside
//!    the rest of the L7c integration surface.
//! 5. Every `pub` item carries a `///` doc comment.
//!
//! Dead-code allowed at the module level: the bridge is scaffolding
//! for the follow-up lane that wires `run_repl` into the TUI driver;
//! it is exercised by the module's own unit tests but has no in-tree
//! production call site yet.

use runtime::message_stream::{
    BlockId, BlockIdGen, PermissionChoice as RenderPermissionChoice,
    PermissionDecision as RenderPermissionDecision, PermissionPrompt, RenderBlock,
};
use runtime::permission::{
    OneshotResponder, PermissionChoice as L3PermissionChoice, PermissionDecision as L3Decision,
    PermissionRequest,
};
use tokio::sync::{mpsc, oneshot};

use crate::remote_control::{
    RemoteShared, ToolApprovalAttempt, ToolApprovalChoice, ToolApprovalDecision,
    ToolApprovalRequest, ToolApprovalSource,
};

/// Errors the bridge pump can surface to its supervisor.
#[derive(Debug, thiserror::Error)]
pub enum PermissionBridgeError {
    /// The render channel into the TUI closed before the bridge could
    /// deliver a prompt. Typically means the TUI task exited.
    #[error("render channel closed before permission prompt could be delivered")]
    RenderChannelClosed,
}

/// Run the translation pump until `request_rx` closes.
///
/// Each inbound `(PermissionRequest, OneshotResponder)` pair is
/// translated into a [`RenderBlock::PermissionPrompt`] and forwarded
/// onto `render_tx`. The pump then awaits the TUI's decision on a
/// fresh oneshot and resolves the original L3 responder.
///
/// `ids` is the shared [`BlockIdGen`] used by the rest of the turn so
/// permission prompts get contiguous block ids alongside text/tool
/// blocks.
///
/// Returns `Ok(())` on a clean shutdown (the `ChannelPrompter` receiver
/// closed), or [`PermissionBridgeError::RenderChannelClosed`] if the
/// TUI hung up while a prompt was still in flight.
#[cfg(test)]
pub async fn run_permission_pump(
    request_rx: mpsc::Receiver<(PermissionRequest, OneshotResponder)>,
    render_tx: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
) -> Result<(), PermissionBridgeError> {
    run_permission_pump_with_remote(request_rx, render_tx, ids, None, None).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RemotePermissionResolution {
    pub(crate) block_id: BlockId,
    pub(crate) decision: RenderPermissionDecision,
}

pub(crate) async fn run_permission_pump_with_remote(
    mut request_rx: mpsc::Receiver<(PermissionRequest, OneshotResponder)>,
    render_tx: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
    remote: Option<RemoteShared>,
    remote_resolution_tx: Option<mpsc::UnboundedSender<RemotePermissionResolution>>,
) -> Result<(), PermissionBridgeError> {
    while let Some((request, l3_responder)) = request_rx.recv().await {
        let (modal_tx, modal_rx) = oneshot::channel::<RenderPermissionDecision>();
        let block_id = ids.next();
        let prompt = build_render_prompt(&request, modal_tx, block_id);
        if render_tx
            .send(RenderBlock::PermissionPrompt(prompt))
            .await
            .is_err()
        {
            // TUI is gone — drop `l3_responder` (causes hard deny on
            // the agent side) and return the error upstream.
            drop(l3_responder);
            return Err(PermissionBridgeError::RenderChannelClosed);
        }

        let render_decision = match remote.as_ref() {
            Some(remote) => {
                let (remote_tx, remote_rx) = oneshot::channel();
                let request_id = remote.publish_tool_approval(
                    remote_approval_request(&request),
                    remote_tx,
                );
                await_first_permission_decision(
                    modal_rx,
                    remote_rx,
                    remote,
                    &request_id,
                    block_id,
                    remote_resolution_tx.as_ref(),
                )
                .await
            }
            None => modal_rx.await.ok(),
        };

        match render_decision {
            Some(render_decision) => {
                let l3_decision = map_decision(render_decision);
                // Best-effort: if the agent loop has already given up
                // waiting, the send returns the decision back to us
                // — nothing to do.
                let _ = l3_responder.respond(l3_decision);
            }
            None => {
                // TUI closed the modal without answering — drop the
                // L3 responder so the runtime treats it as a hard
                // deny, per the `ChannelPrompter` contract.
                drop(l3_responder);
            }
        }
    }
    Ok(())
}

fn remote_approval_request(request: &PermissionRequest) -> ToolApprovalRequest {
    ToolApprovalRequest {
        request_id: String::new(),
        tool_name: request.tool.clone(),
        input_summary: request.input_summary.clone(),
        input_hash: request.input_hash.clone(),
        choices: request
            .choices
            .iter()
            .map(|choice| ToolApprovalChoice {
                label: choice.label.clone(),
                decision: tool_approval_decision(map_decision_forward(choice.decision)),
            })
            .collect(),
    }
}

async fn await_first_permission_decision(
    modal_rx: oneshot::Receiver<RenderPermissionDecision>,
    mut remote_rx: oneshot::Receiver<ToolApprovalDecision>,
    remote: &RemoteShared,
    request_id: &str,
    block_id: BlockId,
    resolution_tx: Option<&mpsc::UnboundedSender<RemotePermissionResolution>>,
) -> Option<RenderPermissionDecision> {
    tokio::pin!(modal_rx);
    tokio::select! {
        biased;
        local = &mut modal_rx => resolve_local_permission(
            local.ok(),
            remote,
            request_id,
            block_id,
            resolution_tx,
        ),
        remote_decision = &mut remote_rx => match remote_decision {
            Ok(remote_decision) => {
                let decision = render_approval_decision(remote_decision);
                notify_remote_resolution(resolution_tx, block_id, decision);
                Some(decision)
            }
            Err(_) => {
                // A disconnected remote must never deny or time out the local
                // prompt. Continue waiting on the unchanged TUI responder.
                modal_rx.await.ok()
            }
        }
    }
}

fn resolve_local_permission(
    local: Option<RenderPermissionDecision>,
    remote: &RemoteShared,
    request_id: &str,
    block_id: BlockId,
    resolution_tx: Option<&mpsc::UnboundedSender<RemotePermissionResolution>>,
) -> Option<RenderPermissionDecision> {
    let Some(local) = local else {
        let _ = remote.resolve_tool_approval(
            request_id,
            ToolApprovalDecision::Deny,
            ToolApprovalSource::Tui,
        );
        return None;
    };
    match remote.resolve_tool_approval(
        request_id,
        tool_approval_decision(local),
        ToolApprovalSource::Tui,
    ) {
        ToolApprovalAttempt::Resolved => Some(local),
        ToolApprovalAttempt::AlreadyResolved(resolution) => {
            let decision = render_approval_decision(resolution.decision);
            if resolution.source == ToolApprovalSource::Remote {
                notify_remote_resolution(resolution_tx, block_id, decision);
            }
            Some(decision)
        }
        ToolApprovalAttempt::InvalidChoice | ToolApprovalAttempt::Unknown => None,
    }
}

fn notify_remote_resolution(
    sender: Option<&mpsc::UnboundedSender<RemotePermissionResolution>>,
    block_id: BlockId,
    decision: RenderPermissionDecision,
) {
    if let Some(sender) = sender {
        let _ = sender.send(RemotePermissionResolution { block_id, decision });
    }
}

fn tool_approval_decision(decision: RenderPermissionDecision) -> ToolApprovalDecision {
    match decision {
        RenderPermissionDecision::AllowOnce => ToolApprovalDecision::AllowOnce,
        RenderPermissionDecision::AllowAlways => ToolApprovalDecision::AllowAlways,
        RenderPermissionDecision::Deny => ToolApprovalDecision::Deny,
        RenderPermissionDecision::DenyAlways => ToolApprovalDecision::DenyAlways,
    }
}

fn render_approval_decision(decision: ToolApprovalDecision) -> RenderPermissionDecision {
    match decision {
        ToolApprovalDecision::AllowOnce => RenderPermissionDecision::AllowOnce,
        ToolApprovalDecision::AllowAlways => RenderPermissionDecision::AllowAlways,
        ToolApprovalDecision::Deny => RenderPermissionDecision::Deny,
        ToolApprovalDecision::DenyAlways => RenderPermissionDecision::DenyAlways,
    }
}

/// Translate an L3 [`PermissionRequest`] into the TUI-facing
/// [`PermissionPrompt`] payload.
///
/// The resulting prompt carries `modal_tx` as its responder so the
/// TUI modal can resolve the request by calling `send(..)` exactly
/// once.
#[must_use]
pub fn build_render_prompt(
    request: &PermissionRequest,
    modal_tx: oneshot::Sender<RenderPermissionDecision>,
    id: BlockId,
) -> PermissionPrompt {
    let choices = request.choices.iter().map(map_choice).collect::<Vec<_>>();
    PermissionPrompt {
        id,
        tool_call_id: runtime::message_stream::ToolCallId(String::new()),
        tool_name: request.tool.clone(),
        reasoning: request.reasoning.clone(),
        audit_hint: Some(request.audit_hint()),
        choices,
        responder: modal_tx,
    }
}

/// Translate an L3 [`L3PermissionChoice`] into the render-block
/// [`RenderPermissionChoice`] used by the TUI modal widget.
fn map_choice(choice: &L3PermissionChoice) -> RenderPermissionChoice {
    RenderPermissionChoice {
        key: choice.key,
        label: choice.label.clone(),
        decision: map_decision_forward(choice.decision),
    }
}

/// Map the TUI's returned decision back onto the L3 shape.
///
/// The render-block vocabulary distinguishes "remember this session"
/// forms (`AllowAlways` / `DenyAlways`); the L3 vocabulary collapses
/// those onto [`L3Decision::Allow`] and [`L3Decision::Deny`]
/// respectively. `AllowOnce` / `Deny` map straight through.
#[must_use]
pub fn map_decision(decision: RenderPermissionDecision) -> L3Decision {
    match decision {
        RenderPermissionDecision::AllowOnce => L3Decision::AllowOnce,
        RenderPermissionDecision::AllowAlways => L3Decision::Allow,
        RenderPermissionDecision::Deny | RenderPermissionDecision::DenyAlways => L3Decision::Deny,
    }
}

/// Forward mapping — L3 decision → render-block decision — used when
/// lowering the initial [`PermissionRequest::choices`] into the TUI
/// prompt. The L3 enum is narrower so the mapping is lossless in this
/// direction.
#[must_use]
pub fn map_decision_forward(decision: L3Decision) -> RenderPermissionDecision {
    match decision {
        L3Decision::Allow => RenderPermissionDecision::AllowAlways,
        L3Decision::AllowOnce => RenderPermissionDecision::AllowOnce,
        L3Decision::Deny => RenderPermissionDecision::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::permission::{ChannelPrompter, PermissionPrompter, RiskLevel};

    fn remote_shared() -> RemoteShared {
        let (prompt_effects, _) = mpsc::channel(4);
        let (control_effects, _) = mpsc::channel(4);
        let (notices, _) = mpsc::channel(4);
        RemoteShared::new(
            "session".to_string(),
            "zo".to_string(),
            prompt_effects,
            control_effects,
            notices,
        )
    }

    async fn next_remote_approval(
        events: &mut tokio::sync::broadcast::Receiver<crate::remote_control::ServerMessage>,
    ) -> ToolApprovalRequest {
        loop {
            if let crate::remote_control::ServerMessage::ToolApprovalRequest {
                approval,
            } = events.recv().await.expect("remote approval event")
            {
                return approval;
            }
        }
    }

    fn sample_request() -> PermissionRequest {
        PermissionRequest {
            tool: "bash".to_string(),
            input_summary: "echo safe".to_string(),
            input_hash: "abc123".to_string(),
            reasoning: "run a harmless command".to_string(),
            choices: vec![
                L3PermissionChoice {
                    key: 'y',
                    label: "Allow once".to_string(),
                    decision: L3Decision::AllowOnce,
                },
                L3PermissionChoice {
                    key: 'a',
                    label: "Allow for session".to_string(),
                    decision: L3Decision::Allow,
                },
                L3PermissionChoice {
                    key: 'n',
                    label: "Deny".to_string(),
                    decision: L3Decision::Deny,
                },
            ],
            risk_level: RiskLevel::Low,
        }
    }

    #[test]
    fn map_decision_collapses_always_onto_allow_and_deny() {
        assert_eq!(
            map_decision(RenderPermissionDecision::AllowOnce),
            L3Decision::AllowOnce
        );
        assert_eq!(
            map_decision(RenderPermissionDecision::AllowAlways),
            L3Decision::Allow
        );
        assert_eq!(
            map_decision(RenderPermissionDecision::Deny),
            L3Decision::Deny
        );
        assert_eq!(
            map_decision(RenderPermissionDecision::DenyAlways),
            L3Decision::Deny
        );
    }

    #[test]
    fn map_decision_forward_is_lossless_for_l3_vocabulary() {
        assert_eq!(
            map_decision_forward(L3Decision::Allow),
            RenderPermissionDecision::AllowAlways
        );
        assert_eq!(
            map_decision_forward(L3Decision::AllowOnce),
            RenderPermissionDecision::AllowOnce
        );
        assert_eq!(
            map_decision_forward(L3Decision::Deny),
            RenderPermissionDecision::Deny
        );
    }

    #[test]
    fn build_render_prompt_preserves_tool_name_and_reasoning() {
        let (tx, _rx) = oneshot::channel();
        let request = sample_request();
        let prompt = build_render_prompt(&request, tx, BlockId(42));
        assert_eq!(prompt.id, BlockId(42));
        assert_eq!(prompt.tool_name, "bash");
        assert_eq!(prompt.reasoning, "run a harmless command");
        assert!(prompt
            .audit_hint
            .as_deref()
            .is_some_and(|hint| hint.contains("explicitly unblock")));
        assert_eq!(prompt.choices.len(), 3);
        assert_eq!(prompt.choices[0].key, 'y');
        assert_eq!(
            prompt.choices[1].decision,
            RenderPermissionDecision::AllowAlways
        );
    }

    #[tokio::test]
    async fn pump_translates_request_through_render_channel_and_resolves_responder() {
        let (prompter, request_rx) = ChannelPrompter::new(4);
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(8);
        let ids = BlockIdGen::default();

        // Drive the pump in a background task.
        let pump = tokio::spawn(run_permission_pump(request_rx, render_tx, ids));

        // Caller side: kick off a `decide` and keep its future alive.
        let prompter_clone = prompter.clone();
        let decide = tokio::spawn(async move { prompter_clone.decide(sample_request()).await });

        // TUI side: pull the prompt off the render channel and
        // respond with `AllowAlways`.
        let block = render_rx.recv().await.expect("prompt arrives");
        let responder = match block {
            RenderBlock::PermissionPrompt(prompt) => prompt.responder,
            other => panic!("unexpected block: {other:?}"),
        };
        responder
            .send(RenderPermissionDecision::AllowAlways)
            .expect("modal responder not dropped");

        let decision = decide.await.expect("decide join").expect("decide ok");
        assert_eq!(decision, L3Decision::Allow);

        // Dropping the live prompter closes the ChannelPrompter
        // receiver and lets the pump exit cleanly.
        drop(prompter);
        let pump_result = pump.await.expect("pump join");
        assert!(pump_result.is_ok(), "clean shutdown: {pump_result:?}");
    }

    #[tokio::test]
    async fn pump_hard_denies_when_render_channel_is_closed() {
        let (prompter, request_rx) = ChannelPrompter::new(4);
        let (render_tx, render_rx) = mpsc::channel::<RenderBlock>(1);
        drop(render_rx); // TUI never started.
        let pump = tokio::spawn(run_permission_pump(
            request_rx,
            render_tx,
            BlockIdGen::default(),
        ));

        let prompter_clone = prompter.clone();
        let decide = tokio::spawn(async move { prompter_clone.decide(sample_request()).await });

        // The pump should surface RenderChannelClosed, and the agent
        // side should see ResponderDropped.
        let pump_result = pump.await.expect("pump join");
        assert!(matches!(
            pump_result,
            Err(PermissionBridgeError::RenderChannelClosed)
        ));
        let decide_result = decide.await.expect("decide join");
        assert!(decide_result.is_err(), "ResponderDropped expected");

        drop(prompter);
    }

    #[tokio::test]
    async fn permission_remote_race_tui_wins_and_late_remote_answer_is_ignored() {
        let shared = remote_shared();
        let mut events = shared.events();
        let (prompter, request_rx) = ChannelPrompter::new(4);
        let (render_tx, mut render_rx) = mpsc::channel(4);
        let (resolution_tx, mut resolution_rx) = mpsc::unbounded_channel();
        let pump = tokio::spawn(run_permission_pump_with_remote(
            request_rx,
            render_tx,
            BlockIdGen::default(),
            Some(shared.clone()),
            Some(resolution_tx),
        ));
        let decide_prompter = prompter.clone();
        let decide = tokio::spawn(async move { decide_prompter.decide(sample_request()).await });

        let prompt = match render_rx.recv().await.expect("TUI prompt") {
            RenderBlock::PermissionPrompt(prompt) => prompt,
            other => panic!("unexpected block: {other:?}"),
        };
        let approval = next_remote_approval(&mut events).await;
        prompt
            .responder
            .send(RenderPermissionDecision::AllowAlways)
            .expect("TUI responder is live");

        assert_eq!(decide.await.expect("join").expect("decision"), L3Decision::Allow);
        assert!(matches!(
            shared.resolve_tool_approval(
                &approval.request_id,
                ToolApprovalDecision::Deny,
                ToolApprovalSource::Remote,
            ),
            ToolApprovalAttempt::AlreadyResolved(crate::remote_control::ToolApprovalResolution {
                decision: ToolApprovalDecision::AllowAlways,
                source: ToolApprovalSource::Tui,
            })
        ));
        assert!(resolution_rx.try_recv().is_err());

        drop(prompter);
        assert!(pump.await.expect("pump join").is_ok());
    }

    #[tokio::test]
    async fn permission_remote_race_remote_wins_and_tui_is_notified() {
        let shared = remote_shared();
        let mut events = shared.events();
        let (prompter, request_rx) = ChannelPrompter::new(4);
        let (render_tx, mut render_rx) = mpsc::channel(4);
        let (resolution_tx, mut resolution_rx) = mpsc::unbounded_channel();
        let pump = tokio::spawn(run_permission_pump_with_remote(
            request_rx,
            render_tx,
            BlockIdGen::default(),
            Some(shared.clone()),
            Some(resolution_tx),
        ));
        let decide_prompter = prompter.clone();
        let decide = tokio::spawn(async move { decide_prompter.decide(sample_request()).await });

        let prompt = match render_rx.recv().await.expect("TUI prompt") {
            RenderBlock::PermissionPrompt(prompt) => prompt,
            other => panic!("unexpected block: {other:?}"),
        };
        let approval = next_remote_approval(&mut events).await;
        assert_eq!(
            shared.resolve_tool_approval(
                &approval.request_id,
                ToolApprovalDecision::AllowOnce,
                ToolApprovalSource::Remote,
            ),
            ToolApprovalAttempt::Resolved
        );

        assert_eq!(
            decide.await.expect("join").expect("decision"),
            L3Decision::AllowOnce
        );
        assert_eq!(
            resolution_rx.recv().await.expect("TUI resolution notice"),
            RemotePermissionResolution {
                block_id: prompt.id,
                decision: RenderPermissionDecision::AllowOnce,
            }
        );
        assert!(prompt.responder.send(RenderPermissionDecision::Deny).is_err());
        assert!(matches!(
            shared.resolve_tool_approval(
                &approval.request_id,
                ToolApprovalDecision::Deny,
                ToolApprovalSource::Tui,
            ),
            ToolApprovalAttempt::AlreadyResolved(crate::remote_control::ToolApprovalResolution {
                decision: ToolApprovalDecision::AllowOnce,
                source: ToolApprovalSource::Remote,
            })
        ));

        drop(prompter);
        assert!(pump.await.expect("pump join").is_ok());
    }
}
