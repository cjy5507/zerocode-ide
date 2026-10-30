//! Scripted runtime regressions for admitted independent tool waves.
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use futures_util::future::BoxFuture;
use crate::message_stream::{BlockId, RenderBlock};
use crate::permission::{PermissionDecision, PermissionError, PermissionPrompter, PermissionRequest};
use super::{ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ConcurrentDispatchFn,
    ContentBlock, ConversationRuntime, MessageRole, PermissionMode, PermissionPolicy, RuntimeError,
    Session, StaticToolExecutor, TurnSummary, MAX_PARALLEL_SAFE_TOOL_DISPATCHES};

#[derive(Clone)]
struct BatchApi { names: Vec<&'static str> }
impl BatchApi {
    fn response(&self, request: &ApiRequest) -> Vec<AssistantEvent> {
        if request.messages.iter().any(|message| message.role == MessageRole::Tool) {
            return vec![AssistantEvent::TextDelta("done".into()), AssistantEvent::MessageStop];
        }
        let mut result: Vec<_> = self.names.iter().enumerate().map(|(index, name)| AssistantEvent::ToolUse {
            id: format!("tool-{index}"), name: (*name).into(), input: format!(r#"{{"path":"file-{index}.txt"}}"#),
        }).collect();
        result.push(AssistantEvent::MessageStop);
        result
    }
}
impl ApiClient for BatchApi {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> { Ok(self.response(&request)) }
}
impl AsyncApiClient for BatchApi {
    fn stream_async<'a>(&'a self, request: ApiRequest, _tx: tokio::sync::mpsc::Sender<RenderBlock>, _id: BlockId)
        -> BoxFuture<'a, Result<Vec<AssistantEvent>, RuntimeError>> {
        let result = self.response(&request);
        Box::pin(async move { Ok(result) })
    }
}
struct Allow;
impl PermissionPrompter for Allow {
    fn decide<'a>(&'a self, _request: PermissionRequest) -> BoxFuture<'a, Result<PermissionDecision, PermissionError>> {
        Box::pin(async { Ok(PermissionDecision::Allow) })
    }
}

fn run(streaming: bool, names: Vec<&'static str>, dispatch: ConcurrentDispatchFn, executor: StaticToolExecutor, cancel: crate::ToolCancelSignal) -> TurnSummary {
    let api = BatchApi { names };
    let mut runtime = ConversationRuntime::new(Session::new(), api.clone(), executor,
        PermissionPolicy::new(PermissionMode::DangerFullAccess), vec!["system".into()]);
    runtime.set_concurrent_dispatch(dispatch);
    runtime.set_tool_cancel_signal(cancel);
    if !streaming { return runtime.run_turn("inspect files", None).unwrap(); }
    runtime.set_async_api_client(Arc::new(api));
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap().block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let turn = runtime.run_turn_streaming_maybe_deep("inspect files", Vec::new(), tx, Arc::new(Allow));
        tokio::pin!(turn);
        loop {
            tokio::select! {
                result = &mut turn => return result.unwrap(),
                Some(_) = rx.recv() => {}
            }
        }
    })
}
fn index(input: &str) -> usize {
    let value: serde_json::Value = serde_json::from_str(input).unwrap();
    value["path"].as_str().unwrap().trim_start_matches("file-").trim_end_matches(".txt").parse().unwrap()
}
fn result_ids(summary: &TurnSummary) -> Vec<&str> {
    summary.tool_results.iter().flat_map(|message| &message.blocks).filter_map(|block| match block {
        ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()), _ => None,
    }).collect()
}
fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
    *gate.0.lock().unwrap() = true; gate.1.notify_all();
}

fn ninth_before_first(streaming: bool) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let first_started = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, received) = std::sync::mpsc::sync_channel(16);
    let observer_gate = gate.clone();
    let observer = std::thread::spawn(move || {
        let mut observed = false;
        // A liveness deadline releases the held tool on the old implementation.
        // The assertion itself is event ordering, never elapsed milliseconds.
        while let Ok(index) = received.recv_timeout(Duration::from_secs(10)) {
            if index == 8 { observed = true; break; }
        }
        release(&observer_gate);
        observed
    });
    let active = Arc::new(AtomicUsize::new(0)); let maximum = Arc::new(AtomicUsize::new(0));
    let dispatch_active=active.clone(); let dispatch_maximum=maximum.clone(); let dispatch_gate=gate.clone();
    let dispatch: ConcurrentDispatchFn = Arc::new(move |_, input| {
        let index=index(input);
        if index == 0 { release(&first_started); }
        else { let mut ready=first_started.0.lock().unwrap(); while !*ready { ready=first_started.1.wait(ready).unwrap(); } }
        let current=dispatch_active.fetch_add(1,Ordering::SeqCst)+1; dispatch_maximum.fetch_max(current,Ordering::SeqCst);
        let _=started.send(index);
        if index == 0 { let mut ready=dispatch_gate.0.lock().unwrap(); while !*ready { ready=dispatch_gate.1.wait(ready).unwrap(); } }
        dispatch_active.fetch_sub(1,Ordering::SeqCst);
        Ok(format!("result-{index}"))
    });
    let summary=run(streaming,vec!["read_file";9],dispatch,StaticToolExecutor::new().register("read_file",|_|panic!("independent reads must use concurrent dispatch")),crate::ToolCancelSignal::new());
    assert!(observer.join().unwrap(),"item 9 must start before the held item 1 ends");
    assert!(maximum.load(Ordering::SeqCst)<=MAX_PARALLEL_SAFE_TOOL_DISPATCHES);
    assert_eq!(active.load(Ordering::SeqCst),0);
    assert_eq!(result_ids(&summary),(0..9).map(|index|format!("tool-{index}")).collect::<Vec<_>>());
}

