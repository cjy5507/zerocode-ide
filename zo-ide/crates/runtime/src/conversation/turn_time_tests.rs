//! Where a turn's time goes before its requests leave (t-78938): how often a
//! turn recalls and puts the recall to the seat's judgment.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ConcurrentDispatchFn,
    ConversationRuntime, RuntimeError, StaticToolExecutor, SteeringQueue,
};
use crate::message_stream::types::{BlockId, RenderBlock};
use crate::permission::{PermissionDecision, PermissionError, PermissionPrompter, PermissionRequest};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, MessageRole, Session};
use crate::{MemoryEntry, MemoryHit, MemoryRetriever, TurnProgress};

/// The sync client a runtime is built with; every request here goes out on
/// the async one.
struct UnusedSyncClient;

impl ApiClient for UnusedSyncClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(vec![AssistantEvent::TextDelta("done".to_string()), AssistantEvent::MessageStop])
    }
}

struct AllowAll;

impl PermissionPrompter for AllowAll {
    fn decide<'a>(
        &'a self,
        _request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<PermissionDecision, PermissionError>> + Send + 'a>> {
        Box::pin(async { Ok(PermissionDecision::Allow) })
    }
}

fn hit(slug: &str, summary: &str) -> MemoryHit {
    MemoryHit {
        entry: MemoryEntry { slug: slug.to_string(), path: format!("{slug}.md"), summary: summary.to_string() },
        score: 1,
    }
}

/// Recalls the parser note every time it is asked — or, once `grows`, the
/// parser note the first time and a note the session wrote since every time
/// after: the store changing under a turn, as a memory written mid-turn
/// changes it.
#[derive(Default)]
struct Store {
    grows: bool,
    recalls: AtomicUsize,
}

impl MemoryRetriever for Store {
    fn recall(&self, _query: &str, _k: usize) -> Vec<MemoryHit> {
        if self.recalls.fetch_add(1, Ordering::SeqCst) > 0 && self.grows {
            vec![hit("lexers", "written while the turn ran")]
        } else {
            vec![hit("parsers", "recall me about parser bugs")]
        }
    }
}

/// A seat that settles nothing and counts each recall it was asked about:
/// the query, and the notes recall handed it.
#[derive(Default)]
struct CountingSeat {
    asked: Mutex<Vec<(String, Vec<String>)>>,
}

impl CountingSeat {
    fn asked(&self) -> Vec<(String, Vec<String>)> {
        self.asked.lock().expect("asked").clone()
    }
}

impl crate::RecallSeat for CountingSeat {
    fn settle(&self, _attempt: &str, query: &str, hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
        let notes = hits.iter().map(|hit| hit.entry.slug.clone()).collect();
        self.asked.lock().expect("asked").push((query.to_string(), notes));
        hits
    }

