//! `/context` typed into the shipped `zo` (adversarial review A, Z12,
//! 2026-10-28). The runtime already counted what fills the window and where
//! each compaction tier sits (`ConversationRuntime::context_breakdown_report`),
//! but `/context` answered "is not in zo". Both front ends answer it now,
//! from the session, without asking the model.

use super::*;

/// The report's rows, by the words `context_breakdown_report` writes.
const BREAKDOWN_ROWS: [&str; 7] = [
    "Window",
    "In use",
    "System prompt",
    "Messages",
    "Ladder",
    "Auto compact",
    "Headroom",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_context_in_the_tui_shows_what_fills_the_window() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused").await.expect("start text script");
    let mut run = pty(&layout, service.base_url(), &interactive_args());
    run.wait_for("directory:", TEST_TIMEOUT);

    run.send(b"/context\r").expect("send /context");
    run.wait_for_history_row("Auto compact", Duration::from_secs(20));
    let mut screen = Screen::new(40);
    screen.feed(&run.snapshot_output());
    let rows = screen.transcript();
    for row in BREAKDOWN_ROWS {
        assert!(rows.iter().any(|line| line.contains(row)), "/context lacks {row}: {rows:#?}");
    }
    assert!(!rows.iter().any(|line| line.contains("is not in zo")), "{rows:#?}");
    assert!(service.request_bodies().await.is_empty(), "/context asked the model");
    let _ = run.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_context_in_the_pipe_prints_what_fills_the_window() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text("unused").await.expect("start text script");
    let output = run_pipe(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--plain", "--permission-mode", "danger-full-access"],
        b"/context\n/exit\n",
    )
    .expect("run zo in pipe mode");
    assert!(output.status.success(), "pipe exited with {:?}", output.status);

    let stdout = String::from_utf8_lossy(&output.stdout);
    for row in BREAKDOWN_ROWS {
        assert!(stdout.contains(row), "/context lacks {row}:\n{stdout}");
    }
    assert!(service.request_bodies().await.is_empty(), "/context asked the model");
    // The sample a hand-in quotes: `cargo test ... -- --nocapture` prints it.
    println!("{stdout}");
}
