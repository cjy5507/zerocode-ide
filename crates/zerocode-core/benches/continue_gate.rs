//! What the continue gate costs, measured (t-26583).
//!
//! Three costs, each paid by a different part of the window, each on a path a
//! slow machine feels if it is wrong:
//!
//! - **one hook event**, `note_activity` — paid by the hook road for every tool
//!   call of every pane, on the task that also forwards the event to the window;
//! - **one beat's judgment**, `judge` — paid once a second per live worker;
//! - **one look at a transcript**, `feed` — paid at most every three seconds per
//!   worker, on the bytes the file grew by. The chunk here is what a transcript
//!   mostly is: model calls a kilobyte long, each followed by a tool result of
//!   many kilobytes that no model call ever mentions.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use zerocode_core::continue_gate::spend::{ClaudeFormat, CostReader};
use zerocode_core::continue_gate::{Allowance, Cap, StepBook};
use zerocode_core::hook::{Activity, Phase, Tool};

fn started() -> Activity {
    Activity {
        verb: Tool::Bash,
        target: Some("cargo test -p zerocode-core".into()),
        phase: Phase::Started,
        reads: Vec::new(),
        writes: Vec::new(),
        vcs: Vec::new(),
        cwd: None,
    }
}

/// A tool event as a hook delivers it: a name, a command, and the context a
/// vendor adds beside them.
fn tool_payload(index: usize) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "b3b2a5f0-6c1d-4e58-9a3b-2f6d8c4e1a07",
        "transcript_path": "/home/dev/.claude/projects/repo/session.jsonl",
        "cwd": "/home/dev/repo",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("cargo test -p zerocode-core --lib -- case_{index}"),
            "description": "Run the core tests",
        },
    })
    .to_string()
}

/// A worker's day, as the book holds it: a full window and a full ring of costs.
fn a_busy_book() -> StepBook {
    let mut book = StepBook::default();
    let activity = started();
    for index in 0..200 {
        book.note_activity(&activity, &tool_payload(index));
        book.note_cost(Some(0.15 + (index % 7) as f64 * 0.01));
    }
    book
}

/// `calls` model calls of about a kilobyte, each followed by a tool result of
/// `result_bytes` of file contents.
fn transcript_chunk(calls: usize, result_bytes: usize) -> String {
    let words = "fn main() { println!(\"hello\"); } ".repeat(result_bytes / 34 + 1);
    let mut chunk = String::new();
    for index in 0..calls {
        chunk.push_str(
            &serde_json::json!({
                "type": "assistant",
                "sessionId": "session-1",
                "timestamp": "2026-10-03T00:00:00.000Z",
                "uuid": format!("uuid-{index}"),
                "requestId": format!("request-{index}"),
                "message": {
                    "id": format!("message-{index}"),
                    "model": "claude-haiku-4-5",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Reading the file before I change it."}],
                    "usage": {
                        "input_tokens": 3,
                        "output_tokens": 120 + index,
                        "cache_read_input_tokens": 90_000,
                        "cache_creation_input_tokens": 400,
                    },
                },
            })
            .to_string(),
        );
        chunk.push('\n');
        chunk.push_str(
            &serde_json::json!({
                "type": "user",
                "sessionId": "session-1",
                "timestamp": "2026-10-03T00:00:01.000Z",
                "uuid": format!("result-{index}"),
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": format!("toolu_{index}"),
                        "content": words[..result_bytes.min(words.len())],
                    }],
                },
            })
            .to_string(),
        );
        chunk.push('\n');
    }
    chunk
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("continue_gate");

    let activity = started();
    let payload = tool_payload(7);
    group.bench_function("note_activity_one_tool_call", |b| {
        let mut book = StepBook::default();
        b.iter(|| book.note_activity(black_box(&activity), black_box(&payload)));
    });

    let book = a_busy_book();
    let allowance = Allowance {
        task: Some(Cap {
            limit_usd: 200.0,
            spent_usd: 60.0,
            ahead_usd: book.ahead_usd(),
        }),
        day: Some(Cap {
            limit_usd: 800.0,
            spent_usd: 240.0,
            ahead_usd: 5.0,
        }),
    };
    group.bench_function("judge_one_worker_one_beat", |b| {
        b.iter(|| black_box(book.judge(black_box(&allowance), black_box(1_800_000_000_000))));
    });

    // A look at a transcript that grew by thirty calls, each with a result of
    // eight kilobytes: about a quarter of a mebibyte, three seconds of work.
    let chunk = transcript_chunk(30, 8_192);
    group.bench_function("feed_a_look_at_a_growing_transcript", |b| {
        b.iter(|| {
            let mut reader = ClaudeFormat::default();
            black_box(reader.feed(black_box(&chunk)))
        });
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
