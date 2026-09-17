//! The generated hook scripts are a product surface: they run inside somebody
//! else's agent CLI. The rule they must never break is that they exit 0.

use std::io::Write;
use std::process::{Command, Stdio};

use zerocode_core::{ALL_AGENTS, AgentKind};
use zerocode_hookd::endpoint::EndpointFields;
use zerocode_hookd::{BridgeState, env_var, install_hook_scripts, serve};

fn run_script(
    path: &std::path::Path,
    env: &[(&str, String)],
    payload: &str,
) -> std::process::Output {
    let mut child = Command::new("/bin/sh")
        .arg(path)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook script");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    child.wait_with_output().expect("wait")
}

#[test]
fn every_agent_gets_an_executable_script() {
    let dir = tempfile::tempdir().expect("tempdir");
    let written = install_hook_scripts(dir.path()).expect("install");
    assert_eq!(written.len(), ALL_AGENTS.len());

    for agent in ALL_AGENTS {
        let path = dir.path().join(format!("{}-hook.sh", agent.slug()));
        assert!(path.exists(), "missing script for {}", agent.slug());
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains(&format!("/hook/{}", agent.slug())));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "{} is not executable", agent.slug());
        }
    }
}

#[test]
fn an_unconfigured_script_exits_zero_and_stays_silent() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_hook_scripts(dir.path()).expect("install");
    let script = dir.path().join("claude-hook.sh");

    let output = run_script(&script, &[], "{\"event\":\"stop\"}");
    assert!(output.status.success(), "hook must never fail its agent");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn an_empty_payload_exits_zero_without_calling_anything() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_hook_scripts(dir.path()).expect("install");
    let script = dir.path().join("codex-hook.sh");

    let output = run_script(
        &script,
        &[
            (env_var::PORT, "1".to_string()),
            (env_var::TOKEN, "t".to_string()),
            (env_var::PANE_KEY, "a/b".to_string()),
        ],
        "",
    );
    assert!(output.status.success());
}

#[test]
fn a_dead_bridge_still_leaves_the_agent_healthy() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_hook_scripts(dir.path()).expect("install");
    let script = dir.path().join("claude-hook.sh");
    let marker = dir.path().join("failures/window-1/term-7");

    // Port 1 on loopback: nothing listens there, so curl fails fast.
    let output = run_script(
        &script,
        &[
            (env_var::PORT, "1".to_string()),
            (env_var::TOKEN, "t".to_string()),
            (env_var::PANE_KEY, "a/b".to_string()),
            (
                env_var::DELIVERY_FAILURE_MARKER,
                marker.to_string_lossy().into_owned(),
            ),
        ],
        "{\"event\":\"stop\"}",
    );
    assert!(output.status.success(), "hook must never fail its agent");
    assert!(
        output.stderr.is_empty(),
        "hook must not pollute agent stderr"
    );
    assert!(
        marker.is_dir(),
        "a failed delivery left no evidence for the window"
    );
}

