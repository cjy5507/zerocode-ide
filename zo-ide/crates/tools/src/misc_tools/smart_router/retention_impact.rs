//! Paired compaction-boundary replay through the production runtime and recall
//! tool. Model outputs are fixed; provider usage and model wait are unmeasured.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use runtime::{ApiClient, ApiRequest, AssistantEvent, CompactionConfig, ContentBlock, ConversationMessage,
    ConversationRuntime, PermissionMode, PermissionPolicy, RuntimeError, Session, StaticToolExecutor};
use serde::Serialize;
use serde_json::{json, Value};
use telemetry::{MemoryTelemetrySink, SessionTracer, TelemetryEvent};
use zerocode_core::jev::COMPACTION;

use super::compaction_seat::CompactionJudge;
use super::jev_mock::{machine, Mock};
use super::super::session_recall::{run_session_recall, SessionRecallInput};
use crate::ToolContext;

#[derive(Default)]
struct Calls {
    previous: Vec<u8>,
    summaries: Vec<Value>,
    probes: Vec<Value>,
}

fn prefix(a: &[u8], b: &[u8]) -> usize { a.iter().zip(b).take_while(|(a,b)| a == b).count() }

#[derive(Serialize)]
struct RequestShape<'a> { system: &'a [String], messages: &'a [api::InputMessage] }

fn words(message: &ConversationMessage) -> impl Iterator<Item = &str> {
    message.blocks.iter().filter_map(|block| match block {
        ContentBlock::Text { text } => Some(text.as_str()),
        ContentBlock::ToolResult { output, .. } => Some(output.as_str()),
        _ => None,
    })
}

struct ReplayClient(Arc<Mutex<Calls>>);
impl ApiClient for ReplayClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let summary = request.system_prompt.iter().any(|text| text.contains("You are summarizing a coding conversation"))
            || request.messages.iter().flat_map(words).any(|text| text.contains("You are summarizing a coding conversation"));
        let mut messages = runtime::convert_messages(&request.messages);
        runtime::append_wire_reminders(&mut messages, &request.wire_reminders);
        let bytes = serde_json::to_vec(&RequestShape { system: &request.system_prompt, messages: &messages }).unwrap();
        let mut calls = self.0.lock().unwrap();
        let row = json!({"bytes":bytes.len(),"estimatedTokens":bytes.len().div_ceil(4),
            "samePrefixBytes":prefix(&calls.previous,&bytes),"providerReportedUsage":null});
        if summary { calls.summaries.push(row); }
        else { calls.probes.push(row); calls.previous=bytes; }
        drop(calls);
        let answer = if summary {
            let facts: BTreeSet<String> = request.messages.iter().flat_map(words).flat_map(str::lines)
                .map(str::trim).filter(|line| ["USER_RULE_","OLD_FACT_","TOOL_FACT_","RECENT_FACT_"]
                    .iter().any(|prefix| line.starts_with(prefix))).map(str::to_string).collect();
            format!("<summary>\n{}\n</summary>", facts.into_iter().collect::<Vec<_>>().join("\n"))
        } else { "Fixture probe complete.".to_string() };
        Ok(vec![AssistantEvent::TextDelta(answer), AssistantEvent::MessageStop])
    }
}

fn pairs_are_valid(session: &Session) -> bool {
    let mut calls = BTreeSet::new();
    let mut results = BTreeSet::new();
    for message in session.messages.iter() {
        for block in &message.blocks {
            match block {
                ContentBlock::ToolUse { id, .. } => { calls.insert(id.as_str()); },
                ContentBlock::ToolResult { tool_use_id, .. } => { results.insert(tool_use_id.as_str()); },
                _ => {},
            }
        }
    }
    calls==results
}

