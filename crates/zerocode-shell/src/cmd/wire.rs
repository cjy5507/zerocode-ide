//! The wire sessions' doors: start, read, send, answer, steer, stop
//! (`wire_runtime`; docs/design/agent-wire-sessions-20260915.md §3).

use crate::*;

/// What a started wire hands the page: its id, and what it learned in the
/// handshake.
#[derive(Serialize)]
pub(crate) struct WireStarted {
    pub(crate) id: wire_runtime::WireId,
    pub(crate) agent: String,
    pub(crate) protocol: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// The conversation it continues, when it was opened over one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
}

/// `wire:update` — the session whose state the page should read now.
#[derive(Serialize, Clone)]
struct WireUpdate {
    id: wire_runtime::WireId,
}

/// Start `agent` on its wire in `cwd`. The executable is the one the agent
/// list itself found (`detected_agents`, `found_as`), resolved on the same
/// PATH — the wire cannot open a program the picker would not have shown.
/// The session tells the window (`wire:update`) each time it has something
/// new to draw, so the page streams instead of waiting for its poll.
///
/// `resume` continues an existing conversation by its session id (the road's
/// own flag); `from_pane` is the pane whose screen session that conversation
/// is — once the wire is up, the pane's CLI is told its own exit command, so
/// one process speaks for the conversation at a time. A worker's pane is
/// refused: the ledger drives it through its screen.
#[tauri::command(async)]
pub(crate) fn wire_start(
    app: AppHandle,
    state: State<'_, AppState>,
    agent: String,
    cwd: String,
    resume: Option<String>,
    from_pane: Option<TermId>,
) -> Result<WireStarted, String> {
    // A refusal before the wire stands is logged beside the hand-over's own
    // receipt: a pane that asked for its conversation and kept its screen
    // must leave a line saying why, not only a toast (2026-09-21 — twenty-two
    // Claude panes, no line, no way to tell).
    // The pane's transcript, read before the pane hands its conversation
    // over: the pictures its history names stand there (t-6323 A8).
    let history = from_pane.and_then(|term| {
        state
            .pane_sessions()
            .get(&term)
            .and_then(|session| session.transcript_path.clone())
    });
    let session = stand_wire(&app, &state, &agent, &cwd, resume.as_deref(), from_pane)
        .inspect_err(|reason| {
            if let Some(term) = from_pane {
                crate::system_runtime::note_window_event(
                    state.local_data_root(),
                    &wire_runtime::refusal_line(term, &agent, resume.as_deref(), reason),
                );
            }
        })?;
    if let Some(path) = history {
        wire_runtime::remember_history(&session, std::path::PathBuf::from(path));
    }
    if let Some(term) = from_pane {
        let outcome = hand_over_pane(&state, term, &agent);
        crate::system_runtime::note_window_event(
            state.local_data_root(),
            &wire_runtime::hand_over_line(term, &agent, resume.as_deref(), session.id, &outcome),
        );
        // A screen that kept the conversation keeps it: the wire that would
        // have spoken for it is stopped here, never left running beside a
        // CLI that never left.
        if let Err(reason) = outcome {
            let _ = state.shell_runtime().wires.stop(session.id);
            return Err(reason);
        }
    }
    let (version, model, protocol) = {
        let held = session
            .state
            .lock()
            .map_err(|_| "wire state lock".to_string())?;
        let log = wire_runtime::log_of(&session, 0)?;
        (held.version.clone(), held.model.clone(), log.protocol)
    };
    Ok(WireStarted {
        id: session.id,
        agent,
        protocol,
        version,
        model,
        session: resume,
    })
}

