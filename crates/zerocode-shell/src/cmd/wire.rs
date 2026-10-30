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
///
/// `deadline_ms` is the moment (epoch ms) past which the window no longer
/// waits for this wire (t-84670): it gives what was sent meanwhile to the
/// pane. A wire that stands after it is stopped and refused — it never opens
/// beside a pane the words went to, nor tells that pane's CLI to exit.
#[tauri::command(async)]
pub(crate) fn wire_start(
    app: AppHandle,
    state: State<'_, AppState>,
    agent: String,
    cwd: String,
    resume: Option<String>,
    from_pane: Option<TermId>,
    deadline_ms: Option<i64>,
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
    // A wire that stood after the window stopped holding for it never takes
    // the conversation: it is stopped here, before anything is remembered or
    // typed, and the window keeps what it shows (t-84670).
    if hand_over_overdue(deadline_ms, epoch_ms_now()) {
        let _ = state.shell_runtime().wires.stop(session.id);
        if let Some(term) = from_pane {
            crate::system_runtime::note_window_event(
                state.local_data_root(),
                &wire_runtime::refusal_line(term, &agent, resume.as_deref(), HAND_OVER_OVERDUE),
            );
        }
        return Err(HAND_OVER_OVERDUE.to_string());
    }
    if let Some(path) = history.as_deref() {
        wire_runtime::remember_history(&session, std::path::PathBuf::from(path));
    }
    if let Some(term) = from_pane {
        let outcome = hand_over_pane(
            &app,
            &state,
            term,
            &agent,
            &session,
            history.as_deref(),
            &cwd,
        );
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

/// Why a pane's conversation is never handed to a wire: the pane is one the
/// ledger drives through its screen. A worker's seat; and a run's
/// coordinator seat (t-84850) — the ledger brings that run's mail to its
/// screen, and a coordinator whose CLI left would leave its run without the
/// seat the ledger knows it by. The coordinators' seats are asked only when
/// the pane is no worker's, and read from the ledger as they stand now
/// (`orchestration::seated_coordinators_read`), never from a seat the window
/// remembered: a restart moves them. A ledger that cannot be read says
/// nothing of whose pane this is, and the pane stays (t-84850 r2). `None` for
/// a person's own pane.
pub(crate) fn pane_the_ledger_drives(
    term: TermId,
    workers: impl IntoIterator<Item = TermId>,
    coordinators: impl FnOnce() -> Option<Vec<crate::orchestration::restart_census::KeyedPane>>,
) -> Option<&'static str> {
    if workers.into_iter().any(|seat| seat == term) {
        return Some(WORKER_PANE_STAYS);
    }
    let Some(coordinators) = coordinators() else {
        return Some(LEDGER_UNREAD_PANE_STAYS);
    };
    coordinators
        .iter()
        .any(|seated| seated.term == term)
        .then_some(COORDINATOR_PANE_STAYS)
}

/// The refusals [`pane_the_ledger_drives`] gives, in the hand-over's log line
/// and on the pane's page.
pub(crate) const WORKER_PANE_STAYS: &str =
    "a worker's pane stays a pane: the ledger drives it through its screen";
pub(crate) const COORDINATOR_PANE_STAYS: &str =
    "a coordinator's pane stays a pane: the ledger brings its run's mail to that screen";
pub(crate) const LEDGER_UNREAD_PANE_STAYS: &str =
    "the ledger cannot be read now, so this pane may be a coordinator's: it stays a pane";

/// What the window knows of a pane's CLI just before its exit command would
/// be typed (t-84850 r2) — each read from what the window already holds,
/// never from a program's name.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneCli<'a> {
    /// The pane's own child: the process the pane was spawned as.
    pub(crate) child: Option<u32>,
    /// Whether that child leads the terminal's foreground group. A CLI a
    /// person started from the pane's shell leads a group of its own.
    pub(crate) child_in_front: Option<bool>,
    /// What this window spawned as the pane's agent, as written down at the
    /// spawn (`pane_leftovers::launched`): its pid and the agent.
    pub(crate) launched: Option<(u32, &'a str)>,
    /// Whether the pane's program has shown it reads its input since the
    /// pane began (`human_input::reads_input`): a prompt it reported taking —
    /// the receipt a delivery's Enter waits for — or the hand-over's probe
    /// shown on its line.
    pub(crate) reads_input: bool,
    /// What the pane's agent last said of itself through its hooks; `None`
    /// when it has said nothing.
    pub(crate) state: Option<zerocode_core::hook::HookState>,
    /// Whether a helper of the pane's agent still runs, or its latest
    /// background roll call named other work still running (a shell).
    pub(crate) background_work: bool,
    /// Whether the screen shows the agent's input line empty
    /// (`composer_empty::observed`) — positive evidence, never a guess.
    pub(crate) line_shown_empty: bool,
}

/// Why a pane's CLI is not told its exit command (t-84850 r2), or `None` when
/// it may be. Anything the window cannot show refuses: the exit is typed only
/// when it will end the pane and nothing with it.
///
/// - It has to end the pane: only the agent this window spawned as the
///   pane's own child, still in front of the terminal, leaves nothing behind
///   — a CLI started from the pane's shell leaves that shell standing, and
///   the words that waited would be typed at it.
/// - It has to be at rest, with no helper or background shell the exit would
///   end, and an empty input line for `/exit` to stand alone on.
/// - It has to read the line: a CLI that has shown no reading since it began
///   may be one still waking (a resumed Claude Code reads nothing for up to a
///   minute), and `/exit` would sit on its input line for the words after it
///   to join. Asked last, because the one answer the window can still get —
///   the probe ([`pane_cli_refusal`]) — types at the pane, and is typed only
///   where everything else already holds.
pub(crate) fn pane_cli_cannot_leave(agent: &str, cli: &PaneCli<'_>) -> Option<&'static str> {
    use zerocode_core::hook::HookState;
    let spawned_here = cli
        .launched
        .is_some_and(|(pid, launched)| cli.child == Some(pid) && launched == agent);
    if !spawned_here || cli.child_in_front != Some(true) {
        return Some(SHELL_PANE_STAYS);
    }
    if !matches!(cli.state, Some(HookState::Done | HookState::Idle)) {
        return Some(PANE_NOT_AT_REST);
    }
    if cli.background_work {
        return Some(PANE_BACKGROUND_WORK);
    }
    if !cli.line_shown_empty {
        return Some(PANE_LINE_HOLDS_WORDS);
    }
    (!cli.reads_input).then_some(PANE_NOT_READING_YET)
}

/// The refusals [`pane_cli_cannot_leave`] gives, beside the ledger's.
pub(crate) const SHELL_PANE_STAYS: &str =
    "a CLI started from the pane's shell stays in the pane: the terminal is its live view";
pub(crate) const PANE_NOT_READING_YET: &str = "the pane's CLI did not show a key typed at it, so it is not reading its input yet: send one message on the pane, then ask again";
pub(crate) const PANE_NOT_AT_REST: &str = "the pane's CLI is not at rest — a turn, a question or an approval is open: try again once it rests";
pub(crate) const PANE_BACKGROUND_WORK: &str =
    "the pane's CLI still runs background work its exit would end: try again once it is done";
pub(crate) const PANE_LINE_HOLDS_WORDS: &str =
    "the pane's input line is not shown empty — words may be written there: nothing was typed";

/// The refusal that comes after the exit command was typed (t-84850 r2): the
/// pane still stands, so its CLI may yet read the command, or have left a
/// shell behind. The window gives the words that waited back to its input box
/// rather than to the pane, and knows this refusal by these opening words
/// (`WIRE_EXIT_TYPED` in the window).
pub(crate) const EXIT_TYPED_PANE_STANDS: &str =
    "the exit command was typed at the pane and the pane still stands";

/// How long the probe looks for the space it typed to show on the pane's
/// field (t-84850 r2). A CLI that reads its input draws a key well inside it;
/// one that does not draw it by then is not reading yet.
pub(crate) const PROBE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
/// How long a probe that did not show in time is still looked for, to be taken
/// away when a waking CLI draws it late — unless a person's hand reaches the
/// line first, whose words then follow it.
pub(crate) const PROBE_LATE_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);

/// [`pane_cli_cannot_leave`] for the pane as it stands now: its child, the
/// terminal's foreground and its screen read in one hold of the pane's lock,
/// what this window launched there, what its hooks last said, and whether its
/// program has shown it reads. A pane that has shown no reading yet, and
/// passes everything else, is asked by the probe (`answer_door::probe_reading`):
/// one space typed at its empty field, erased once it shows. One that does not
/// show it is refused and typed nothing more; the space is looked for a while
/// longer and erased if it shows late.
fn pane_cli_refusal(state: &State<'_, AppState>, term: TermId, agent: &str) -> Result<(), String> {
    let held = state
        .terminals()
        .handle(term)
        .ok_or("터미널이 떠 있지 않습니다")?;
    let (child, child_in_front, line_shown_empty) = {
        let pty = lock_pty(&held);
        (
            pty.pid(),
            pty.foreground_is_child(),
            crate::composer_empty::observed(agent, pty.terminal().grid()),
        )
    };
    let launched = crate::pane_leftovers::launched_as(term);
    let helpers = state.subagents().get(&term).is_some_and(|rows| {
        rows.iter()
            .any(|row| row.state == hooks::SubagentState::Running)
    });
    let said = state.pane_states().get(&term).map(|said| said.state);
    let cli = PaneCli {
        child,
        child_in_front,
        launched: launched
            .as_ref()
            .map(|(pid, launched)| (*pid, launched.as_str())),
        reads_input: crate::human_input::reads_input(term),
        state: said,
        background_work: helpers || hooks::other_background_work(term),
        line_shown_empty,
    };
    match pane_cli_cannot_leave(agent, &cli) {
        None => Ok(()),
        Some(PANE_NOT_READING_YET) => {
            use crate::answer_door::Probe;
            let probed =
                crate::answer_door::probe_reading(&held, agent, PROBE_WAIT, HAND_OVER_EXIT_POLL)
                    .map_err(|error| error.to_string())?;
            state.cadence().wake();
            match probed {
                Probe::Read => {
                    crate::human_input::saw_reading(term);
                    Ok(())
                }
                Probe::Deaf => {
                    erase_a_late_probe(held, term, agent.to_string());
                    Err(PANE_NOT_READING_YET.to_string())
                }
                Probe::NotTyped => Err(PANE_LINE_HOLDS_WORDS.to_string()),
            }
        }
        Some(reason) => Err(reason.to_string()),
    }
}

/// Look for a probe that did not show in time, up to [`PROBE_LATE_LIMIT`], and
/// take it away when it shows — on a thread of its own, so the refusal is not
/// held for it. It stops the moment a person's hand reaches the line (their
/// words follow the space, which is harmless there) or the pane goes.
fn erase_a_late_probe(held: crate::terminal_registry::HeldTerminal, term: TermId, agent: String) {
    let hand = crate::human_input::line_of(term).1;
    let _ = std::thread::Builder::new()
        .name(format!("hand-over-probe-{term}"))
        .spawn(move || {
            let began = std::time::Instant::now();
            while began.elapsed() < PROBE_LATE_LIMIT
                && std::sync::Arc::strong_count(&held) > 1
                && crate::human_input::line_of(term).1 == hand
            {
                if crate::answer_door::erase_probe(&held, &agent).unwrap_or(true) {
                    return;
                }
                std::thread::sleep(HAND_OVER_EXIT_POLL);
            }
        });
}

/// Why the wire is not trusted to carry the conversation once the pane's CLI
/// leaves (t-84850 r2 P1), or `None`. Its handshake reads only the CLI's
/// `--help` and `--version`, so a wire that stands has not yet shown it holds
/// the conversation: its child must still run, and the conversation's record
/// must be on disk in the store Claude Code keeps for the wire's folder — the
/// one its `--resume` looks in.
pub(crate) fn wire_cannot_take(
    running: bool,
    transcript: Option<&Path>,
    on_disk: bool,
    cwd: &Path,
) -> Option<&'static str> {
    if !running {
        return Some(WIRE_NOT_RUNNING);
    }
    let Some(transcript) = transcript.filter(|_| on_disk) else {
        return Some(RECORD_NOT_ON_DISK);
    };
    (!transcript_in_store_of(transcript, cwd)).then_some(RECORD_IN_ANOTHER_STORE)
}

