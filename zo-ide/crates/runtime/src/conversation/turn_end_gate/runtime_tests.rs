//! The turn-end gate through a whole turn (t-58176): a scripted model that
//! answers the same way every time, and what the person sees — how many
//! times the gate re-prompts and how many replies the turn prints.
//!
//! The live case: a long evaluation ending on 「정해 주시면 그 범위로
//! 작업하겠습니다」 was read as a promise, the reminder asked for "a complete
//! final answer", and the model wrote the whole evaluation again.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::super::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ConversationRuntime, RuntimeError,
    StaticToolExecutor,
};
use super::{turn_end_gate_reminder, TurnEndingIssue};
use crate::message_stream::types::{BlockId, RenderBlock};
use crate::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError,
    PermissionPrompter as AsyncPermissionPrompter, PermissionRequest as AsyncPermissionRequest,
};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, MessageRole, Session};

/// The reply in the person's picture: several paragraphs of evaluation, then
/// the choice handed back to them.
const EVALUATION_HANDED_BACK: &str = "설계 문서를 평가했습니다.\n\n\
     ## 강점\n- 단계가 분명하고 단계마다 인수 기준이 있습니다.\n- 위험 항목이 따로 정리되어 있습니다.\n\n\
     ## 약점\n1. 2단계의 롤백 절차가 빠져 있습니다.\n2. 성능 목표에 숫자가 없습니다.\n3. 시험 계획이 단위 시험에 머뭅니다.\n\n\
     어느 항목부터 진행할지 정해 주시면 그 범위로 작업하겠습니다.";

/// A reply that ends on a promise nobody else is waiting on.
const EVALUATION_THEN_PROMISE: &str = "설계 문서를 평가했습니다.\n\n\
     ## 약점\n1. 2단계의 롤백 절차가 빠져 있습니다.\n2. 성능 목표에 숫자가 없습니다.\n\n\
     다음으로 시험을 실행하겠습니다.";

/// A promise after a clause that tells what happened, not what the person
/// should do first (ZO review of t-58176).
const TIME_CLAUSE_THEN_PROMISE: &str =
    "The tests ran.\n\nWhen you ran it the build broke, so I'll fix it.";

const QUESTION_ENDING: &str = "The migration is ready.\n\nShould I also update the staging config?";

/// Answers with `replies` in order, then repeats the last one — a model that
/// writes the same answer however often it is asked.
struct Replies {
    replies: Vec<&'static str>,
    calls: AtomicUsize,
}

impl Replies {
    fn new(replies: &[&'static str]) -> Self {
        Self {
            replies: replies.to_vec(),
            calls: AtomicUsize::new(0),
        }
    }

    fn next_events(&self) -> Vec<AssistantEvent> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let text = self.replies[call.min(self.replies.len() - 1)];
        vec![
            AssistantEvent::TextDelta(text.to_string()),
            AssistantEvent::MessageStop,
        ]
    }
}

impl ApiClient for Replies {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(self.next_events())
    }
}

impl AsyncApiClient for Replies {
    fn stream_async<'a>(
        &'a self,
        _request: ApiRequest,
        _render_tx: tokio::sync::mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move { Ok(self.next_events()) })
    }
}

struct Allow;
impl AsyncPermissionPrompter for Allow {
    fn decide<'a>(
        &'a self,
        _request: AsyncPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(AsyncPermissionDecision::Allow) })
    }
}

/// What a turn left in the session: the gate's re-prompts, by issue, and the
/// number of replies the model printed.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    reprompts: Vec<TurnEndingIssue>,
    replies: usize,
}

fn seen(session: &Session) -> Seen {
    let issues = [
        TurnEndingIssue::Promise,
        TurnEndingIssue::Question,
        TurnEndingIssue::TextualToolCall,
        TurnEndingIssue::UnverifiedCompletion,
    ];
    let reprompts = session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } if text.starts_with("[zo:turn-end-gate]") => issues
                .into_iter()
                .find(|issue| text == turn_end_gate_reminder(*issue)),
            _ => None,
        })
        .collect();
    let replies = session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    Seen { reprompts, replies }
}

/// The sync loop on an autonomous surface takes the whole gate.
fn autonomous_turn(replies: &[&'static str]) -> Seen {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        Replies::new(replies),
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::WorkspaceWrite),
        vec!["system".to_string()],
    );
    runtime.set_autonomous_surface(true);
    runtime.run_turn("평가해 줘", None).expect("turn runs");
    seen(runtime.session())
}

/// The streaming loop of an attended session — the surface in the picture.
fn streaming_turn(replies: &[&'static str]) -> Seen {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        Replies::new(replies),
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
    .with_async_api_client(Arc::new(Replies::new(replies)));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        let (render_tx, mut render_rx) = tokio::sync::mpsc::channel(256);
        let drain = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn AsyncPermissionPrompter> = Arc::new(Allow);
        runtime
            .run_turn_streaming_maybe_deep("평가해 줘", Vec::new(), render_tx, prompter)
            .await
            .expect("streaming turn finishes");
        drain.abort();
    });
    seen(runtime.session())
}

/// The picture's turn: an ending that hands the choice back draws no
/// re-prompt, so the evaluation is printed once.
#[test]
fn the_evaluation_handed_back_is_printed_once() {
    let expected = Seen {
        reprompts: Vec::new(),
        replies: 1,
    };
    assert_eq!(streaming_turn(&[EVALUATION_HANDED_BACK]), expected);
    assert_eq!(autonomous_turn(&[EVALUATION_HANDED_BACK]), expected);
}

/// A real promise is still asked about — once a turn: a model that answers
/// the reminder with the same promise is not asked again.
#[test]
fn an_unconditional_promise_is_asked_about_once_a_turn() {
    let expected = Seen {
        reprompts: vec![TurnEndingIssue::Promise],
        replies: 2,
    };
    assert_eq!(streaming_turn(&[EVALUATION_THEN_PROMISE]), expected);
    assert_eq!(autonomous_turn(&[EVALUATION_THEN_PROMISE]), expected);
}

/// "When you …" that tells what happened leaves the promise a promise: it
/// is asked about once, like any other.
#[test]
fn a_promise_after_a_time_clause_is_asked_about_once() {
    let expected = Seen {
        reprompts: vec![TurnEndingIssue::Promise],
        replies: 2,
    };
    assert_eq!(streaming_turn(&[TIME_CLAUSE_THEN_PROMISE]), expected);
    assert_eq!(autonomous_turn(&[TIME_CLAUSE_THEN_PROMISE]), expected);
}

/// The other issues keep the shared cap of two: a question asked twice is
/// re-prompted twice, and a promise's one re-prompt leaves a later question
/// its own.
#[test]
fn the_other_issues_keep_their_cap() {
    assert_eq!(
        autonomous_turn(&[QUESTION_ENDING]),
        Seen {
            reprompts: vec![TurnEndingIssue::Question, TurnEndingIssue::Question],
            replies: 3,
        }
    );
    assert_eq!(
        autonomous_turn(&[EVALUATION_THEN_PROMISE, QUESTION_ENDING]),
        Seen {
            reprompts: vec![TurnEndingIssue::Promise, TurnEndingIssue::Question],
            replies: 3,
        }
    );
}