/// Everything between the ask and a wire that stands: the pane's own gates
/// (a worker's seat, an exit command), the catalog, the PATH, the account's
/// door, and the child's handshake. One `Err` for every reason, so the
/// caller can log it once.
fn stand_wire(
    app: &AppHandle,
    state: &State<'_, AppState>,
    agent: &str,
    cwd: &str,
    resume: Option<&str>,
    from_pane: Option<TermId>,
) -> Result<Arc<wire_runtime::WireSession>, String> {
    if let Some(term) = from_pane {
        if state.workers().values().any(|(seat, _)| *seat == term) {
            return Err(
                "a worker's pane stays a pane: the ledger drives it through its screen".into(),
            );
        }
        if zerocode_core::agent::agent_voice(agent)
            .exit_command
            .is_none()
        {
            return Err(format!("{agent} has no exit command this window knows"));
        }
    }
    let found = scm_runtime::detected_agents(false)
        .into_iter()
        .find(|row| row.id == agent)
        .ok_or_else(|| format!("{agent} is not in the catalog"))?;
    let name = found
        .found_as
        .ok_or_else(|| format!("{} is not installed here", found.name))?;
    let path = shell_path::launch_path();
    let binary = zerocode_core::agent::resolve_on_path(path.as_deref(), &name)
        .ok_or_else(|| format!("{name} is not on the shell's PATH"))?;
    // Which account it runs as — the door every launch of this agent takes.
    let launch_env = account_env_for(state.config_root(), agent)?;
    let app = app.clone();
    state.shell_runtime().wires.start(
        agent,
        &binary,
        Path::new(cwd),
        env!("CARGO_PKG_VERSION"),
        resume,
        &launch_env,
        move |id| {
            let _ = app.emit("wire:update", WireUpdate { id });
        },
    )
}

/// How long the pane's CLI gets to leave after its exit command. A TUI
/// unmounts in well under a second; the wait is for the machine, not the
/// program — and a screen still standing past it is an answer, not a delay.
/// The account switch waits the same time for a closed pane's process group
/// before it resumes the conversation elsewhere (t-7538).
pub(crate) const HAND_OVER_EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(8);
/// How often the wait looks for the pane's leaving.
pub(crate) const HAND_OVER_EXIT_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// Wait until no process of the group `root` leads is left, up to `wait`,
/// looking every `poll`. A pane's child is spawned through `setsid`
/// (`zerocode_pty`), so its pid is its group's id and the agent CLI —
/// whether the child itself or a shell's child — is in that group. Answers
/// whether the group is gone.
///
/// Windows has no process groups to ask; the close there is the kill, and
/// the answer is yes.
pub(crate) fn wait_process_group_gone(
    root: u32,
    wait: std::time::Duration,
    poll: std::time::Duration,
) -> bool {
    #[cfg(unix)]
    {
        let began = std::time::Instant::now();
        while crate::codex_queue::process_group_exists(root) {
            if began.elapsed() >= wait {
                return false;
            }
            std::thread::sleep(poll);
        }
        true
    }
    #[cfg(not(unix))]
    {
        let _ = (root, wait, poll);
        true
    }
}

