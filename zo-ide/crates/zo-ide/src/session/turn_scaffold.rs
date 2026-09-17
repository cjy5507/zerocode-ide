//! 턴 하나의 **배선** — 두 프런트엔드가 같은 한 벌을 쓴다.
//!
//! [`crate::tui::app`] 과 [`crate::ide::run_loop`] 은 화면이 완전히 다르다(하나는
//! painter 가 그리는 뷰포트, 하나는 append-only 스트림). 그런데 그 화면 뒤의
//! 배선 — 블록 채널, 권한 펌프, user-question 채널 설치, [`HookAbortSignal`] 과
//! 취소 플래그, 스티어 큐, IDE 채널의 `begin_turn`/`end_turn`, 턴이 끝난 뒤의
//! 잔여 블록 드레인 — 은 같다. 그것이 두 벌로 있으면 한쪽만 고쳐지고, 실제로
//! 그렇게 됐다(드레인 루프가 갈렸다). 그래서 배선만 여기 한 자리에 모은다.
//!
//! 쓰는 법은 세 걸음이다.
//!
//! 1. [`TurnScaffold::start`] — 채널·펌프·신호를 세우고 블록 수신기를 받는다.
//! 2. [`TurnScaffold::launch`] 로 꺼낸 [`TurnLaunch`] 를 턴 모양에 맞게 쓴다.
//!    세션을 빌린 채 도는 프런트는 [`TurnLaunch::drive`], 세션을 통째로 다른
//!    task 에 넘기는 프런트는 [`TurnLaunch::spawn`].
//! 3. 턴이 끝나면 [`TurnScaffold::drain`] 으로 남은 블록을 비우고
//!    [`TurnScaffold::finish`] 로 펌프를 끊고 IDE 에 종료를 알린다.
//!
//! 남는 것은 화면 처리뿐이다 — `select!` 팔의 UI 반응, 파킹/다이얼로그, 노트.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::FutureExt as _;
use runtime::message_stream::{BlockIdGen, RenderBlock};
use runtime::permission::{ChannelPrompter, PermissionPrompter};
use runtime::{HookAbortSignal, SteeringQueue, ToolCancelSignal, TurnSummary};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::ide::channel::wire::TurnOutcome;
use crate::ide::events::{self, EventsChannel};
use crate::session::permission_bridge::{run_permission_pump, PermissionBridgeError};
use crate::session::plain_session::PlainSession;
use crate::session::user_question_bridge::install_tui_user_question_channel;

/// 블록 채널 용량 — 두 프런트엔드가 같은 값을 쓴다.
const RENDER_CHANNEL_CAPACITY: usize = 64;

/// 권한 프롬프터가 한 번에 물고 있을 수 있는 요청 수. code-rules R8 이 작게
/// 두라고 하는 자리다 — 펌프가 한 장씩 렌더 채널로 옮긴다.
const PERMISSION_QUEUE_CAPACITY: usize = 4;

/// 도는 턴 하나에 붙어 있는 배선 전부.
pub(crate) struct TurnScaffold {
    /// 이번 턴의 블록 id 발급기. 프런트가 제 손으로 만드는 블록
    /// (유저 메시지 셀)도 같은 발급기에서 번호를 받아야 한다.
    pub(crate) ids: BlockIdGen,
    /// 훅 중단 신호 — 취소가 훅 실행까지 닿는 길.
    abort: HookAbortSignal,
    /// 사람이 끊었는가. 턴 결과 판정([`Self::finish`])이 이 값을 읽는다.
    cancel: Arc<AtomicBool>,
    /// 열려 있으면 IDE 이벤트 채널. 맨 터미널에서는 `None` 이다.
    pub(crate) ide: Option<&'static EventsChannel>,
    /// 턴 중 스티어 큐. 런타임이 없으면 `None`.
    steer: Option<SteeringQueue>,
    /// 도는 도구를 끊는 신호. `abort`·`cancel` 은 깃발이라 도구 디스패치의
    /// `select!` 가 읽지 않는다 — 물린 도구를 실제로 놓게 하는 건 이것뿐이다.
    tool_cancel: ToolCancelSignal,
    /// IDE 채널이 발급한 이번 턴 번호.
    turn_id: Option<u64>,
    /// 권한 요청을 [`RenderBlock::PermissionPrompt`] 로 바꾸는 펌프.
    pump: JoinHandle<Result<(), PermissionBridgeError>>,
    /// 턴 future 에 넘길 재료 — [`Self::launch`] 가 한 번만 꺼낸다.
    launch: Option<TurnLaunch>,
}

/// `run_turn` 에 넘길 재료 한 벌. 한 턴에 한 번만 존재한다 — 소유로 넘겨야
/// 턴 future 가 스캐폴드를 빌리지 않고, 프런트가 도는 동안 스캐폴드를 그대로
/// 쓸 수 있다.
pub(crate) struct TurnLaunch {
    blocks: mpsc::Sender<RenderBlock>,
    prompter: Arc<dyn PermissionPrompter>,
    abort: HookAbortSignal,
    cancel: Arc<AtomicBool>,
}