/// A payload the window already HOLDS is not a broken bridge.
///
/// The script's whole judgement is `curl`'s exit status, and that status
/// answers a narrower question than the one the marker claims to answer. With
/// a bounded `--max-time`, the request body can be entirely on the wire —
/// read, parsed, enveloped, already the window's — while the answer is still
/// owed; curl gives up (28) and the script calls a delivery that landed a
/// failure. Nothing ever takes that back: the marker is erased only by this
/// pane's NEXT successful POST, so an agent whose last turn ended this way
/// reads `unreachable` for the rest of the session, on the one signal that
/// exists to say the opposite.
///
/// A bridge that is not there (7) and a window that refuses the payload (22,
/// which the 503 produces) both still mark, and a window that accepts the
/// connection and never answers is still caught by the readiness deadline —
/// the other half of `unreachable`. Only the false word is given up here.
#[tokio::test(flavor = "multi_thread")]
async fn a_delivered_payload_whose_answer_was_lost_is_not_a_broken_bridge() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let dir = tempfile::tempdir().expect("tempdir");
    install_hook_scripts(dir.path()).expect("install");
    let script = dir.path().join("claude-hook.sh");
    let marker = dir.path().join("failures/window-1/term-7");

    // A bridge that takes the whole payload and owes its answer for longer
    // than the script is willing to wait. Hand-rolled rather than `serve`,
    // because the fact under test is the answer's lateness and nothing the
    // real router does can be asked to be late.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (took, taken) = tokio::sync::oneshot::channel();
    let holding = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut seen = Vec::new();
        let mut buffer = [0_u8; 4096];
        // The body carries the pane key; seeing it is seeing the delivery.
        while !String::from_utf8_lossy(&seen).contains("pane_key=term-7") {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => seen.extend_from_slice(&buffer[..read]),
            }
        }
        let _ = took.send(String::from_utf8_lossy(&seen).contains("pane_key=term-7"));
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let _ = stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
            .await;
    });

    let script_path = script.clone();
    let env = vec![
        (env_var::PORT, port.to_string()),
        (env_var::TOKEN, "secret".to_string()),
        (env_var::PANE_KEY, "term-7".to_string()),
        (
            env_var::DELIVERY_FAILURE_MARKER,
            marker.to_string_lossy().into_owned(),
        ),
    ];
    let output =
        tokio::task::spawn_blocking(move || run_script(&script_path, &env, "{\"event\":\"stop\"}"))
            .await
            .expect("join");

    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), taken)
            .await
            .expect("the bridge should have been given the payload")
            .expect("the reading half"),
        "the test bridge never saw the body it is supposed to be holding"
    );
    assert!(output.status.success(), "hook must never fail its agent");
    assert!(
        output.stderr.is_empty(),
        "hook must not pollute agent stderr"
    );
    assert!(
        !marker.exists(),
        "a payload the window received was written down as a broken bridge"
    );
    holding.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_bridge_receives_what_the_script_sends() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_hook_scripts(dir.path()).expect("install");
    let script = dir.path().join("claude-hook.sh");
    let marker = dir.path().join("failures/window-1/term-7");
    std::fs::create_dir_all(&marker).expect("standing failure marker");

    let (state, mut events, _teams, _browser) = BridgeState::new("secret", "browser-secret");
    let (addr, _server) = serve(state, 0).await.expect("serve");
    assert!(addr.ip().is_loopback(), "the bridge must stay on loopback");

    let script_path = script.clone();
    let env = vec![
        (env_var::PORT, addr.port().to_string()),
        (env_var::TOKEN, "secret".to_string()),
        (env_var::PANE_KEY, "tab-1/leaf-1".to_string()),
        (env_var::TAB_ID, "tab-1".to_string()),
        (env_var::WORKTREE_ID, "wt-7".to_string()),
        (
            env_var::DELIVERY_FAILURE_MARKER,
            marker.to_string_lossy().into_owned(),
        ),
    ];
    let output = tokio::task::spawn_blocking(move || {
        run_script(&script_path, &env, "{\"event\":\"permission_request\"}")
    })
    .await
    .expect("join");
    assert!(output.status.success());
    assert!(
        !marker.exists(),
        "a successful delivery left the bridge marked unreachable"
    );

    let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await
        .expect("bridge should receive the hook")
        .expect("envelope");
    assert_eq!(envelope.agent, AgentKind::Claude);
    assert_eq!(envelope.pane_key, "tab-1/leaf-1");
    assert_eq!(envelope.worktree_id, "wt-7");
    assert_eq!(envelope.payload, "{\"event\":\"permission_request\"}");
    assert_eq!(envelope.version, zerocode_hookd::HOOK_CONTRACT_VERSION);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hook_child_reads_the_bridge_token_from_the_endpoint_path() {
    let scripts = tempfile::tempdir().expect("scripts dir");
    let endpoint_dir = tempfile::tempdir().expect("endpoint dir");
    install_hook_scripts(scripts.path()).expect("install");
    let (state, mut events, _teams, _browser) = BridgeState::new("secret", "browser-secret");
    let (addr, _server) = serve(state, 0).await.expect("serve");
    let endpoint = zerocode_hookd::endpoint::write_endpoint_file(
        endpoint_dir.path(),
        &EndpointFields {
            port: addr.port(),
            token: "secret".to_string(),
            env: "production".to_string(),
            version: zerocode_hookd::HOOK_CONTRACT_VERSION.to_string(),
        },
    )
    .expect("endpoint");

    let script = scripts.path().join("claude-hook.sh");
    let endpoint_path = endpoint.to_string_lossy().into_owned();
    let output = tokio::task::spawn_blocking(move || {
        run_script(
            &script,
            &[
                (env_var::ENDPOINT, endpoint_path),
                (env_var::PANE_KEY, "path-only/claude".to_string()),
            ],
            "{\"event\":\"stop\"}",
        )
    })
    .await
    .expect("join");
    assert!(output.status.success(), "hook failed without TOKEN env");

    let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await
        .expect("bridge should receive the hook")
        .expect("envelope");
    assert_eq!(envelope.pane_key, "path-only/claude");
    assert_eq!(envelope.payload, "{\"event\":\"stop\"}");
}

