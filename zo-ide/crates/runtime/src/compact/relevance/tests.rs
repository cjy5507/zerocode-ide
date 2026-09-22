//! The pure half, held to its contract: which blocks are asked about, what
//! leaves the machine, how a reply is read, and what a drop does to the plan
//! and to the vault. No wire anywhere — the seat is a closure.

use std::collections::BTreeMap;

use api::{SystemOneResponse, SystemOneUsage};
use serde_json::json;
use zerocode_core::jev::{
    rubric_fingerprint, COMPACTION_BLOCK_HEAD_BYTE_CAP, COMPACTION_DROP, COMPACTION_KEEP,
    COMPACTION_SHARD_TARGET, CUT_MARK,
};

use super::*;
use crate::compact::{apply_compaction, prepare_compaction, CompactionConfig};

/// The smallest body a question is spent on, as the tests ask it — the
/// runtime hands its own trim floor in at the seam.
const MIN_BYTES: usize = 240;

fn user_text(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text { text: text.to_string() }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

fn call(id: &str, tool: &str, input: &str) -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: id.to_string(),
        name: tool.to_string(),
        input: input.to_string(),
    }])
}

fn result(id: &str, tool: &str, output: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            tool_name: tool.to_string(),
            output: output.to_string(),
            is_error: false,
            images: Vec::new(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

fn big(tag: &str) -> String {
    format!("{tag} {}", "x".repeat(600))
}

/// A session whose compaction set holds one droppable read, one edit, one
/// small result and one placeholder, with a goal and an answer in the tail.
fn session_with_four_results() -> Session {
    let mut session = Session::new();
    for message in [
        user_text("please make the tests pass in src/lib.rs"),
        call("t1", "Read", r#"{"path":"/work/src/lib.rs"}"#),
        result("t1", "Read", &big("READ_BODY")),
        call("t2", "edit_file", r#"{"path":"/work/src/lib.rs"}"#),
        result("t2", "edit_file", &big("EDIT_DIFF")),
        call("t3", "Bash", r#"{"command":"ls"}"#),
        result("t3", "Bash", "a\nb\n"),
        call("t4", "Grep", r#"{"pattern":"fn"}"#),
        result("t4", "Grep", MICROCOMPACT_PLACEHOLDER),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "the tests now pass; wrapping up".to_string(),
        }]),
        user_text("great, now fix the warning too"),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "looking at the warning".to_string(),
        }]),
    ] {
        session.push_message(message).expect("push");
    }
    session
}

fn plan_of(session: &Session) -> CompactionPlan {
    prepare_compaction(
        session,
        CompactionConfig {
            preserve_recent_messages: 2,
            max_estimated_tokens: 0,
        },
    )
    .expect("a session over budget yields a plan")
}

