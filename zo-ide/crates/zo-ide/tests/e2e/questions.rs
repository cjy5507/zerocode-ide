//! `AskUserQuestion` in Claude Code's shape, typed into the shipped `zo`
//! (t-95970 C6): a `questions` array of two decisions, each with a chip and
//! 2-4 options. The overlay asks them in turn — `Question 1/2`, `Question
//! 2/2` — gathers the answers on a last screen, and Enter sends them; the
//! model reads one answer per question. The scrollback keeps one row per
//! question and no `Call AskUserQuestion(…)` cell repeating the input.

use super::*;
use crate::e2e::scripted::body_text_contains;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_two_questions_are_asked_in_turn_and_answered_together() {
    const ROWS: usize = 40;
    let layout = Layout::new();
    let questions = json!({ "questions": [
        { "question": "Which store keeps the session cache?", "header": "Cache", "options": [
            { "label": "Redis (Recommended)", "description": "Shared by every worker; one more service to run." },
            { "label": "In memory", "description": "Nothing to run; each worker keeps its own copy." }
        ]},
        { "question": "Which checks run before a release?", "header": "Checks", "options": [
            { "label": "Unit only", "description": "Two minutes; misses screen regressions." },
            { "label": "Unit and e2e", "description": "Twelve minutes; catches screen regressions." }
        ]}
    ]});
    let service = ScriptedAnthropicService::tool_calls(&[("AskUserQuestion", questions)], "Answers received.")
        .await
        .expect("start question script");
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Set up the cache\r").expect("send prompt");
    run.wait_for_screen("Question 1/2", ROWS, TEST_TIMEOUT);
    let at = run.output_len();
    run.send(b"1").expect("pick Redis");
    run.wait_for_screen_after("Question 2/2", ROWS, at, TEST_TIMEOUT);
    let at = run.output_len();
    run.send(b"2").expect("pick unit and e2e");
    run.wait_for_after("Review answers", at, TEST_TIMEOUT);
    run.send(b"\r").expect("send the answers");
    run.wait_for_history_row("Answers received.", TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    let mut screen = Screen::new(ROWS);
    screen.feed(&run.snapshot_output());
    let rows = screen.transcript();
    let _ = run.finish();

    let bodies = service.request_bodies().await;
    assert_eq!(bodies.len(), 2, "the answers went back in one tool result");
    let request: Value = serde_json::from_str(&bodies[1]).expect("request JSON");
    let answer = &request;
    assert!(body_text_contains(answer, "\"Which store keeps the session cache?\": \"Redis (Recommended)\""), "{}", bodies[1]);
    assert!(body_text_contains(answer, "\"Which checks run before a release?\": \"Unit and e2e\""), "{}", bodies[1]);
    assert!(rows.iter().any(|row| row.trim_end() == "  · Cache → Redis (Recommended)"), "{rows:#?}");
    assert!(rows.iter().any(|row| row.trim_end() == "  · Checks → Unit and e2e"), "{rows:#?}");
    assert!(!rows.iter().any(|row| row.contains("Call AskUserQuestion")), "the input is not repeated: {rows:#?}");
}