/// The refusals [`wire_cannot_take`] gives.
pub(crate) const WIRE_NOT_RUNNING: &str =
    "the wire's CLI is not running: the pane keeps the conversation";
pub(crate) const RECORD_NOT_ON_DISK: &str = "the conversation's record is not on disk yet, so the wire cannot continue it: the pane keeps it";
pub(crate) const RECORD_IN_ANOTHER_STORE: &str = "the conversation's record is kept for another folder than the wire's, so the wire cannot continue it: the pane keeps it";

/// Whether `transcript` stands in the store folder Claude Code keeps for
/// `cwd` — either spelling of its flattened path
/// (`scm_runtime::claude_project_slugs`), for the folder as named or as the
/// file system resolves it.
pub(crate) fn transcript_in_store_of(transcript: &Path, cwd: &Path) -> bool {
    let Some(store) = transcript
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    let resolved = std::fs::canonicalize(cwd).ok();
    [Some(cwd), resolved.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(Path::to_str)
        .any(|folder| {
            crate::scm_runtime::claude_project_slugs(folder)
                .iter()
                .any(|slug| slug == store)
        })
}

/// Everything between the ask and a wire that stands: the pane's own gates
/// (a worker's or a coordinator's seat, an exit command, a CLI whose exit
/// ends the pane and that reads its input), the catalog, the PATH, the
/// account's door, and the child's handshake. One `Err` for every
/// reason, so the caller can log it once.
fn stand_wire(
    app: &AppHandle,
    state: &State<'_, AppState>,
    agent: &str,
    cwd: &str,
    resume: Option<&str>,
    from_pane: Option<TermId>,
) -> Result<Arc<wire_runtime::WireSession>, String> {
    if let Some(term) = from_pane {
        let workers: Vec<TermId> = state.workers().values().map(|(seat, _)| *seat).collect();
        if let Some(reason) = pane_the_ledger_drives(
            term,
            workers,
            crate::orchestration::seated_coordinators_read,
        ) {
            return Err(reason.into());
        }
        if zerocode_core::agent::agent_voice(agent)
            .exit_command
            .is_none()
        {
            return Err(format!("{agent} has no exit command this window knows"));
        }
        // Asked before any wire starts, so a pane that cannot hand over is
        // refused at once and what waited goes to it straight.
        pane_cli_refusal(state, term, agent)?;
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
    let page = app.clone();
    let app = app.clone();
    state.shell_runtime().wires.start(
        agent,
        &binary,
        Path::new(cwd),
        env!("CARGO_PKG_VERSION"),
        resume,
        &launch_env,
        wire_runtime::WireNews {
            changed: Box::new(move |id| {
                let _ = page.emit("wire:update", WireUpdate { id });
            }),
            // The tool calls the session said, on the road every pane's
            // travel — the file tree hears a wire session's writes (t-31715).
            acted: Box::new(move |id, activities| {
                pane_runtime::note_wire_activities(&app, id, activities);
            }),
        },
    )
}

/// Why a wire that stood after the window's deadline was stopped (t-84670) —
/// the refusal its log line and the page carry.
const HAND_OVER_OVERDUE: &str =
    "the wire stood after the window stopped waiting for it; the pane keeps the conversation";

/// Whether a hand-over the window gave up on at `deadline_ms` is past it at
/// `now_ms` — no deadline is never past.
pub(crate) fn hand_over_overdue(deadline_ms: Option<i64>, now_ms: i64) -> bool {
    deadline_ms.is_some_and(|deadline| now_ms >= deadline)
}

/// How long a pane's CLI usually takes to leave after its exit command. A
/// TUI unmounts in well under a second; the wait is for the machine, not the
/// program. The account switch waits this long for a closed pane's process
/// group before it resumes the conversation elsewhere (t-7538); the
/// hand-over watches longer ([`HAND_OVER_LEAVE_LIMIT`]), for a CLI that
/// reads its line late leaves anyway.
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
    #[cfg(windows)]
    {
        let began = std::time::Instant::now();
        loop {
            if crate::resource_usage::process_exited(root) == Some(true) {
                return true;
            }
            if began.elapsed() >= wait {
                return false;
            }
            std::thread::sleep(poll);
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (root, wait, poll);
        false
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
/// Windows reads the native child exit status; requesting a kill is not an
/// exit receipt. Unknown observations remain unconfirmed on every platform.
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
    #[cfg(windows)]
    {
        match crate::resource_usage::process_exited(witness.group) {
            Some(true) => true,
            Some(false) => match (
                witness.started.as_deref(),
                crate::resource_usage::process_start_identity(witness.group),
            ) {
                (Some(started), Ok(now)) => now != started,
                _ => false,
            },
            None => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = witness;
        false
    }
}

/// How long the hand-over watches for the pane to leave once its exit command
/// is typed (t-84850 r2 P2-5). A CLI past its usual moment
/// ([`HAND_OVER_EXIT_WAIT`]) may still be reading the line late, and ending
/// the wire then would leave the conversation on neither; the window holds
/// what was sent meanwhile for this whole watch (`wire:exit-typed`) and gives
/// it back to the box, never to the pane, if the pane outlasts it.
pub(crate) const HAND_OVER_LEAVE_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// `wire:exit-typed` — the exit command's first key reached the pane: from
/// here the window holds what is sent with no clock of its own, and never
/// types it at the pane (t-84850 r2).
#[derive(Serialize, Clone)]
struct ExitTyped {
    term: TermId,
}

/// End the pane's screen session with the CLI's own exit command, typed the
/// way a person types a line (`ask::line_keys`: the text, then its Enter —
/// the walk `answer_ask` takes), and WAIT for the screen to leave: the pane
/// retires itself when its child exits, so the terminal's absence is the
/// receipt. Ok carries how long the leaving took; Err says the screen kept
/// the conversation — and the caller stops the wire it started for it. An
/// Err from the first typed key on opens with [`EXIT_TYPED_PANE_STANDS`].
///
/// Before the first key, the pane is asked again ([`pane_cli_refusal`]) and
/// the wire too ([`wire_cannot_take`]): either may have changed while the
/// wire's handshake ran.
fn hand_over_pane(
    app: &AppHandle,
    state: &State<'_, AppState>,
    term: TermId,
    agent: &str,
    wire: &wire_runtime::WireSession,
    history: Option<&str>,
    cwd: &str,
) -> Result<std::time::Duration, String> {
    let exit = zerocode_core::agent::agent_voice(agent)
        .exit_command
        .ok_or_else(|| format!("{agent} has no exit command this window knows"))?;
    if !state.terminals().contains_key(&term) {
        return Err("터미널이 떠 있지 않습니다".to_string());
    }
    let record = history.map(Path::new);
    if let Some(reason) = wire_cannot_take(
        wire.running(),
        record,
        record.is_some_and(Path::is_file),
        Path::new(cwd),
    ) {
        return Err(reason.to_string());
    }
    pane_cli_refusal(state, term, agent)?;
    let (groups, step) = zerocode_core::ask::line_keys(exit);
    let started = std::time::Instant::now();
    with_terminal(state, term, |pty| {
        pty.terminal_mut().grid_mut().view_to_bottom();
        Ok(())
    })?;
    // The exit command is a line the window types by itself, and a menu takes
    // its Enter for the highlighted row — which may be a permission nobody
    // gave; and a line that holds words would send them with it. So its first
    // key goes through the answer door, which types it only while the screen
    // shows no menu and the input line empty; the rest is the walk every line
    // takes.
    let Some((first, rest)) = groups.split_first() else {
        return Err(format!("{agent}'s exit command has nothing to type"));
    };
    let held = state
        .terminals()
        .handle(term)
        .ok_or("터미널이 떠 있지 않습니다")?;
    let typed = crate::answer_door::type_if_up(
        &held,
        &crate::answer_door::Expect::no_menu().with_empty_line(agent),
        &crate::cmd::terminal::key_group_bytes(first),
    )
    .map_err(|error| error.to_string())?;
    state.cadence().wake();
    if !typed {
        return Err(
            "the pane stands on a menu, or its input line is not shown empty — nothing was typed"
                .to_string(),
        );
    }
    let _ = app.emit("wire:exit-typed", ExitTyped { term });
    std::thread::sleep(step);
    // From the first key on, every refusal says the exit command was typed:
    // the window must not type the words that waited after it.
    crate::cmd::terminal::walk_key_groups(state, term, rest, step).map_err(|error| {
        format!("{EXIT_TYPED_PANE_STANDS}: `{exit}` stopped part way ({error})")
    })?;
    while state.terminals().contains_key(&term) {
        if started.elapsed() >= HAND_OVER_LEAVE_LIMIT {
            return Err(format!(
                "{EXIT_TYPED_PANE_STANDS} {}s after `{exit}` — check the pane, then send again",
                HAND_OVER_LEAVE_LIMIT.as_secs()
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

/// What stands above a session's page (t-82901): the turns of the transcript
/// of the pane this session continues whose lines end at `before` — the
/// page's 「이전 기록 더 보기」, read back down the pane door's own road and
/// marked as it marks them. The window names a session, never a path; a
/// session that continues no pane has nothing above its first turn.
#[tauri::command(async)]
pub(crate) fn wire_history(
    state: State<'_, AppState>,
    id: wire_runtime::WireId,
    before: u64,
) -> Result<SubagentLog, String> {
    let session = state.shell_runtime().wires.get(id)?;
    let Some(path) = wire_runtime::history_of(&session) else {
        return Ok(SubagentLog::not_found(before));
    };
    let mut log = crate::cmd::terminal::transcript_log_at(
        &path,
        crate::cmd::terminal::LogRead::Before(before),
    )?;
    restart_nudge_runtime::mark_window_notes(log.turns.iter_mut().map(|one| &mut one.turn));
    Ok(log)
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
    state.shell_runtime().wires.stop(id)?;
    // The session's card has nothing more to say, and its ring goes with it
    // (t-31715): a card nothing forgets is a leak wearing a feature's clothes.
    state.activities().remove(&hooks::activity_wire(id));
    Ok(())
}
