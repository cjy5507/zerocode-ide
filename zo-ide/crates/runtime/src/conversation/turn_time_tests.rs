//! Where a turn's time goes before its requests leave (t-78938): how often a
//! turn recalls and puts the recall to the seat's judgment, and what its
//! timing rows say of the waits in front of each request.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ConcurrentDispatchFn,
    ConversationRuntime, RuntimeError, StaticToolExecutor, SteeringQueue,
};
use crate::message_stream::types::{BlockId, RenderBlock, StreamPhase};
use crate::permission::{PermissionDecision, PermissionError, PermissionPrompter, PermissionRequest};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::request_timings::{take_recorded, timing_record, StreamStamps, TimingContext};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};
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

/// Answers `script` a request at a time — the tools an answer calls, by
/// name and input, or none for an answer in words — and keeps what each
/// request carried of the recall. With `announces`, each tool block is also
/// announced on the probe channel as closed, as the Anthropic parser
/// announces one whose input is complete (`ToolCallStatus::Running`).
/// `steer_on` names a request during which the person types `steer`.
struct Scripted {
    script: Vec<Vec<(&'static str, String)>>,
    announces: bool,
    sent: AtomicUsize,
    carried: Mutex<Vec<Vec<String>>>,
    steer_on: Option<(usize, &'static str, SteeringQueue)>,
}

impl Scripted {
    /// Each `Some(path)` an answer that reads that file, each `None` one in
    /// words.
    fn new(script: Vec<Option<&'static str>>) -> Self {
        Self::calling(
            script
                .into_iter()
                .map(|read| read.map_or_else(Vec::new, |path| vec![("read_file", format!(r#"{{"path":"{path}"}}"#))]))
                .collect(),
        )
    }

    fn calling(script: Vec<Vec<(&'static str, String)>>) -> Self {
        Self { script, announces: false, sent: AtomicUsize::new(0), carried: Mutex::new(Vec::new()), steer_on: None }
    }

    fn carried(&self) -> Vec<Vec<String>> {
        self.carried.lock().expect("carried").clone()
    }
}

impl AsyncApiClient for Scripted {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: tokio::sync::mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        use crate::message_stream::types::{ToolCallId, ToolCallStatus, ToolPreview};

        let sent = self.sent.fetch_add(1, Ordering::SeqCst);
        self.carried.lock().expect("carried").push(recalled_blocks(&request));
        if let Some((during, steer, queue)) = &self.steer_on {
            if *during == sent {
                queue.lock().expect("steering").push((*steer).to_string());
            }
        }
        let calls = self.script.get(sent).cloned().unwrap_or_default();
        let mut announced = Vec::new();
        let mut events = Vec::new();
        for (index, (name, input)) in calls.iter().enumerate() {
            let id = format!("tool-{sent}-{index}");
            if self.announces {
                announced.push(RenderBlock::ToolCall {
                    id: BlockId(u64::try_from(index).expect("a block id")),
                    tool_call_id: ToolCallId(id.clone()),
                    name: (*name).to_string(),
                    summary: String::new(),
                    preview: ToolPreview::Generic { name: (*name).to_string(), input_summary: String::new() },
                    input_print: Some(input.clone()),
                    status: ToolCallStatus::Running,
                });
            }
            events.push(AssistantEvent::ToolUse { id, name: (*name).to_string(), input: input.clone() });
        }
        if calls.is_empty() {
            events.push(AssistantEvent::TextDelta(format!("answer {sent}")));
        }
        events.push(AssistantEvent::MessageStop);
        Box::pin(async move {
            for block in announced {
                let _ = render_tx.send(block).await;
            }
            Ok(events)
        })
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

/// Leaves the last note out the first time it is asked and nothing after —
/// a seat whose answer moves between askings, as a judgment that lands in
/// its memo after the wall moves the apply road's.
#[derive(Default)]
struct ChangesItsMind {
    asked: AtomicUsize,
}

impl crate::RecallSeat for ChangesItsMind {
    fn settle(&self, _attempt: &str, _query: &str, mut hits: Vec<MemoryHit>) -> Vec<MemoryHit> {
        if self.asked.fetch_add(1, Ordering::SeqCst) == 0 {
            hits.pop();
        }
        hits
    }
}

/// Z5: the turn's seat asks the seat beside recall once per reading — the
/// same attempt, query and notes — and hands every later asking the first
/// answer; another attempt, another query or other notes is asked again.
#[test]
fn the_turns_seat_asks_once_per_reading_and_keeps_the_first_answer() {
    use crate::RecallSeat as _;

    let seat = Arc::new(ChangesItsMind::default());
    let once = super::turn_support::SettledOnce::over(seat.clone(), Arc::default());
    let two = vec![hit("parsers", "recall me about parser bugs"), hit("lexers", "and lexer bugs")];
    for _ in 0..5 {
        let read = once.settle("s@1", "parser bugs", two.clone());
        assert_eq!(read, two[..1], "the first answer, every time");
    }
    assert_eq!(seat.asked.load(Ordering::SeqCst), 1, "five askings of one reading");

    once.settle("s@1", "parser bugs", two[1..].to_vec());
    once.settle("s@1", "lexer bugs", two[1..].to_vec());
    once.settle("s@2", "lexer bugs", two[1..].to_vec());
    assert_eq!(seat.asked.load(Ordering::SeqCst), 4, "other notes, another query, another attempt");
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

/// The context a row is written under, as the ledger names it.
fn context() -> TimingContext<'static> {
    TimingContext {
        session_id: "session-1",
        model: None,
        iteration: 1,
        request_messages: 2,
        recorded_at: 1_789_000_000,
        outcome: "completed",
        attempt: "session-1@1",
    }
}

/// The rows the turns run on this thread since the last look would have
/// written, as the ledger writes them.
fn recorded_rows() -> Vec<serde_json::Value> {
    take_recorded()
        .iter()
        .map(|stamps| serde_json::to_value(timing_record(&context(), stamps)).expect("a row"))
        .collect()
}

/// A skill judgment that answers as soon as it is asked, with nothing to say.
struct AnswersAtOnce;

impl crate::skill_rank::SkillSuggestionSeat for AnswersAtOnce {
    fn suggest(&self, _request: String) -> futures_util::future::BoxFuture<'_, Option<String>> {
        Box::pin(async { None })
    }

    fn finish(&self, _turn: &[ConversationMessage]) {}
}

/// Z6: a row says how long the turn-start judgments held its request, how
/// long the compaction preflight took, and the whole wait from the turn's
/// start until the request left — read off stamps given as instants, so
/// nothing is waited out.
#[test]
fn a_row_says_how_long_the_turn_start_judgments_held_its_request() {
    let ms = Duration::from_millis;
    let held = super::TURN_START_JUDGMENT_WAIT;
    let t0 = Instant::now();
    let judged = t0 + ms(4) + held;
    let assembling = judged + ms(30);
    let mut stamps = StreamStamps::begun(assembling);
    stamps.turn_started = Some(t0);
    stamps.judge_wait = Some(t0 + ms(4)..judged);
    stamps.preflight = Some(judged..assembling);
    stamps.observe_at(&RenderBlock::StreamPhase(StreamPhase::RequestSent { attempt: 1 }), assembling + ms(2));
    stamps.complete_at(assembling + ms(900));

    let row = serde_json::to_value(timing_record(&context(), &stamps)).expect("a row");
    let held_ms = u64::try_from(held.as_millis()).expect("a wait in ms");
    assert_eq!(row["judge_wait_ms"].as_u64(), Some(held_ms), "the judgments' wait is on the row: {row}");
    assert_eq!(row["preflight_ms"].as_u64(), Some(30), "{row}");
    assert_eq!(row["assemble_ms"].as_u64(), Some(2), "assembly is measured as it was: {row}");
    assert_eq!(row["turn_start_ms"].as_u64(), Some(4 + held_ms + 30 + 2), "the turn's start to the send: {row}");
}

/// Z6: a turn's first request carries the turn-start judgments' wait and
/// the compaction preflight; the requests after it carry neither, and each
/// says how long after the turn's start it left.
#[test]
fn only_a_turns_first_row_carries_the_judgments_wait_and_the_preflight() {
    let api = Arc::new(Scripted::new(vec![Some("a.rs"), None]));
    let mut runtime =
        runtime_with(api, Arc::new(Store::default()), Arc::new(CountingSeat::default()), SteeringQueue::default());
    runtime.set_skill_suggestion_seat(Some(Arc::new(AnswersAtOnce)));
    take_recorded();

    run_turn(&mut runtime, "read a.rs");

    let rows = recorded_rows();
    assert_eq!(rows.len(), 2, "two requests, two rows: {rows:#?}");
    let first = &rows[0];
    let waited = first["judge_wait_ms"].as_u64().expect("the first row says how long the judgments held it");
    let preflight = first["preflight_ms"].as_u64().expect("the first row says how long the preflight took");
    let assembled = first["assemble_ms"].as_u64().expect("the first row's assembly");
    let entered = first["turn_start_ms"].as_u64().expect("the first row's wait from the turn's start");
    assert!(waited + preflight + assembled <= entered, "the parts of a wait fit inside it: {first}");
    let second = &rows[1];
    assert!(second["judge_wait_ms"].is_null() && second["preflight_ms"].is_null(), "{second}");
    assert!(second["turn_start_ms"].as_u64() >= Some(entered), "{second}");
}

/// Z6: the turn's clock takes the turn-start judgments' wait when one was
/// asked — a judgment that answers at once holds the turn a moment, far
/// inside the wait — and none when nothing was asked.
#[test]
fn the_turns_clock_takes_the_judgments_wait_only_when_one_was_asked() {
    let mut runtime =
        runtime_with(Arc::new(Scripted::new(Vec::new())), Arc::new(Store::default()), Arc::new(CountingSeat::default()), SteeringQueue::default());
    runtime.run_user_prompt_submit_for_streaming_user_entry("read a.rs").expect("the turn proceeds");
    assert!(runtime.turn_clock.started.is_some(), "the turn's start is stamped");
    assert_eq!(runtime.turn_clock.judge_wait, None, "nothing was asked, so nothing held the turn");

    runtime.set_skill_suggestion_seat(Some(Arc::new(AnswersAtOnce)));
    runtime.run_user_prompt_submit_for_streaming_user_entry("read a.rs").expect("the turn proceeds");
    let held = runtime.turn_clock.judge_wait.clone().expect("a judgment was asked and waited for");
    assert!(held.end - held.start < super::TURN_START_JUDGMENT_WAIT, "an answer at once is no wall: {held:?}");
}

/// The names on a row's tool marks, in order.
fn tool_names(marks: &serde_json::Value) -> Vec<String> {
    marks.as_array().map_or_else(Vec::new, |marks| {
        marks.iter().map(|mark| mark["name"].as_str().unwrap_or_default().to_string()).collect()
    })
}

/// Z13: an answer that calls two tools says on its own row when each tool's
/// block closed, and the request that carries their results back says on
/// its row how long each ran — the two rows joined by their attempt and
/// iteration, and by each tool's name, never its input or its result.
#[test]
fn a_two_tool_answer_says_when_its_blocks_closed_and_the_next_request_how_long_each_ran() {
    let mut scripted = Scripted::calling(vec![
        vec![
            ("read_file", r#"{"path":"a.rs"}"#.to_string()),
            ("write_file", r#"{"path":"b.rs","content":"fn b() {}"}"#.to_string()),
        ],
        Vec::new(),
    ]);
    scripted.announces = true;
    let mut runtime = runtime_with(
        Arc::new(scripted),
        Arc::new(Store::default()),
        Arc::new(CountingSeat::default()),
        SteeringQueue::default(),
    );
    take_recorded();

    run_turn(&mut runtime, "read a.rs, then write b.rs");

    let rows = recorded_rows();
    assert_eq!(rows.len(), 2, "two requests, two rows: {rows:#?}");
    let (answer, after) = (&rows[0], &rows[1]);
    assert_eq!(tool_names(&answer["tool_closed_ms"]), ["read_file", "write_file"], "the answer's row: {answer}");
    assert!(
        answer["tool_closed_ms"].as_array().is_some_and(|marks| marks.iter().all(|mark| mark["ms"].is_u64())),
        "each block's close is a time: {answer}"
    );
    assert!(after["tool_closed_ms"].is_null(), "an answer in words closed no tool block: {after}");
    assert_eq!(tool_names(&after["tool_ran_ms"]), ["read_file", "write_file"], "the next request's row: {after}");
    assert!(
        after["tool_ran_ms"].as_array().is_some_and(|marks| marks.iter().all(|mark| mark["ms"].is_u64())),
        "both tools ran: {after}"
    );
    let written: String = rows.iter().map(ToString::to_string).collect();
    assert!(!written.contains("a.rs") && !written.contains("fn b()"), "no tool input reaches a row: {written}");
}
