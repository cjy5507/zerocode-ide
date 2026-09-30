//! Nothing ticks on its own while nothing of the session runs (t-17057).
//!
//! An idle zo woke 3.2 times a second in t-11961's final run (Codex 2.0,
//! Gemini 1.2, Claude Code 16.8), and three things did it, each with a
//! `tokio::time::interval` of one second that ran whether or not anything of
//! the session ran: the idle loop's size-and-foreground poll, the roster
//! watcher and the completion pump's stall watch. They now wait on the
//! session's pulse, which beats while a helper or a background task runs and is
//! parked while none does.
//!
//! The behaviour is pinned where it lives — `session::pulse`'s tests for the
//! pulse, the watchers' tests for what they do at rest, and the pane resize
//! e2e for the reconciliation the idle poll exists for. This file pins the
//! structure that would quietly undo it: an unconditional interval put back in
//! one of these loops. The turn and compaction loops keep their own size poll on
//! purpose; their 32 ms frame tick has the loop awake already.

fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
    let start = source
        .find(from)
        .unwrap_or_else(|| panic!("`{from}` is gone from the source this test reads"));
    let rest = &source[start..];
    let end = rest
        .find(to)
        .unwrap_or_else(|| panic!("`{to}` is gone from the source this test reads"));
    &rest[..end]
}

/// The production part of a source file: what stands above its test module.
fn production(source: &str) -> &str {
    source.split("#[cfg(test)]").next().unwrap_or(source)
}

#[test]
fn the_idle_loop_and_a_teammates_wait_arm_no_size_interval_of_their_own() {
    let source = include_str!("../src/tui/app.rs");
    let idle = between(source, "async fn drive(", "async fn drive_teammate(");
    let waiting = between(source, "async fn await_parent(", "fn boot(&mut self)");
    for (name, body) in [("drive", idle), ("await_parent", waiting)] {
        assert!(
            !body.contains("interval(SIZE_POLL)"),
            "`{name}` arms a one-second size poll again: an idle zo would wake once a second for it"
        );
    }
}

#[test]
fn the_roster_watcher_and_the_stall_watch_arm_no_interval_of_their_own() {
    let watcher = production(include_str!("../src/session/subagent_progress.rs"));
    assert!(
        !watcher.contains("tokio::time::interval("),
        "the roster watcher ticks on a timer of its own again"
    );
    let pump = production(include_str!("../src/session/agent_completion_pump.rs"));
    assert!(
        !pump.contains("tokio::time::interval("),
        "the completion pump's stall watch ticks on a timer of its own again"
    );
}
