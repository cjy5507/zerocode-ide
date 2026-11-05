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

/// `/plan off` does what the plan card promises (t-95970 C3). In plan mode
/// (read-only) the model submits a plan; the card says `/plan off to
/// approve`. Before this the TUI's own command list had no `/plan`, so the
/// line answered "is not in zo" and plan mode stayed on. Now `/plan off`
/// leaves plan mode for workspace-write — Shift+Tab's step from read-only —
/// and `/plan on` goes back; neither reaches the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_plan_off_approves_the_plan_the_card_shows() {
    const ROWS: u16 = 40;
    let layout = Layout::new();
    let service = ScriptedAnthropicService::tool_calls(
        &[("ExitPlanModeV2", serde_json::json!({"plan": "1. Read the module\n2. Change one line"}))],
        "Plan submitted for approval.",
    )
    .await
    .expect("start plan script");
    let mut run = pty(&layout, service.base_url(), &["--permission-mode", "read-only"]);
    run.wait_for("directory:", TEST_TIMEOUT);
    run.send(b"Plan the change\r").expect("send prompt");
    run.wait_for("to approve", TEST_TIMEOUT);
    run.wait_for_history_row("Plan submitted for approval.", TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    let footer_row_one = |run: &PtyRun| {
        let visible = settled_screen(&run.snapshot_output(), ROWS).visible();
        let hints = footer_last_row(&visible).expect("the footer is on screen");
        visible[hints - 1].clone()
    };
    assert!(footer_row_one(&run).contains("Plan mode"), "read-only shows plan mode");

    let at = run.output_len();
    run.send(b"/plan off\r").expect("send /plan off");
    run.wait_until(at, TEST_TIMEOUT, |bytes| {
        let text = String::from_utf8_lossy(bytes);
        text.contains("is not in zo") || text.contains("Plan mode off")
    });
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    let after = String::from_utf8_lossy(&run.snapshot_output()[at..]).to_string();
    assert!(!after.contains("is not in zo"), "/plan off is not a zo command: {}", after.replace('\u{1b}', "\\e"));
    assert!(!footer_row_one(&run).contains("Plan mode"), "plan mode stayed on: {}", footer_row_one(&run));

    let at = run.output_len();
    run.send(b"/status\r").expect("send /status");
    run.wait_for_after("Permissions", at, TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    let mut screen = Screen::new(usize::from(ROWS));
    screen.feed(&run.snapshot_output());
    let rows = screen.transcript();
    let permissions = rows.iter().rev().find(|row| row.contains("Permissions")).expect("the status card's permissions row");
    assert!(permissions.contains("workspace-write"), "write access is back: {permissions}");

    let at = run.output_len();
    run.send(b"/plan on\r").expect("send /plan on");
    run.wait_for_after("Plan mode on", at, TEST_TIMEOUT);
    wait_until_quiet(&run, Duration::from_millis(250), TEST_TIMEOUT);
    assert!(footer_row_one(&run).contains("Plan mode"), "/plan on goes back to plan mode");
    let _ = run.finish();
    assert_eq!(service.request_bodies().await.len(), 2, "only the plan turn reached the model");
}
