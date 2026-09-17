//! The per-event payload readers, measured.
//!
//! One hook envelope used to be parsed by every reader privately — the
//! shell's `report_of` road asked serde for the same tree a dozen times per
//! event. `HookPayload` pays that parse once; this bench is the witness that
//! the difference is real and stays real. The payload here is Stop-shaped and
//! carries a long answer, because that is the event where the repeat hurt:
//! the text being re-parsed is the whole turn.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use zerocode_core::agent::AgentKind;
use zerocode_core::payload::HookPayload;

/// A Stop payload the size a real turn leaves behind: a model line, a
/// transcript-free answer, and enough filler that the parse is not free.
fn stop_payload() -> String {
    let said = "The refactor is done. ".repeat(200);
    serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": "b3b2a5f0-6c1d-4e58-9a3b-2f6d8c4e1a07",
        "model": "claude-fable-5",
        "last_assistant_message": said,
        "background_tasks": [],
        "tool_name": "",
    })
    .to_string()
}

/// Every reader `report_of` asks of a Stop envelope, each paying its own
/// parse — the shape the shell had before `HookPayload`.
fn readers_reparse(payload: &str) {
    black_box(zerocode_core::hook::session_boundary(
        AgentKind::Claude,
        "Stop",
        payload,
    ));
    black_box(zerocode_core::hook::hook_state("Stop", payload));
    black_box(zerocode_core::provider_session::session_in_payload(
        AgentKind::Claude,
        payload,
    ));
    black_box(zerocode_core::transcript::said_in_payload(payload));
    black_box(zerocode_core::hook::helper_attributed(payload));
    black_box(zerocode_core::hook::model_in_payload(payload));
    black_box(zerocode_core::hook::subagent_in_payload(payload));
    black_box(zerocode_core::hook::payload_names_live_background_work(
        payload,
    ));
}

/// The same readers borrowing one parsed view — the shape the shell has now.
fn readers_parse_once(payload: &str) {
    let parsed = HookPayload::of(payload);
    black_box(zerocode_core::hook::session_boundary_parsed(
        AgentKind::Claude,
        "Stop",
        &parsed,
    ));
    black_box(zerocode_core::hook::hook_state_parsed("Stop", &parsed));
    black_box(zerocode_core::provider_session::session_in_parsed(
        AgentKind::Claude,
        &parsed,
    ));
    black_box(zerocode_core::transcript::said_in_parsed(&parsed));
    black_box(zerocode_core::hook::helper_attributed_parsed(&parsed));
    black_box(zerocode_core::hook::model_in_parsed(&parsed));
    black_box(zerocode_core::hook::subagent_in_parsed(&parsed));
    // No parsed twin on purpose: the background-work reader stayed on the
    // `&str` road (its callers live outside the per-event path), so the
    // parse-once side pays it too — the comparison stays honest about what
    // the shell actually runs.
    black_box(zerocode_core::hook::payload_names_live_background_work(
        payload,
    ));
}

fn bench(c: &mut Criterion) {
    let payload = stop_payload();
    let mut group = c.benchmark_group("hook_payload");
    group.bench_function("stop_readers_reparse_each", |b| {
        b.iter(|| readers_reparse(black_box(&payload)));
    });
    group.bench_function("stop_readers_parse_once", |b| {
        b.iter(|| readers_parse_once(black_box(&payload)));
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