/// Whether the program a pane's close left behind has left since (t-7538,
/// astra R3): no process of its group is left, or the pid that led it now
/// names another program — a pid is not handed out again while a group of
/// that id still has members, so a new leader under it means the group
/// ended. A group whose leader is gone and whose members remain is still
/// the program's; and whatever the process table cannot say answers "not
/// yet", so the next look asks again rather than a restore guessing.
///
/// Windows has no process groups to ask; the close there is the kill.
pub(crate) fn program_left(witness: &crate::agent_teams::ExitWitness) -> bool {
    #[cfg(unix)]
    {
        if !crate::codex_queue::process_group_exists(witness.group) {
            return true;
        }
        match (
            witness.started.as_deref(),
            crate::resource_usage::process_start_identity(witness.group),
        ) {
            (Some(started), Ok(now)) => now != started,
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = witness;
        true
    }
}

/// End the pane's screen session with the CLI's own exit command, typed the
/// way a person types a line (`ask::line_keys`: the text, then its Enter —
/// the walk `answer_ask` takes), and WAIT for the screen to leave: the pane
/// retires itself when its child exits, so the terminal's absence is the
/// receipt. Ok carries how long the leaving took; Err says the screen kept
/// the conversation — and the caller stops the wire it started for it.
fn hand_over_pane(
    state: &State<'_, AppState>,
    term: TermId,
    agent: &str,
) -> Result<std::time::Duration, String> {
    let exit = zerocode_core::agent::agent_voice(agent)
        .exit_command
        .ok_or_else(|| format!("{agent} has no exit command this window knows"))?;
    if !state.terminals().contains_key(&term) {
        return Err("터미널이 떠 있지 않습니다".to_string());
    }
    let (groups, step) = zerocode_core::ask::line_keys(exit);
    let started = std::time::Instant::now();
    with_terminal(state, term, |pty| {
        pty.terminal_mut().grid_mut().view_to_bottom();
        Ok(())
    })?;
    crate::cmd::terminal::walk_key_groups(state, term, &groups, step, || true)?;
    while state.terminals().contains_key(&term) {
        if started.elapsed() >= HAND_OVER_EXIT_WAIT {
            return Err(format!(
                "the pane's CLI kept its screen {}s after `{exit}` — the conversation stays on the screen",
                HAND_OVER_EXIT_WAIT.as_secs()
            ));
        }
        std::thread::sleep(HAND_OVER_EXIT_POLL);
    }
    Ok(started.elapsed())
}

/// The session's turns from `after` (a turn number) on, with its status,
/// open questions, model, mode and commands — the pane log's shape, so the
/// page reads a wire the way it reads a transcript.
#[tauri::command(async)]
pub(crate) fn wire_log(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
    after: u64,
) -> Result<wire_runtime::WireLog, String> {
    let session = state.shell_runtime().wires.get(id)?;
    wire_runtime::log_of(&session, after)
}

/// One picture of a session's page, by its place — a key the wire kept a
/// tool's picture under (`wire:<n>`), or a place in the transcript of the
/// pane the session continues, read through the pane door's own check (base64
/// alone): its base64, for the page to draw when it comes into view (t-6323
/// A8). The window names a session, never a path.
#[tauri::command(async)]
pub(crate) fn wire_image(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
    at: String,
) -> Result<String, String> {
    let session = state.shell_runtime().wires.get(id)?;
    match wire_runtime::image_of(&session, &at)? {
        wire_runtime::WireImage::Held(payload) => Ok(payload),
        wire_runtime::WireImage::InFile(path) => crate::cmd::terminal::payload_at(&path, &at),
    }
}

#[tauri::command(async)]
pub(crate) fn wire_send(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
    text: String,
) -> Result<(), String> {
    state.shell_runtime().wires.get(id)?.send(&text)
}

/// Answer one open question: the option picked (an approval), or the answers
/// per question (a `request_user_input`). `None` for the option dismisses.
///
/// `message` rides a refusal where the protocol carries one — the plan card's
/// feedback field, which Claude Code's denial shows the model as its reason.
#[tauri::command(async)]
pub(crate) fn wire_answer(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
    ask: serde_json::Value,
    option: Option<String>,
    answers: Option<Vec<Vec<String>>>,
    message: Option<String>,
) -> Result<(), String> {
    state.shell_runtime().wires.get(id)?.answer(
        &ask,
        option.as_deref(),
        &answers.unwrap_or_default(),
        message.as_deref(),
    )
}

#[tauri::command(async)]
pub(crate) fn wire_interrupt(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
) -> Result<(), String> {
    state.shell_runtime().wires.get(id)?.interrupt()
}

#[tauri::command(async)]
pub(crate) fn wire_models(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
) -> Result<Vec<wire_runtime::WireModel>, String> {
    state.shell_runtime().wires.get(id)?.models()
}

#[tauri::command(async)]
pub(crate) fn wire_set_model(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
    model: String,
) -> Result<(), String> {
    state.shell_runtime().wires.get(id)?.set_model(&model)
}

#[tauri::command(async)]
pub(crate) fn wire_set_mode(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
    mode: String,
) -> Result<(), String> {
    state.shell_runtime().wires.get(id)?.set_mode(&mode)
}

#[tauri::command(async)]
pub(crate) fn wire_stop(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
) -> Result<(), String> {
    state.shell_runtime().wires.stop(id)
}