    fn observe(&self, _attempt: &str, _progress: TurnProgress<'_>) {}
}

/// What one request carried of the recall: each recalled-memory block of
/// its transcript, in order.
fn recalled_blocks(request: &ApiRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::Text { text } if text.contains("# Recalled memory") => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Answers `script` a request at a time — `Some(path)` reads that file,
/// `None` answers in words — and keeps what each request carried of the
/// recall. `steer_on` names a request during which the person types `steer`.
struct Scripted {
    script: Vec<Option<&'static str>>,
    sent: AtomicUsize,
    carried: Mutex<Vec<Vec<String>>>,
    steer_on: Option<(usize, &'static str, SteeringQueue)>,
}

impl Scripted {
    fn new(script: Vec<Option<&'static str>>) -> Self {
        Self { script, sent: AtomicUsize::new(0), carried: Mutex::new(Vec::new()), steer_on: None }
    }

    fn carried(&self) -> Vec<Vec<String>> {
        self.carried.lock().expect("carried").clone()
    }
}

impl AsyncApiClient for Scripted {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        _render_tx: tokio::sync::mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        let sent = self.sent.fetch_add(1, Ordering::SeqCst);
        self.carried.lock().expect("carried").push(recalled_blocks(&request));
        if let Some((during, steer, queue)) = &self.steer_on {
            if *during == sent {
                queue.lock().expect("steering").push((*steer).to_string());
            }
        }
        let events = match self.script.get(sent).copied().flatten() {
            Some(path) => vec![
                AssistantEvent::ToolUse {
                    id: format!("tool-{sent}"),
                    name: "read_file".to_string(),
                    input: format!(r#"{{"path":"{path}"}}"#),
                },
                AssistantEvent::MessageStop,
            ],
            None => vec![AssistantEvent::TextDelta(format!("answer {sent}")), AssistantEvent::MessageStop],
        };
        Box::pin(async move { Ok(events) })
    }
}

/// A runtime reading files through the async seam, recalling from `store`
/// beside `seat`.
fn runtime_with(
    api: Arc<Scripted>,
    store: Arc<Store>,
    seat: Arc<CountingSeat>,
    steering: SteeringQueue,
) -> ConversationRuntime<UnusedSyncClient, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        UnusedSyncClient,
        StaticToolExecutor::new().register("read_file", |input| Ok(format!("read:{input}"))),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
    .with_async_api_client(api)
    .with_steering_queue(steering);
    let dispatch: ConcurrentDispatchFn = Arc::new(|_name, input| Ok(format!("read:{input}")));
    runtime.set_concurrent_dispatch(dispatch);
    runtime.set_memory_retriever(Some(store));
    runtime.set_recall_seat(Some(seat));
    runtime
}

fn run_turn(runtime: &mut ConversationRuntime<UnusedSyncClient, StaticToolExecutor>, input: &str) {
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    tokio_runtime.block_on(async {
        let (render_tx, mut render_rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowAll);
        runtime
            .run_turn_streaming_with_images(input, Vec::new(), render_tx, prompter)
            .await
            .expect("the turn ends");
    });
}

/// Z5: a turn puts its recall to the judgment once, however many requests
/// it sends — the query is the person's last words and the store holds
/// still, so every request recalls the same notes for the same words — and
/// each later request carries the section the first one built.
#[test]
fn a_turn_settles_its_recall_once_and_every_request_carries_the_same_section() {
    let api = Arc::new(Scripted::new(vec![Some("a.rs"), Some("b.rs"), Some("c.rs"), Some("d.rs"), None]));
    let seat = Arc::new(CountingSeat::default());
    let mut runtime = runtime_with(api.clone(), Arc::new(Store::default()), seat.clone(), SteeringQueue::default());

    run_turn(&mut runtime, "parser bugs: read the four files");

    let carried = api.carried();
    assert_eq!(carried.len(), 5, "the turn sent five requests");
    let settled = seat.asked().len();
    assert_eq!(settled, 1, "a five-request turn put its recall to the judgment {settled} times");
    assert!(
        carried[0].len() == 1 && carried[0][0].contains("parser bugs"),
        "the first request carries the recall: {:?}",
        carried[0]
    );
    for (request, blocks) in carried.iter().enumerate().skip(1) {
        assert_eq!(blocks, &carried[0], "request {} carried another recall section", request + 1);
    }
}

/// Z5: a memory written while the turn runs changes what recall hands the
/// seat, and other notes are another reading: judged once more, then kept
/// for the requests after it.
#[test]
fn notes_written_during_the_turn_are_judged_once_more() {
    let api = Arc::new(Scripted::new(vec![Some("a.rs"), Some("b.rs"), Some("c.rs"), Some("d.rs"), None]));
    let store = Arc::new(Store { grows: true, ..Store::default() });
    let seat = Arc::new(CountingSeat::default());
    let mut runtime = runtime_with(api.clone(), store, seat.clone(), SteeringQueue::default());

    run_turn(&mut runtime, "parser bugs: read the four files");

    assert_eq!(api.carried().len(), 5, "the turn sent five requests");
    let notes: Vec<Vec<String>> = seat.asked().into_iter().map(|(_, notes)| notes).collect();
    assert_eq!(notes, [vec!["parsers".to_string()], vec!["lexers".to_string()]], "each reading judged once");
}

/// Z5: words the person types during the turn are a new query, and a new
/// query is judged once more — then kept for the requests after it.
#[test]
fn words_typed_during_the_turn_are_judged_once_more() {
    let steering = SteeringQueue::default();
    let mut scripted = Scripted::new(vec![Some("a.rs"), None, Some("b.rs"), None]);
    scripted.steer_on = Some((1, "now the lexer notes", steering.clone()));
    let api = Arc::new(scripted);
    let seat = Arc::new(CountingSeat::default());
    let mut runtime = runtime_with(api.clone(), Arc::new(Store::default()), seat.clone(), steering);

    run_turn(&mut runtime, "parser bugs: read a.rs");

    assert_eq!(api.carried().len(), 4, "the steer took the turn to four requests");
    let queries: Vec<String> = seat.asked().into_iter().map(|(query, _)| query).collect();
    assert_eq!(queries.len(), 2, "two queries, each judged once: {queries:#?}");
    assert_eq!(queries[0], "parser bugs: read a.rs");
    assert!(queries[1].contains("now the lexer notes"), "the second query is the steer's: {:?}", queries[1]);
}
