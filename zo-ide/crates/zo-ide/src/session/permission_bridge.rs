//! 권한 프롬프트 브리지 — L3 `ChannelPrompter` ↔ `RenderBlock::PermissionPrompt`.
//!
//! 런타임은 [`runtime::permission::ChannelPrompter`] 로 `(PermissionRequest,
//! OneshotResponder)` 쌍을 밀어낸다. 이 펌프는 쌍마다 새 oneshot 을 만들어
//! 렌더 채널에 `PermissionPrompt` 블록으로 실어 보내고, 응답이 오면 L3
//! 결정 어휘로 되돌려 원래 responder 를 해소한다. 응답자가 블록을 버리면
//! responder 가 값 없이 drop 되어 런타임은 hard deny 로 읽는다.
//!
//! 포팅: zo-cli `session/permission_bridge.rs`. 원격(폰) 승인 경쟁
//! (`remote_control`) 은 이 트리에서 제거 — 응답자는 패인/이벤트 채널뿐이다.

use runtime::message_stream::{
    BlockId, BlockIdGen, PermissionChoice as RenderPermissionChoice,
    PermissionDecision as RenderPermissionDecision, PermissionPrompt, RenderBlock,
};
use runtime::permission::{
    OneshotResponder, PermissionChoice as L3PermissionChoice, PermissionDecision as L3Decision,
    PermissionRequest,
};
use tokio::sync::{mpsc, oneshot};

/// 펌프가 감독자에게 올릴 수 있는 오류.
#[derive(Debug, thiserror::Error)]
pub enum PermissionBridgeError {
    /// 프롬프트를 전달하기 전에 렌더 채널이 닫혔다(소비자 종료).
    #[error("render channel closed before permission prompt could be delivered")]
    RenderChannelClosed,
}

/// `request_rx` 가 닫힐 때까지 번역 펌프를 돈다.
///
/// `ids` 는 턴의 나머지 블록과 공유하는 [`BlockIdGen`] — 프롬프트 블록이
/// 텍스트/툴 블록과 연속 id 를 받는다.
pub async fn run_permission_pump(
    mut request_rx: mpsc::Receiver<(PermissionRequest, OneshotResponder)>,
    render_tx: mpsc::Sender<RenderBlock>,
    ids: BlockIdGen,
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
            drop(l3_responder);
            return Err(PermissionBridgeError::RenderChannelClosed);
        }
        match modal_rx.await.ok() {
            Some(render_decision) => {
                let _ = l3_responder.respond(map_decision(render_decision));
            }
            None => drop(l3_responder),
        }
    }
    Ok(())
}

/// L3 요청 → 렌더 블록 프롬프트. `modal_tx` 가 responder 로 실린다.
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

fn map_choice(choice: &L3PermissionChoice) -> RenderPermissionChoice {
    RenderPermissionChoice {
        key: choice.key,
        label: choice.label.clone(),
        decision: map_decision_forward(choice.decision),
    }
}

/// 렌더 결정 → L3 결정. `AllowAlways`/`DenyAlways` 는 L3 의 `Allow`/`Deny` 로 접힌다.
#[must_use]
pub fn map_decision(decision: RenderPermissionDecision) -> L3Decision {
    match decision {
        RenderPermissionDecision::AllowOnce => L3Decision::AllowOnce,
        RenderPermissionDecision::AllowAlways => L3Decision::Allow,
        RenderPermissionDecision::Deny | RenderPermissionDecision::DenyAlways => L3Decision::Deny,
    }
}

/// L3 결정 → 렌더 결정(무손실 방향).
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

    #[test]
    fn decision_round_trip_collapses_remembered_forms() {
        assert_eq!(map_decision(RenderPermissionDecision::AllowOnce), L3Decision::AllowOnce);
        assert_eq!(map_decision(RenderPermissionDecision::AllowAlways), L3Decision::Allow);
        assert_eq!(map_decision(RenderPermissionDecision::Deny), L3Decision::Deny);
        assert_eq!(map_decision(RenderPermissionDecision::DenyAlways), L3Decision::Deny);
        assert_eq!(
            map_decision_forward(L3Decision::Allow),
            RenderPermissionDecision::AllowAlways
        );
    }

    #[tokio::test]
    async fn pump_forwards_prompt_and_resolves_responder() {
        use runtime::permission::{ChannelPrompter, PermissionPrompter};

        let (prompter, request_rx) = ChannelPrompter::new(4);
        let (render_tx, mut render_rx) = mpsc::channel::<RenderBlock>(4);
        let pump = tokio::spawn(run_permission_pump(
            request_rx,
            render_tx,
            BlockIdGen::default(),
        ));

        let request = PermissionRequest {
            tool: "Bash".to_string(),
            input_summary: "ls".to_string(),
            input_hash: String::new(),
            reasoning: "run ls".to_string(),
            choices: vec![L3PermissionChoice {
                key: 'y',
                label: "Allow once".to_string(),
                decision: L3Decision::AllowOnce,
            }],
            risk_level: runtime::permission::RiskLevel::Low,
        };
        let decide = tokio::spawn(async move { prompter.decide(request).await });

        let Some(RenderBlock::PermissionPrompt(prompt)) = render_rx.recv().await else {
            panic!("expected a permission prompt block");
        };
        assert_eq!(prompt.tool_name, "Bash");
        prompt
            .responder
            .send(RenderPermissionDecision::AllowOnce)
            .expect("modal answer");

        let decision = decide.await.expect("join").expect("decision");
        assert_eq!(decision, L3Decision::AllowOnce);
        drop(render_rx);
        let _ = pump.await;
    }
}