#[test]
fn only_a_readable_body_past_the_floor_is_a_candidate_and_it_carries_its_calls_input() {
    let session = session_with_four_results();
    let plan = plan_of(&session);
    let found = candidates(&plan, MIN_BYTES);
    assert_eq!(found.len(), 1, "{found:?}");
    let block = &found[0];
    assert_eq!((block.position, block.tool_use_id.as_str(), block.tool_name.as_str()), (0, "t1", "Read"));
    assert_eq!(block.input, r#"{"path":"/work/src/lib.rs"}"#, "the call travels whole");
    assert_eq!(block.output_bytes, big("READ_BODY").len());
    assert!(block.output_head.starts_with("READ_BODY"));
    assert!(block.output_head.len() <= COMPACTION_BLOCK_HEAD_BYTE_CAP + CUT_MARK.len());
    assert_eq!((block.message_index, block.block_index), (2, 0));
    assert_eq!(block.question_id(), "b0");
}

#[test]
fn the_ask_reads_the_goal_and_the_newest_words_off_the_whole_session() {
    let session = session_with_four_results();
    let plan = plan_of(&session);
    let ask = ask_for(&session, &plan, MIN_BYTES, "s@1").expect("something to ask");
    assert_eq!(ask.goal, "great, now fix the warning too", "the goal is the present, tail included");
    assert_eq!(ask.recent, "looking at the warning");
    assert_eq!(ask.attempt, "s@1");
    assert_eq!(ask.blocks.len(), 1);

    // Nothing to ask about is no ask at all: neither a question nor a row.
    let mut bare = Session::new();
    for message in [
        user_text("hello"),
        result("t9", "Bash", "short"),
        ConversationMessage::assistant(vec![ContentBlock::Text { text: "hi".to_string() }]),
        user_text("more"),
        user_text("and more"),
    ] {
        bare.push_message(message).expect("push");
    }
    let plan = plan_of(&bare);
    assert_eq!(ask_for(&bare, &plan, MIN_BYTES, "s@1"), None);
}

#[test]
fn the_state_carries_heads_and_never_a_body() {
    let session = session_with_four_results();
    let plan = plan_of(&session);
    let ask = ask_for(&session, &plan, MIN_BYTES, "s@1").expect("ask");
    let state = state(&ask, &ask.blocks);
    assert_eq!(state["goal"], "great, now fix the warning too");
    assert_eq!(state["recent"], "looking at the warning");
    assert_eq!(state["blocks"][0]["tool"], "Read");
    assert_eq!(state["blocks"][0]["input"], r#"{"path":"/work/src/lib.rs"}"#);
    let head = state["blocks"][0]["head"].as_str().expect("a head");
    assert!(head.len() <= COMPACTION_BLOCK_HEAD_BYTE_CAP + CUT_MARK.len());
    assert!(!state.to_string().contains(&big("READ_BODY")), "the body never leaves");
}

#[test]
fn questions_are_closed_choices_named_by_the_blocks_place_in_its_shard() {
    let blocks: Vec<BlockHead> = (0..3)
        .map(|position| BlockHead {
            position,
            message_index: position,
            block_index: 0,
            tool_use_id: format!("t{position}"),
            tool_name: "Read".to_string(),
            input: String::new(),
            output_head: "head".to_string(),
            output_bytes: 1_000,
        })
        .collect();
    let asked = questions(&blocks[1..]);
    assert_eq!(asked.keys().cloned().collect::<Vec<_>>(), ["b1", "b2"]);
    let body = serde_json::to_value(&asked["b2"]).expect("a question serializes");
    assert_eq!(body["type"], "choice");
    assert!(
        body["instructions"].as_str().expect("words").ends_with("`blocks[1]`?"),
        "the second block of this shard is blocks[1] of its state: {body}"
    );
    assert_eq!(body["criteria"][COMPACTION_KEEP], KEEP_MEANS);
    assert_eq!(body["criteria"][COMPACTION_DROP], DROP_MEANS);

    let many: Vec<BlockHead> = (0..=COMPACTION_SHARD_TARGET)
        .map(|position| blocks[0].clone_at(position))
        .collect();
    let cut = shards(&many);
    assert_eq!(cut.len(), 2, "one over the target is two shards");
    assert_eq!(cut[0].len() + cut[1].len(), COMPACTION_SHARD_TARGET + 1);
}

impl BlockHead {
    fn clone_at(&self, position: usize) -> Self {
        Self { position, ..self.clone() }
    }
}

/// The rubric's words are pinned: a changed sentence is a red test here, not
/// a quiet drift the ledger's `rubricVersion` never noticed.
#[test]
fn the_rubric_is_pinned_to_its_version() {
    assert_eq!(COMPACTION_RUBRIC_VERSION, 1);
    assert_eq!(rubric_fingerprint(rubric_words), "426c79112f852c13");
}

fn answer(chosen: &str, drop: f64, confidence: f64) -> serde_json::Value {
    json!({
        "type": "choice",
        "choice": chosen,
        "probabilities": { COMPACTION_KEEP: 1.0 - drop, COMPACTION_DROP: drop },
        "confidence": confidence,
    })
}

fn response(answers: BTreeMap<String, serde_json::Value>) -> SystemOneResponse {
    SystemOneResponse {
        model: "jev-test".to_string(),
        answers,
        usage: SystemOneUsage { input_tokens: 100, output_tokens: 1 },
    }
}

#[test]
fn validate_reads_every_answer_and_refuses_a_stray_or_a_broken_one() {
    let session = session_with_four_results();
    let plan = plan_of(&session);
    let blocks = candidates(&plan, MIN_BYTES);

    let read = validate(&blocks, &response(BTreeMap::from([("b0".to_string(), answer(COMPACTION_DROP, 0.8, 0.9))])))
        .expect("a reply that checks out");
    assert_eq!(read.len(), 1);
    assert_eq!((read[0].position, read[0].chosen.as_str()), (0, COMPACTION_DROP));
    assert!((read[0].drop_probability - 0.8).abs() < 1e-9);

    let stray = validate(
        &blocks,
        &response(BTreeMap::from([
            ("b0".to_string(), answer(COMPACTION_KEEP, 0.1, 0.9)),
            ("b7".to_string(), answer(COMPACTION_KEEP, 0.1, 0.9)),
        ])),
    )
    .expect_err("an answer nobody asked for");
    assert_eq!((stray.rule(), stray.position()), ("unknown_answer", None));

    let missing = validate(&blocks, &response(BTreeMap::new())).expect_err("no answer");
    assert_eq!((missing.rule(), missing.position()), ("schema_no_answer", Some(0)));

    let alien = validate(
        &blocks,
        &response(BTreeMap::from([("b0".to_string(), json!({"type": "choice", "choice": "maybe", "probabilities": {}, "confidence": 0.5}))])),
    )
    .expect_err("an option nobody offered");
    assert_eq!(alien.rule(), "schema_unknown_option");
}

#[test]
fn a_reading_drops_only_past_the_lean_and_only_when_it_chose_drop() {
    let reading = |chosen: &str, drop: f64| BlockReading {
        position: 0,
        chosen: chosen.to_string(),
        drop_probability: drop,
        confidence: 0.9,
    };
    assert!(reading(COMPACTION_DROP, 0.70).drops());
    assert!(reading(COMPACTION_DROP, 0.99).drops());
    assert!(!reading(COMPACTION_DROP, 0.69).drops(), "under the lean, a drop is a keep");
    assert!(!reading(COMPACTION_KEEP, 0.90).drops(), "the chosen word rules, whatever the number");
}

/// A drop seals the original to the vault, blanks the SUMMARY's copy, and
/// the eviction seal heals it back: the vault never holds the placeholder,
/// and the compacted session reads the same as one compacted without a seat.
#[test]
fn a_drop_seals_the_original_blanks_the_summary_copy_and_the_eviction_seal_heals_it() {
    let _env = crate::test_env_lock();
    let dir = tempfile::tempdir().expect("a session dir");
    let path = dir.path().join("sess.jsonl");
    let mut session = Session::new().with_persistence_path(path);
    for message in session_with_four_results().messages.iter() {
        session.push_message(message.clone()).expect("push");
    }
    let mut plan = plan_of(&session);
    let blocks = candidates(&plan, MIN_BYTES);

    assert_eq!(drop_blocks(&mut plan, &session, &blocks, &[0]), 1);
    let ContentBlock::ToolResult { output, .. } = &plan.messages_to_compact[2].blocks[0] else {
        panic!("the read's result");
    };
    assert_eq!(output, MICROCOMPACT_PLACEHOLDER, "the summary reads a placeholder");
    assert_eq!(
        summary_input_tokens(&plan.messages_to_compact) + big("READ_BODY").len() / 4,
        summary_input_tokens(&plan_of(&session).messages_to_compact) + MICROCOMPACT_PLACEHOLDER.len() / 4,
        "what the summary no longer reads is the body's own tokens"
    );
    let sealed = session.read_vault();
    assert!(
        sealed.iter().any(|record| record.message.blocks.iter().any(
            |block| matches!(block, ContentBlock::ToolResult { output, .. } if *output == big("READ_BODY"))
        )),
        "the original was sealed before the copy was blanked"
    );

    let result = apply_compaction(plan, "<summary>\n1. Primary Request and Intent: tests\n</summary>");
    let vault = result.compacted_session.read_vault();
    assert!(
        vault.iter().any(|record| record.message.blocks.iter().any(
            |block| matches!(block, ContentBlock::ToolResult { output, .. } if *output == big("READ_BODY"))
        )),
        "the evicted record carries the original"
    );
    // The Grep result was a placeholder BEFORE any of this (the fixture's
    // own), and is sealed as the placeholder it is; the read's never is.
    assert!(
        !vault.iter().any(|record| record.message.blocks.iter().any(|block| matches!(
            block,
            ContentBlock::ToolResult { tool_use_id, output, .. }
                if tool_use_id == "t1" && output.as_str() == MICROCOMPACT_PLACEHOLDER
        ))),
        "the dropped block's placeholder was never sealed"
    );

    // Dropping a position nobody was asked about, or one already dropped,
    // touches nothing.
    let mut again = plan_of(&session);
    assert_eq!(drop_blocks(&mut again, &session, &blocks, &[7]), 0);
    assert_eq!(again.messages_to_compact, plan_of(&session).messages_to_compact);
}

#[test]
fn a_seal_that_does_not_land_drops_nothing() {
    let _env = crate::test_env_lock();
    // A persistence path under a FILE, so the vault beside it cannot be
    // opened: the append fails, and the bodies in hand are the only copies.
    let dir = tempfile::tempdir().expect("a dir");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("a file in the way");
    // Bound to the blocked path only once the messages are in, so the pushes
    // themselves persist nothing and only the seal meets the wall.
    let session = session_with_four_results().with_persistence_path(blocker.join("sess.jsonl"));
    let before = plan_of(&session);
    let mut plan = before.clone();
    let blocks = candidates(&plan, MIN_BYTES);
    assert_eq!(drop_blocks(&mut plan, &session, &blocks, &[0]), 0);
    assert_eq!(plan.messages_to_compact, before.messages_to_compact, "the plan is byte-identical");
}

/// A seat that only says what it would drop.
struct Says(CompactionJudgment);

impl CompactionSeat for Says {
    fn judge<'a>(&'a self, _ask: &'a CompactionAsk) -> BoxFuture<'a, CompactionJudgment> {
        Box::pin(async move { self.0.clone() })
    }
}

/// A seat that never answers inside any wall — what a timeout looks like
/// from the runtime's side is the seat's own default answer, and the plan
/// the summary reads is the plan `prepare_compaction` made.
struct Silent;

impl CompactionSeat for Silent {
    fn judge<'a>(&'a self, _ask: &'a CompactionAsk) -> BoxFuture<'a, CompactionJudgment> {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            CompactionJudgment::default()
        })
    }
}

