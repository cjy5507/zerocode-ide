//! 턴 하나를 `RenderBlock` 채널로 흘리는 드라이버.
//!
//! 포팅: zo-cli `session/ndjson_summary.rs` 의 `drive_render_stream` 만. `-p` 텍스트/NDJSON 원샷 드라이버들은 가져오지
//! 않았다 — 이 프런트엔드의 소비자는 항상 `ide::render` 다.
//!
//! 프롬프터 주입 지점이 여기다 — TUI 든 IDE 파이프든 자기 어댑터를
//! `Arc<dyn PermissionPrompter>` 로 넘긴다.

use std::sync::Arc;

use runtime::message_stream::RenderBlock;

use super::runtime_bridge::LiveAsyncApiClient;

/// 턴을 스트리밍 런타임으로 돌리며 모든 블록을 `block_tx` 로 전달한다.
///
/// 포워딩 루프는 턴이 내부 sender 를 놓으면 끝난다. 호출자의 receiver 가
/// 먼저 끊기면 진행 중 턴을 취소해, 사라진 소비자를 위해 비싼 모델 요청이
/// 끝까지 돌지 않게 한다.
pub(crate) async fn drive_render_stream(
    rt: &mut runtime::ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>,
    live_client: Arc<LiveAsyncApiClient>,
    input: String,
    images: Vec<(String, String)>,
    // 배너가 이미 말한 모델 — 이제 이 함수는 쓰지 않는다. 호출부(plain_session)
    // 를 건드리지 않으려고 자리만 남겨 뒀다.
    _model: &str,
    block_tx: tokio::sync::mpsc::Sender<RenderBlock>,
    prompter: Arc<dyn runtime::permission::PermissionPrompter>,
) -> Result<runtime::TurnSummary, String> {
    rt.set_async_api_client(live_client);
    let (render_tx, mut render_rx) = tokio::sync::mpsc::channel::<RenderBlock>(64);

    // 턴 시작을 알리는 `· session start · model …` System 블록이 여기 있었다.
    // codex 패인에는 그런 줄이 없다 — 배너가 이미 모델을 말했고, 턴마다
    // 되풀이할 이유도 없다. 렌더러에서 문자열로 숨기는 대신 근원에서 뺀다.
    let turn = rt.run_turn_streaming_maybe_deep(input, images, render_tx, prompter);
    let forward = async {
        while let Some(block) = render_rx.recv().await {
            block_tx.send(block).await.map_err(|_| ())?;
        }
        Ok::<(), ()>(())
    };
    tokio::pin!(turn);
    tokio::pin!(forward);
    let turn_result = tokio::select! {
        turn_result = &mut turn => {
            let _ = (&mut forward).await;
            turn_result
        }
        forward_result = &mut forward => match forward_result {
            Ok(()) => (&mut turn).await,
            Err(()) => return Err("render stream receiver dropped".to_string()),
        },
    };
    turn_result.map_err(|error| error.to_string())
}
