//! The kill guard in the bridge's reply (t-80602): a pane's command that
//! would signal another pane is refused in its own pre-tool hook's answer,
//! and a command without a kill is answered exactly as before, with the
//! process table never read. The table is a fake one, counted.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use zerocode_core::AgentKind;
use zerocode_core::shell_rule::kill::{PaneMark, ProcessRow, ProcessTable, ProtectedRoot};
use zerocode_hookd::{BridgeState, HOOK_TOKEN_HEADER, KillRoots, KillWitness, router};

const TOKEN: &str = "test-token";
const CALLER: u32 = 200;
/// The pid the refusal's way out names: a job the calling pane set loose.
const SET_LOOSE: u32 = 4242;
const SET_LOOSE_BY_THE_OTHER_PANE: u32 = 4343;

/// A window (100) with the calling pane (200, its agent 210) and another
/// pane (300) whose agent (310) the incident's pattern matches, and a job
/// each pane set loose under pid 1, naming its pane in its environment.
struct FakeWindow {
    tables: AtomicUsize,
    readable: bool,
}

impl FakeWindow {
    fn new(readable: bool) -> Arc<Self> {
        Arc::new(Self {
            tables: AtomicUsize::new(0),
            readable,
        })
    }

    fn tables_read(&self) -> usize {
        self.tables.load(Ordering::SeqCst)
    }
}

impl KillWitness for FakeWindow {
    fn roots(&self, pane_key: &str) -> KillRoots {
        assert_eq!(pane_key, "term-1", "the guard asked about another pane");
        KillRoots {
            caller: Some(CALLER),
            protected: vec![
                ProtectedRoot {
                    pid: 100,
                    label: "the ZeroCode window".to_string(),
                },
                ProtectedRoot {
                    pid: 300,
                    label: "pane term-2".to_string(),
                },
            ],
        }
    }

    fn table(&self) -> ProcessTable {
        self.tables.fetch_add(1, Ordering::SeqCst);
        if !self.readable {
            return ProcessTable::unreadable("ps did not answer");
        }
        let row = |pid, ppid, command: &str| ProcessRow {
            pid,
            ppid,
            pgid: pid,
            command: command.to_string(),
        };
        ProcessTable::new(vec![
            row(1, 0, "/sbin/launchd"),
            row(
                100,
                1,
                "/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell",
            ),
            row(CALLER, 100, "-zsh"),
            row(210, CALLER, "claude"),
            row(300, 100, "-zsh"),
            row(310, 300, "zo zo-step run"),
            row(SET_LOOSE, 1, "python3 -m http.server 8000"),
            row(
                SET_LOOSE_BY_THE_OTHER_PANE,
                1,
                "python3 -m http.server 8001",
            ),
        ])
        .with_pane_mark(CALLER, pane("term-1"))
        .with_pane_mark(SET_LOOSE, pane("term-1"))
        .with_pane_mark(300, pane("term-2"))
        .with_pane_mark(SET_LOOSE_BY_THE_OTHER_PANE, pane("term-2"))
    }
}

/// What a process of this window's pane `key` carries in its environment.
fn pane(key: &str) -> PaneMark {
    PaneMark {
        window: 100,
        pane: key.to_string(),
    }
}

fn pre_tool_use(agent: AgentKind, command: &str) -> Request<Body> {
    pre_tool_call(agent, "Bash", serde_json::json!(command))
}

/// A pre-tool knock carrying `command` the way `agent`'s `tool` holds it: a
/// string for Claude's `Bash`, an argv for Codex's `shell`.
fn pre_tool_call(agent: AgentKind, tool: &str, command: serde_json::Value) -> Request<Body> {
    let body = serde_json::json!({
        "pane_key": "term-1",
        "payload": {
            "hook_event_name": "PreToolUse",
            "session_id": "session-1",
            "tool_name": tool,
            "tool_use_id": "call-1",
            "tool_input": { "command": command },
        }
    })
    .to_string();
    Request::post(format!("/hook/{}", agent.slug()))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(HOOK_TOKEN_HEADER, TOKEN)
        .body(Body::from(body))
        .expect("request")
}

/// The reply's status and body for `command` from `agent`, with `window`
/// installed as the witness (or none).
async fn answer(
    window: Option<Arc<FakeWindow>>,
    agent: AgentKind,
    command: &str,
) -> (StatusCode, Vec<u8>) {
    answer_to(window, pre_tool_use(agent, command)).await
}

async fn answer_to(window: Option<Arc<FakeWindow>>, knock: Request<Body>) -> (StatusCode, Vec<u8>) {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, "browser");
    let state = match window {
        Some(window) => state.with_kill_witness(window),
        None => state,
    };
    let response = router(state).oneshot(knock).await.expect("service");
    assert!(
        events.recv().await.is_some(),
        "the window was handed the event"
    );
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .expect("body")
        .to_vec();
    (status, body)
}