fn one_session(root: &Path, scenario: &str, phase: &str, jev: &Mutex<Vec<usize>>) -> Value {
    let cwd = root.join(phase); std::fs::create_dir_all(cwd.join(".zo/sessions")).unwrap();
    let path = cwd.join(".zo/sessions/impact.jsonl");
    let mut session = Session::new(); session.session_id="impact".into();
    if scenario!="no_store" { session=session.with_persistence_path(&path); }
    session.push_user_text("USER_RULE_KEEP_NAMES: Preserve identifiers exactly.").unwrap();
    session.push_message(ConversationMessage::assistant(vec![ContentBlock::Text {
        text:"OLD_FACT_ALPHA: The previous build used cargo test.".into() }])).unwrap();
    let calls=Arc::new(Mutex::new(Calls::default()));
    let sink=Arc::new(MemoryTelemetrySink::default());
    let features=runtime::RuntimeFeatureConfig::default().with_auto_dream_enabled(false);
    let mut runtime=ConversationRuntime::new_with_features(session,ReplayClient(Arc::clone(&calls)),StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),vec!["Synthetic impact fixture; preserve supplied facts.".into()],&features)
        .with_session_tracer(SessionTracer::new("impact",sink.clone()));
    runtime.set_workspace_cwd(cwd.clone());
    runtime.set_context_window(1_000_000);
    runtime=runtime.with_auto_compaction_input_tokens_threshold(if scenario=="headroom" {500} else {500_000});
    runtime.set_auto_compaction_enabled(false);
    runtime.set_compaction_seat(Some(Arc::new(CompactionJudge::at(root))));
    let ctx=ToolContext::new().with_cwd(cwd.clone());ctx.set_session_id("impact");
    let start_requests=jev.lock().unwrap().len();
    let rounds=if scenario=="normal" {6} else {1};
    let mut results=Vec::new();
    for round in 0..rounds {
        let marker=format!("TOOL_FACT_{round}");let id=format!("read-{round}");
        runtime.session_mut().push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id:id.clone(),name:"Read".into(),input:format!("{{\"path\":\"src/resolved-{round}.rs\"}}") }])).unwrap();
        let output=format!("{marker}: resolved output remains recoverable.\n{}",
            "resolved output details\n".repeat(if scenario=="payback" {60} else {2_000}));
        let mut result=ConversationMessage::tool_result(&id,"Read",output,false);
        if scenario=="image" {
            if let ContentBlock::ToolResult { images, .. }=&mut result.blocks[0] {
                images.push(("image/png".into(),"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6oQAAAABJRU5ErkJggg==".into()));
            }
        }
        runtime.session_mut().push_message(result).unwrap();
        if matches!(scenario,"payback"|"headroom") {
            runtime.session_mut().push_user_text("UNCHANGED_LARGE_CONTEXT ".repeat(2_000)).unwrap();
        }
        runtime.session_mut().push_user_text(format!("RECENT_FACT_{round}: Continue the remaining fixture task.")).unwrap();
        runtime.run_turn("Prepare the next fixture boundary.",None).unwrap();
        let before=runtime.estimated_tokens();let summaries_before=calls.lock().unwrap().summaries.len();
        let workflow=Instant::now();
        let began=Instant::now();
        let result=runtime.compact(CompactionConfig {preserve_recent_messages:2,max_estimated_tokens:0},
            (scenario=="focus").then_some("preserve the explicit fixture facts"));
        let cleared=result.cleared_tool_results;let removed=result.removed_message_count;
        runtime.apply_manual_compaction(result);
        let boundary_us=began.elapsed().as_micros();
        let after_boundary=runtime.estimated_tokens();
        let context=runtime.session().messages.iter().flat_map(words).collect::<Vec<_>>().join("\n");
        let facts=json!({"userInstruction":context.contains("USER_RULE_KEEP_NAMES"),"oldDialogue":context.contains("OLD_FACT_ALPHA"),
            "toolResult":context.contains(&marker),"latestTurn":context.contains(&format!("RECENT_FACT_{round}"))});
        let recovery=Instant::now();
        let mut recall_bytes=0;let mut recall_calls=0;let mut recovered=facts["toolResult"]==true;
        if !recovered && scenario!="no_store" {
            let recalled=run_session_recall(SessionRecallInput {session_ref:Some("current".into()),query:Some(marker.clone()),
                include_tool_results:Some(true),..Default::default()},&ctx).unwrap();
            recall_calls=1;recall_bytes=recalled.len();recovered=recalled.contains(&marker);
            runtime.session_mut().push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id:format!("recall-{round}"),name:"session_recall".into(),input:format!("{{\"query\":\"{marker}\"}}") }])).unwrap();
            runtime.session_mut().push_message(ConversationMessage::tool_result(format!("recall-{round}"),"session_recall",recalled,false)).unwrap();
        }
        let recovery_us=recovery.elapsed().as_micros();
        let after=runtime.estimated_tokens();
        let resumed=scenario=="no_store" || Session::load_from_path(&path).unwrap().messages==runtime.session().messages;
        let pairs=pairs_are_valid(runtime.session());
        runtime.run_turn("Use the retained fixture facts to continue.",None).unwrap();
        let counts=calls.lock().unwrap();
        let eligibility=sink.events().into_iter().filter_map(|event| match event {
            TelemetryEvent::SessionTrace(trace) if trace.attributes.get("action").and_then(Value::as_str)==Some("retention_eligibility") => Some(json!(trace.attributes)),
            _=>None,
        }).next_back();
        results.push(json!({"round":round,"cleared":cleared,"removed":removed,"retained":cleared>0,
            "historyEstimatedTokensBefore":before,"historyEstimatedTokensAfterBoundary":after_boundary,
            "historyEstimatedTokensAfterRecovery":after,"recoveryHostMicros":recovery_us,
            "workflowHostMicros":workflow.elapsed().as_micros(),
            "boundaryHostMicros":boundary_us,"summaryRequests":counts.summaries.len()-summaries_before,
            "summaryWire":counts.summaries.get(summaries_before),"nextWire":counts.probes.last(),"eligibility":eligibility,
            "factsLive":facts,"toolFactRecovered":recovered,"recallCalls":recall_calls,"recallBytes":recall_bytes,
            "coldResumeEqual":resumed,"toolPairsValid":pairs}));
    }
    let recorded=jev.lock().unwrap();let requests=&recorded[start_requests..];
    let counts=calls.lock().unwrap();
    json!({"scenario":scenario,"phase":phase,"rounds":results,"jevRequests":requests.len(),
        "jevRequestBytes":requests.iter().sum::<usize>(),"summaryRequests":counts.summaries.len(),
        "probeRequests":counts.probes.len(),"providerReportedUsage":null,"modelWaitMicros":null,
        "summaryRequestBytes":counts.summaries.iter().map(|row| row["bytes"].as_u64().unwrap()).sum::<u64>(),
        "probeRequestBytes":counts.probes.iter().map(|row| row["bytes"].as_u64().unwrap()).sum::<u64>(),
        "requestShape":"normalized ApiRequest envelope; not billed tokens or guaranteed provider cache hits",
        "modelOutput":"fixed faithful summary and closed Jev replies; not model accuracy"})
}