impl TurnScaffold {
    /// 배선을 세운다. 돌려주는 수신기가 이번 턴의 렌더 블록 전부를 나른다.
    ///
    /// IDE 채널이 열려 있으면 여기서 `turn{phase:"start"}` 가 나간다 — 창은
    /// 첫 블록보다 **먼저** 턴이 열린 것을 본다.
    pub(crate) fn start(session: &mut PlainSession) -> (Self, mpsc::Receiver<RenderBlock>) {
        let ide = events::channel();
        let turn_id = ide.map(EventsChannel::begin_turn);
        let (block_tx, block_rx) = mpsc::channel::<RenderBlock>(RENDER_CHANNEL_CAPACITY);
        let ids = BlockIdGen::default();

        let (prompter, request_rx) = ChannelPrompter::new(PERMISSION_QUEUE_CAPACITY);
        let pump = tokio::spawn(run_permission_pump(
            request_rx,
            block_tx.clone(),
            ids.clone(),
        ));
        if let Some(inner) = session.runtime.try_runtime_mut() {
            install_tui_user_question_channel(
                inner.tool_executor_mut().tool_registry_mut(),
                block_tx.clone(),
                ids.clone(),
            );
        }

        let abort = HookAbortSignal::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let scaffold = Self {
            ids,
            abort: abort.clone(),
            cancel: Arc::clone(&cancel),
            ide,
            steer: session.steering_handle(),
            tool_cancel: session.tool_cancel_handle(),
            turn_id,
            pump,
            launch: Some(TurnLaunch {
                blocks: block_tx,
                prompter: Arc::new(prompter),
                abort,
                cancel,
            }),
        };
        (scaffold, block_rx)
    }

    /// 턴 future 에 넘길 재료를 꺼낸다 — 한 턴에 한 번.
    ///
    /// # Panics
    ///
    /// 같은 스캐폴드에서 두 번 부르면 패닉한다. 턴은 하나뿐이다.
    pub(crate) fn launch(&mut self) -> TurnLaunch {
        self.launch
            .take()
            .expect("a scaffold launches exactly one turn")
    }

    /// 사람이(또는 IDE 의 Stop 이) 턴을 끊었다 — 취소 플래그와 훅 중단을 함께
    /// 세운다. 둘 중 하나만 세우면 훅이 계속 돌거나(abort 누락) 끝난 턴이
    /// 실패로 보고된다(cancel 누락).
    pub(crate) fn cancel_turn(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.abort.abort();
        // 깃발 **다음에** 이것. 도구가 떠 있는 동안 턴은 런타임의
        // `await_cancellable_tool_dispatch` 안에 주차돼 있고, 그 `select!` 의
        // 두 팔은 "도구 완료" 와 "이 신호" 뿐이다 — 위의 깃발 둘은 거기서
        // 아무도 읽지 않는다. 신호가 마지막인 건 순서 때문이다: 깨어난 턴이
        // 다음 스트림 레이스에서 곧바로 중단 깃발을 보고 취소로 앉도록.
        //
        // 신호는 물린 포그라운드 bash 에 SIGINT 까지 보낸다(런타임 쪽
        // `interrupt_foreground_bash`) — 끊긴 도구가 뒤에 남지 않는다.
        self.tool_cancel.cancel_running_tools();
    }

    /// 사람이 이 턴을 끊었는가. 화면은 이 답으로 취소와 실패를 가른다 —
    /// 취소가 남기는 "receiver dropped" 는 사고가 아니라 사람의 뜻이다.
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// 사람이 턴 중에 넣은 줄을 스티어 큐에 넣는다. 줄이 큐에 들어갔으면 참 —
    /// 런타임이 없어 큐 자체가 없으면 거짓이고, 그때 화면은 `↳ steer queued`
    /// 를 쓰지 않는다(들어가지도 않은 줄을 들어갔다고 적지 않는다).
    pub(crate) fn steer(&self, text: String) -> bool {
        let Some(queue) = self.steer.as_ref() else {
            return false;
        };
        queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(text);
        true
    }

    /// 블록 한 장을 IDE 채널에 싣는다 — 프롬프트면 그 `prompt_id`.
    pub(crate) fn publish(&self, block: &RenderBlock) -> Option<u64> {
        self.ide.and_then(|channel| channel.publish(block))
    }