#[tokio::test]
async fn a_recording_seat_a_silent_seat_and_an_empty_answer_leave_the_plan_byte_identical() {
    let session = session_with_four_results();
    let before = plan_of(&session);
    for seat in [
        Box::new(Says(CompactionJudgment { dropped: vec![0], applies: false })) as Box<dyn CompactionSeat>,
        Box::new(Says(CompactionJudgment { dropped: vec![], applies: true })),
        Box::new(Silent),
    ] {
        let (after, judged) = judge_plan(seat.as_ref(), &session, before.clone(), MIN_BYTES, "s@1").await;
        assert_eq!(after.messages_to_compact, before.messages_to_compact);
        assert_eq!(after.preserved_tail, before.preserved_tail);
        assert_eq!(judged, Judged { asked: 1, dropped: 0 });
    }
}

#[tokio::test]
async fn an_acting_seat_takes_the_dropped_block_out_of_what_the_summary_reads() {
    let session = session_with_four_results();
    let before = plan_of(&session);
    let seat = Says(CompactionJudgment { dropped: vec![0], applies: true });
    let (after, judged) = judge_plan(&seat, &session, before.clone(), MIN_BYTES, "s@1").await;
    assert_eq!(judged, Judged { asked: 1, dropped: 1 });
    let ContentBlock::ToolResult { output, .. } = &after.messages_to_compact[2].blocks[0] else {
        panic!("the read's result");
    };
    assert_eq!(output, MICROCOMPACT_PLACEHOLDER);
    assert_eq!(after.preserved_tail, before.preserved_tail, "the tail is never touched");
    // The edit's diff was never a candidate, so it is exactly where it was.
    assert_eq!(after.messages_to_compact[4], before.messages_to_compact[4]);
}