#[test]
#[ignore = "explicit fixed-response compaction workflow measurement; no real model or user state"]
fn measure_retention_workflow_impact() {
    let mode=std::env::var("JEV_IMPACT_MODE").unwrap();assert!(["off","shadow","on"].contains(&mode.as_str()));
    let scenario=std::env::var("JEV_IMPACT_SCENARIO").unwrap();
    assert!(["normal","focus","no_store","headroom","image","payback"].contains(&scenario.as_str()));
    let phase=std::env::var("JEV_IMPACT_PHASE").unwrap();assert!(["cold","warm"].contains(&phase.as_str()));
    let requests=Arc::new(Mutex::new(Vec::new()));let recorder=Arc::clone(&requests);
    let endpoint=Mock::answering(move |body| {
        recorder.lock().unwrap().push(body.len());let request:Value=serde_json::from_str(body).unwrap();
        let answers:serde_json::Map<String,Value>=request["questions"].as_object().unwrap().keys().map(|id|
            (id.clone(),json!({"type":"choice","choice":"drop","confidence":1.0,"probabilities":{"keep":0.0,"drop":1.0}}))).collect();
        (200,json!({"model":"jev-fixture-replay","answers":answers,"usage":{"input_tokens":0,"output_tokens":0}}).to_string())
    });
    machine(&COMPACTION,&mode,&endpoint.base_url,|cwd| {
        if phase=="warm" {let _=one_session(cwd,&scenario,"warmup",&requests);}
        let mut measured=one_session(cwd,&scenario,&phase,&requests);measured["mode"]=json!(mode);
        println!("JEV_IMPACT_JSON={measured}");
    });
}