fn decision(body: &[u8]) -> (String, String) {
    let reply: serde_json::Value = serde_json::from_slice(body).expect("a JSON reply");
    let output = &reply["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreToolUse");
    (
        output["permissionDecision"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        output["permissionDecisionReason"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The incident's shape, as Claude's hook carries it: refused before it
/// runs, with the process and the pane it would have hit named.
#[tokio::test]
async fn the_incidents_command_is_refused_in_claudes_pre_tool_reply() {
    let window = FakeWindow::new(true);
    let command =
        "zerocode-orc worker-start --agent claude --prompt \"예: `pkill -f zo-step foo` 금지\"";
    let (status, body) = answer(Some(Arc::clone(&window)), AgentKind::Claude, command).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (word, reason) = decision(&body);
    assert_eq!(word, "deny");
    assert!(
        reason.contains("pid 310") && reason.contains("pane term-2"),
        "{reason}"
    );
    assert_eq!(
        window.tables_read(),
        1,
        "the table is read once per command"
    );
}

/// A command without a kill is answered byte for byte as before, and the
/// process table is never read for it.
#[tokio::test]
async fn a_command_without_kill_reads_no_table_and_keeps_its_reply() {
    for command in [
        "cargo test -p zerocode-core",
        "ls skills/",
        "echo 'pkill -f x'",
    ] {
        let window = FakeWindow::new(true);
        let guarded = answer(Some(Arc::clone(&window)), AgentKind::Claude, command).await;
        let unguarded = answer(None, AgentKind::Claude, command).await;
        assert_eq!(guarded, unguarded, "{command:?}");
        assert_eq!(window.tables_read(), 0, "{command:?}");
    }
}

/// Nobody in a pane can answer a question — a worker's pane has no person at
/// it, and a pane sitting on a prompt waits forever — so what core would ask
/// of a person is refused, with the ways this pane can still end what it
/// started.
#[tokio::test]
async fn the_callers_own_child_is_allowed_and_a_pid_known_only_at_run_time_is_refused() {
    let window = FakeWindow::new(true);
    let (_, body) = answer(Some(Arc::clone(&window)), AgentKind::Claude, "kill 210").await;
    assert!(body.is_empty(), "{}", String::from_utf8_lossy(&body));
    let (_, body) = answer(Some(window), AgentKind::Claude, "kill $SERVER_PID").await;
    let (word, reason) = decision(&body);
    assert_eq!(word, "deny");
    assert!(reason.contains("SERVER_PID"), "{reason}");
    for way_out in ["kill 4242", "kill %1", "pgrep"] {
        assert!(reason.contains(way_out), "{way_out:?} in {reason}");
    }
}

/// The refusal's way out works for a job the pane set loose (t-81714): its
/// pid, learned with `echo $!`, ends it by number, as the refusal says —
/// while another pane's loose job is refused all the same.
#[tokio::test]
async fn the_refusals_way_out_ends_a_job_the_pane_set_loose() {
    let window = FakeWindow::new(true);
    let command = format!("kill {SET_LOOSE_BY_THE_OTHER_PANE}");
    let (_, body) = answer(Some(Arc::clone(&window)), AgentKind::Claude, &command).await;
    let (word, reason) = decision(&body);
    assert_eq!(word, "deny");
    assert!(
        reason.contains(&format!("pid {SET_LOOSE_BY_THE_OTHER_PANE}")),
        "{reason}"
    );
    let command = format!("kill {SET_LOOSE}");
    let (_, body) = answer(Some(window), AgentKind::Claude, &command).await;
    assert!(body.is_empty(), "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
async fn a_table_that_did_not_read_is_refused() {
    let window = FakeWindow::new(false);
    let (_, body) = answer(
        Some(Arc::clone(&window)),
        AgentKind::Claude,
        "pkill -f zo-step",
    )
    .await;
    let (word, reason) = decision(&body);
    assert_eq!(word, "deny");
    assert!(reason.contains("ps did not answer"), "{reason}");
    assert_eq!(window.tables_read(), 1);
}

/// Codex's hook cannot ask a person: a question goes to it as a refusal,
/// never as an answer it would drop and run the command anyway.
#[tokio::test]
async fn codex_is_refused_what_claude_would_be_asked() {
    let (_, body) = answer(
        Some(FakeWindow::new(true)),
        AgentKind::Codex,
        "kill $SERVER_PID",
    )
    .await;
    let (word, reason) = decision(&body);
    assert_eq!(word, "deny");
    assert!(!reason.is_empty());
    let (_, body) = answer(
        Some(FakeWindow::new(true)),
        AgentKind::Codex,
        "pkill -f zo-step",
    )
    .await;
    assert_eq!(decision(&body).0, "deny");
}

/// Codex's own shell tool hands its command as an argv, `bash -lc` and the
/// script: the script inside is read and refused all the same.
#[tokio::test]
async fn codexs_argv_shell_call_is_read_and_refused() {
    let window = FakeWindow::new(true);
    let knock = pre_tool_call(
        AgentKind::Codex,
        "shell",
        serde_json::json!(["bash", "-lc", "pkill -f zo-step"]),
    );
    let (status, body) = answer_to(Some(Arc::clone(&window)), knock).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (word, reason) = decision(&body);
    assert_eq!(word, "deny");
    assert!(
        reason.contains("pid 310") && reason.contains("pane term-2"),
        "{reason}"
    );
    assert_eq!(window.tables_read(), 1);
}

/// A provider whose pre-tool reply was never measured is not handed one.
#[tokio::test]
async fn an_unmeasured_provider_is_answered_as_before() {
    let window = FakeWindow::new(true);
    let guarded = answer(
        Some(Arc::clone(&window)),
        AgentKind::Cursor,
        "pkill -f zo-step",
    )
    .await;
    let unguarded = answer(None, AgentKind::Cursor, "pkill -f zo-step").await;
    assert_eq!(guarded, unguarded);
    assert_eq!(window.tables_read(), 0);
}