#[tokio::test(flavor = "multi_thread")]
async fn measured_coordinator_scripts_print_the_bridge_context_as_valid_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_hook_scripts(dir.path()).expect("install");
    let (state, mut events, _teams, _browser) = BridgeState::new("secret", "browser-secret");
    let (addr, _server) = serve(state, 0).await.expect("serve");

    for (agent, event_name) in [
        (AgentKind::Claude, "SessionStart"),
        (AgentKind::Codex, "SessionStart"),
    ] {
        let script = dir.path().join(format!("{}-hook.sh", agent.slug()));
        let mut env = vec![
            (env_var::PORT, addr.port().to_string()),
            (env_var::TOKEN, "secret".to_string()),
            (env_var::PANE_KEY, format!("tab/{}", agent.slug())),
        ];
        if agent != AgentKind::Codex {
            env.push((env_var::EVENT, event_name.to_string()));
        }
        let payload = serde_json::json!({
            "hook_event_name": event_name,
            "session_id": format!("session-{}", agent.slug()),
        })
        .to_string();
        let output = tokio::task::spawn_blocking(move || run_script(&script, &env, &payload))
            .await
            .expect("join");

        assert!(output.status.success(), "{} hook failed", agent.slug());
        assert!(output.stderr.is_empty(), "{} polluted stderr", agent.slug());
        let reply: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("hook stdout is one JSON value");
        assert_eq!(reply["hookSpecificOutput"]["hookEventName"], event_name);
        assert_eq!(
            reply["hookSpecificOutput"]["additionalContext"],
            zerocode_core::delegation::AGENT_SELECTION_CONTEXT
        );
        assert_eq!(events.recv().await.expect("envelope").agent, agent);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_rejections_never_become_provider_hook_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    install_hook_scripts(dir.path()).expect("install");
    let (state, _events, _teams, _browser) = BridgeState::new("secret", "browser-secret");
    let (addr, _server) = serve(state, 0).await.expect("serve");
    let oversized = format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"{}"}}"#,
        "x".repeat(zerocode_hookd::MAX_HOOK_BODY_BYTES + 1)
    );

    for (agent, event_name, expected) in [
        (AgentKind::Claude, "UserPromptSubmit", &b""[..]),
        (AgentKind::Codex, "UserPromptSubmit", &b""[..]),
    ] {
        let script = dir.path().join(format!("{}-hook.sh", agent.slug()));
        let mut env = vec![
            (env_var::PORT, addr.port().to_string()),
            (env_var::TOKEN, "secret".to_string()),
            (env_var::PANE_KEY, format!("reject/{}", agent.slug())),
        ];
        if agent != AgentKind::Codex {
            env.push((env_var::EVENT, event_name.to_string()));
        }
        let payload = oversized.clone();
        let output = tokio::task::spawn_blocking(move || run_script(&script, &env, &payload))
            .await
            .expect("join");
        assert!(output.status.success(), "{} hook failed", agent.slug());
        assert_eq!(
            output.stdout,
            expected,
            "{} printed the bridge's HTTP error body",
            agent.slug()
        );
        assert!(output.stderr.is_empty());
    }
}