    /// 턴 future 가 sender 를 놓은 뒤 채널에 남은 블록을 비운다.
    ///
    /// 남은 블록도 **살아 있는 블록과 같은 배선**을 지난다 — 훅 보고(호출자의
    /// `handle` 안)와 IDE 채널을 거친다. 두 프런트엔드가 여기서 갈려 있었고,
    /// 한쪽은 화면에만 밀어넣고 훅과 채널을 건너뛰었다. 건너뛰면 마지막
    /// `ToolResult` 와 답변 꼬리가 사라진다: Stop 훅이 받는 마지막 어시스턴트
    /// 텍스트가 잘리고, IDE 카드는 마지막 도구가 영원히 도는 채로 닫힌다.
    /// 스트림의 마지막 몇 장이 이 자리에 남는 것은 드문 일이 아니라 **흔한**
    /// 일이다 — 턴 future 는 sender 를 놓자마자 끝나고, 채널에는 아직 우리가
    /// 안 읽은 것이 남는다.
    ///
    /// 프롬프트만 예외다. 답할 responder 는 턴과 함께 이미 죽었으므로 창에
    /// 싣지 않는다 — 실었다면 바로 뒤 [`Self::finish`] 의 `end_turn` 이 같은
    /// 프레임을 도로 거둔다. 화면에서 내리는 일(파킹 해제·dismiss)은 호출자의
    /// `handle` 이 한다.
    pub(crate) fn drain<F>(&self, blocks: &mut mpsc::Receiver<RenderBlock>, mut handle: F)
    where
        F: FnMut(RenderBlock),
    {
        while let Ok(block) = blocks.try_recv() {
            if !is_prompt(&block) {
                let _ = self.publish(&block);
            }
            handle(block);
        }
    }

    /// 턴이 끝났다 — 펌프를 끊고 IDE 에 결과를 알린다.
    ///
    /// 결과 판정은 한 벌이다: 오류면 실패, 오류가 없고 사람이 끊었으면 취소,
    /// 아니면 완료. 취소된 턴이 실패로 나가면 창의 카드가 빨개진다.
    pub(crate) fn finish(self, outcome: &Result<TurnSummary, String>) {
        self.pump.abort();
        let (Some(channel), Some(turn_id)) = (self.ide, self.turn_id) else {
            return;
        };
        let (result, error) = match outcome {
            Err(error) => (TurnOutcome::Failed, Some(error.clone())),
            Ok(_) if self.cancelled() => (TurnOutcome::Cancelled, None),
            Ok(_) => (TurnOutcome::Completed, None),
        };
        channel.end_turn(turn_id, result, error.as_deref());
    }
}

impl TurnLaunch {
    /// 세션을 **빌린 채** 도는 턴 — 호출자가 이 future 를 제 `select!` 에
    /// pin 한다(파이프 REPL).
    pub(crate) async fn drive(
        self,
        session: &mut PlainSession,
        input: &str,
    ) -> Result<TurnSummary, String> {
        let outcome = Box::pin(catch_lifeline_panic(session.run_turn(
            input,
            self.blocks,
            self.prompter,
            self.abort,
            self.cancel,
        )))
        .await;
        match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                session.record_process_event("lifeline_panic", &error);
                Err(error)
            }
        }
    }

    /// 세션을 **통째로 가져가** 다른 task 에서 도는 턴 — 턴 준비의 동기 구간이
    /// 프레임 틱과 키 입력을 붙잡지 않아야 하는 프런트(TUI)가 쓴다. 세션은
    /// join 값으로 돌아온다.
    pub(crate) fn spawn(
        self,
        mut session: PlainSession,
        input: String,
        images: Vec<(String, String)>,
    ) -> JoinHandle<(PlainSession, Result<TurnSummary, String>)> {
        tokio::spawn(async move {
            let guarded = Box::pin(catch_lifeline_panic(session.run_turn_with_images(
                &input,
                images,
                self.blocks,
                self.prompter,
                self.abort,
                self.cancel,
            )))
            .await;
            let outcome = match guarded {
                Ok(outcome) => outcome,
                Err(error) => {
                    session.record_process_event("lifeline_panic", &error);
                    Err(error)
                }
            };
            (session, outcome)
        })
    }
}

async fn catch_lifeline_panic<F, T>(future: F) -> Result<T, String>
where
    F: std::future::Future<Output = T>,
{
    std::panic::AssertUnwindSafe(future)
        .catch_unwind()
        .await
        .map_err(|payload| {
            let detail = super::process_lifecycle::panic_payload(payload.as_ref());
            format!("session lifeline panicked: {detail}")
        })
}

/// 사람의 답을 기다리는 블록인가.
const fn is_prompt(block: &RenderBlock) -> bool {
    matches!(
        block,
        RenderBlock::PermissionPrompt(_) | RenderBlock::UserQuestionPrompt(_)
    )
}

#[cfg(test)]
mod tests {
    use super::catch_lifeline_panic;

    #[tokio::test]
    async fn lifeline_success_returns_its_value() {
        let result = catch_lifeline_panic(async { 17_u8 }).await;

        assert_eq!(result, Ok(17));
    }

    #[tokio::test]
    async fn lifeline_panic_becomes_an_explicit_turn_error() {
        let result: Result<(), String> = catch_lifeline_panic(async {
            panic!("stream consumer stopped");
        })
        .await;

        assert_eq!(
            result.expect_err("panic must cross the lifeline boundary as an error"),
            "session lifeline panicked: stream consumer stopped"
        );
    }
}