fn queued_cancelled(streaming: bool) {
    let gate=Arc::new((Mutex::new(false),Condvar::new()));
    let cancel=crate::ToolCancelSignal::new(); let observer_cancel=cancel.clone();
    let (started,received)=std::sync::mpsc::sync_channel(16);
    let observer_gate=gate.clone();
    let observer=std::thread::spawn(move || {
        for _ in 0..MAX_PARALLEL_SAFE_TOOL_DISPATCHES { received.recv_timeout(Duration::from_secs(10)).unwrap(); }
        observer_cancel.cancel_running_tools(); release(&observer_gate);
    });
    let count=Arc::new(AtomicUsize::new(0)); let dispatch_count=count.clone();
    let dispatch:ConcurrentDispatchFn=Arc::new(move |_,input| {
        dispatch_count.fetch_add(1,Ordering::SeqCst); let _=started.send(());
        let mut ready=gate.0.lock().unwrap(); while !*ready { ready=gate.1.wait(ready).unwrap(); }
        Ok(format!("result-{}",index(input)))
    });
    let summary=run(streaming,vec!["read_file";9],dispatch,StaticToolExecutor::new().register("read_file",|_|panic!("read bypassed queue")),cancel);
    observer.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst),8,"a queued call must never start after cancellation");
    assert_eq!(result_ids(&summary),(0..9).map(|index|format!("tool-{index}")).collect::<Vec<_>>());
    let queued=summary.tool_results.last().unwrap();
    assert!(queued.blocks.iter().any(|block|matches!(block,ContentBlock::ToolResult{output,is_error:true,..} if output.contains(crate::tool_cancel::CANCELLED_TOOL_RESULT))),"the queued tool needs a paired cancellation receipt");
}

fn mutation_barrier(streaming: bool) {
    let active=Arc::new(AtomicUsize::new(0)); let reads=Arc::new(AtomicUsize::new(0)); let writes=Arc::new(AtomicUsize::new(0));
    let dispatch_active=active.clone(); let dispatch_reads=reads.clone(); let dispatch_writes=writes.clone();
    let dispatch:ConcurrentDispatchFn=Arc::new(move |name,input| {
        assert_eq!(name,"read_file"); let index=index(input);
        if index>9 { assert_eq!(dispatch_writes.load(Ordering::SeqCst),1,"later reads crossed the mutation barrier"); }
        dispatch_active.fetch_add(1,Ordering::SeqCst); dispatch_reads.fetch_add(1,Ordering::SeqCst);
        dispatch_active.fetch_sub(1,Ordering::SeqCst); Ok(format!("result-{index}"))
    });
    let executor=StaticToolExecutor::new().register("read_file",|_|panic!("read bypassed dispatch")).register("write_file",move |_| {
        assert_eq!(active.load(Ordering::SeqCst),0); assert_eq!(reads.load(Ordering::SeqCst),9);
        writes.fetch_add(1,Ordering::SeqCst); Ok("write-result".into())
    });
    let mut names=vec!["read_file";9]; names.extend(["write_file","read_file","read_file"]);
    let summary=run(streaming,names,dispatch,executor,crate::ToolCancelSignal::new());
    assert_eq!(result_ids(&summary),(0..12).map(|index|format!("tool-{index}")).collect::<Vec<_>>());
}

#[test] fn work_conserving_sync_admits_the_ninth_read_before_the_first_finishes() { ninth_before_first(false); }
#[test] fn work_conserving_stream_admits_the_ninth_read_before_the_first_finishes() { ninth_before_first(true); }
#[test] fn work_conserving_sync_pairs_queued_cancellation_results() { queued_cancelled(false); }
#[test] fn work_conserving_stream_pairs_queued_cancellation_results() { queued_cancelled(true); }
#[test] fn work_conserving_sync_preserves_mutation_barriers() { mutation_barrier(false); }
#[test] fn work_conserving_stream_preserves_mutation_barriers() { mutation_barrier(true); }