/// The other road on the same bridge: an orchestrating agent's fake `tmux`,
/// end to end — the shim script this window writes, run by `sh`, against the
/// real server.
///
/// Driven as a script rather than as a POST because the script IS the part
/// that can be wrong in ways Rust cannot see: a quoting slip in the argv
/// packing, a status read that never fires, a failure that exits 0. Everything
/// above it is already covered by calling it.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_bridge_answers_the_tmux_shim_and_refuses_a_stranger() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = dir.path().join("tmux");
    std::fs::write(
        &shim,
        zerocode_core::agent_teams::shim_script(env_var::PORT, env_var::TOKEN, "tmux"),
    )
    .expect("shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let (state, _events, mut teams, _browser) = BridgeState::new("secret", "browser-secret");
    let (addr, _server) = serve(state, 0).await.expect("serve");
    assert!(addr.ip().is_loopback(), "the bridge must stay on loopback");

    // The window's end: answer the first request, so the shim has something
    // real to print.
    let answering = tokio::spawn(async move {
        let request = teams.recv().await.expect("a request");
        let seen = (
            request.team_id.clone(),
            request.pane.clone(),
            request.pane_token.clone(),
            request.argv.clone(),
        );
        request
            .answer
            .send(zerocode_hookd::TeamAnswer {
                stdout: "%3\n".to_string(),
                stderr: String::new(),
                exit_code: 0,
            })
            .expect("answer");
        seen
    });

    let run = |token: &str, pane_token: &str, port: u16| {
        let shim = shim.clone();
        let token = token.to_string();
        let pane_token = pane_token.to_string();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("sh")
                .arg(&shim)
                .args(["split-window", "-h", "claude --teammate-mode auto"])
                .env(env_var::PORT, port.to_string())
                .env(env_var::TOKEN, token)
                .env(zerocode_core::agent_teams::TEAM_ID_VAR, "team-9")
                .env(zerocode_core::agent_teams::TEAM_TOKEN_VAR, pane_token)
                // This test itself may run inside a ZeroCode worker pane. Its
                // ambient logical pane outranks TMUX_PANE in the shim, so it
                // must not leak into the child fixture.
                .env_remove(zerocode_core::agent_teams::TEAM_PANE_VAR)
                .env("TMUX_PANE", "%1")
                .output()
                .expect("sh")
        })
    };

    let output = run("secret", "pane-1-secret", addr.port())
        .await
        .expect("join");
    assert!(
        output.status.success(),
        "the shim failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "%3\n");
    let (team, pane, pane_token, argv) =
        tokio::time::timeout(std::time::Duration::from_secs(5), answering)
            .await
            .expect("the bridge should have been reached")
            .expect("join");
    assert_eq!(team, "team-9");
    assert_eq!(pane, "%1");
    assert_eq!(pane_token, "pane-1-secret");
    // The argument with a space in it survived the trip whole — the one thing
    // a hand-rolled encoder gets wrong, and the reason the body is separated
    // by a character no shell can produce.
    assert_eq!(argv, ["split-window", "-h", "claude --teammate-mode auto"]);

    // And the token is the whole authentication. A stranger on loopback gets
    // a refusal the shim turns into a non-zero status, not a pane.
    let stranger = run("guess", "pane-1-secret", addr.port())
        .await
        .expect("join");
    assert!(
        !stranger.status.success(),
        "an unauthenticated tmux succeeded"
    );
    assert!(
        stranger.stdout.is_empty(),
        "a refusal reached the agent as an answer"
    );
    let no_pane_capability = run("secret", "", addr.port()).await.expect("join");
    assert!(
        !no_pane_capability.status.success(),
        "a team request without a pane capability succeeded"
    );
}

/// The shim's patience must outlast the window's deadline.
///
/// The window answers a request it cannot serve with a sentence an agent can
/// read ("timed out waiting for the window"). If `curl` gave up first, that
/// sentence would be replaced by "could not reach the window" — which means
/// something else entirely, and would send an agent looking for a bridge that
/// was listening the whole time.
///
/// The two constants live in two crates that cannot see each other (the shim's
/// is in `zerocode-core`, which knows nothing of the server). This is the one
/// place both are in scope, so this is where they are held apart.
#[test]
fn the_shim_waits_longer_than_the_window_is_allowed_to_take() {
    let shim = std::time::Duration::from_secs(u64::from(
        zerocode_core::agent_teams::SHIM_DEADLINE_SECONDS,
    ));
    assert!(
        shim > zerocode_hookd::TEAM_DEADLINE,
        "the shim hangs up before the window can refuse: {shim:?} vs {:?}",
        zerocode_hookd::TEAM_DEADLINE
    );
    // And a `check --wait` gives up before EITHER of them. Three deadlines in a
    // row, and the order is the whole of their meaning: the sleeper first, so
    // an empty inbox answers `{"count":0}` — which the ninth invariant says is
    // a checkpoint, not a failure. If the bridge gave up first the agent would
    // read "timed out waiting for the window", and if the shim gave up before
    // that, "could not reach the window". Three sentences meaning three
    // different things, of which only the first is true.
    let sleeper =
        std::time::Duration::from_secs(u64::from(zerocode_core::orchestration::WAIT_SECONDS));
    assert!(
        sleeper < zerocode_hookd::TEAM_DEADLINE,
        "a waiting check outlasts the bridge, so silence comes back as a \
         timeout instead of an empty inbox: {sleeper:?} vs {:?}",
        zerocode_hookd::TEAM_DEADLINE
    );
    // The same ladder holds PER REQUEST when a caller brings its own budget:
    // the sleeper holds exactly the budget, the bridge adds its grace, the
    // shim adds a bigger grace on the same base. Checked at the widest budget
    // any verb accepts, because that is where an off-by-one would live.
    let bridge_grace = zerocode_hookd::BRIDGE_GRACE;
    let shim_grace = std::time::Duration::from_secs(u64::from(
        zerocode_core::agent_teams::SHIM_WAIT_GRACE_SECONDS,
    ));
    assert!(
        bridge_grace < shim_grace,
        "the bridge outlasts the shim on a budgeted wait: {bridge_grace:?} vs {shim_grace:?}"
    );
    assert!(
        u64::from(zerocode_core::orchestration::WAIT_BUDGET_MAX_MS)
            <= zerocode_hookd::WAIT_BUDGET_CEILING_MS,
        "the ledger accepts a budget the bridge would refuse to hold"
    );
    // And the parsed deadline really rides the argv: a budgeted long-running
    // request stretches the bridge, a plain check does not, and a short verb
    // ignores the flag entirely.
    let request = |argv: &[&str]| {
        let (answer, _wait) = tokio::sync::oneshot::channel();
        zerocode_hookd::TeamRequest::new(
            "team".into(),
            "%1".into(),
            "token".into(),
            argv.iter().map(|word| word.to_string()).collect(),
            answer,
        )
    };
    assert_eq!(
        request(&["check", "--wait", "--timeout-ms", "120000"]).deadline(),
        std::time::Duration::from_millis(120_000) + bridge_grace,
    );
    assert_eq!(
        request(&["check", "--wait"]).deadline(),
        zerocode_hookd::TEAM_DEADLINE,
    );
    assert_eq!(
        request(&["task-list", "--timeout-ms", "120000"]).deadline(),
        zerocode_hookd::TEAM_DEADLINE,
    );
    // An absurd spelling is capped, never held open unchecked.
    assert_eq!(
        request(&["check", "--wait", "--timeout-ms", "999999999"]).deadline(),
        std::time::Duration::from_millis(zerocode_hookd::WAIT_BUDGET_CEILING_MS) + bridge_grace,
    );
    // `ask` blocks with no flag at all: it takes the wait seat, and a
    // budget-less one stretches the bridge to the ledger's own default —
    // whose ceiling the bridge can actually hold.
    assert!(request(&["ask", "--body", "which?"]).reserves_wait_slot());
    assert_eq!(
        request(&["ask", "--body", "which?"]).deadline(),
        std::time::Duration::from_millis(u64::from(
            zerocode_core::orchestration::ASK_BUDGET_DEFAULT_MS
        )) + bridge_grace,
    );
    assert_eq!(
        request(&["ask", "--body", "which?", "--timeout-ms", "1500000"]).deadline(),
        std::time::Duration::from_millis(1_500_000) + bridge_grace,
    );
    // A worker summons waits for launch readiness too — with the ledger's
    // default when no flag was supplied and the caller's budget when it was.
    assert!(request(&["worker-start", "--agent", "claude"]).reserves_wait_slot());
    assert_eq!(
        request(&["worker-start", "--agent", "claude"]).deadline(),
        std::time::Duration::from_millis(u64::from(
            zerocode_core::orchestration::READY_TIMEOUT_DEFAULT_MS
        )) + bridge_grace,
    );
    assert_eq!(
        request(&[
            "worker-start",
            "--agent",
            "claude",
            "--timeout-ms",
            "120000",
        ])
        .deadline(),
        std::time::Duration::from_millis(120_000) + bridge_grace,
    );
    assert!(
        u64::from(zerocode_core::orchestration::ASK_BUDGET_MAX_MS)
            <= zerocode_hookd::WAIT_BUDGET_CEILING_MS,
        "the ledger accepts an ask budget the bridge would refuse to hold"
    );
}
