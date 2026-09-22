//! Terminal commands.

use crate::*;
use zerocode_core::agent::{Injected, prompt_injection};
use zerocode_core::capabilities::SpawnRoad;

/// Open the floating panel's shell, if it is not already running.
///
/// A PLAIN shell, whatever the new-terminal command says. Orca's floating
/// workspace never auto-starts an agent — the panel opens onto an empty state
/// (`isEmptyFloatingWorkspacePanelVisible`,
/// remote-runtime-pty-recovery-state-DSp6CaQ4.js) and only an explicit ⌘T
/// inside it creates a tab. The one terminal this panel auto-opens is the
/// quick shell beside the work, and was asked for in exactly those words:
/// "claude 실행이 아니고 터미널이 실행되게 해야함". No agent recording
/// either — a shell born plain was not born an agent; one STARTED in it by
/// hand is recorded by its own hook, the road every hand-started agent takes.
/// The answer is the seat's header label ([`floating_seat_label`]): where the
/// shell REALLY sits. It seats at the floating-workspace preference — Orca's
/// contract, where the panel is a global surface whose directory the settings
/// page owns (default `~`), not a view of the active worktree — and the
/// already-running branch answers the seat remembered at spawn, because the
/// preference may have moved since and a header that follows the setting
/// around while the shell stays put is the small lie that costs an hour.
#[tauri::command(async)]
pub(crate) fn open_terminal(
    state: State<'_, AppState>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    if state.terminals().contains_key(&FLOAT_TERM) {
        return Ok(state.float_seat().clone().unwrap_or_default());
    }
    let prefs = load_settings_for_boot(state.settings())
        .document
        .floating_workspace;
    let seat = resolved_floating_workspace_seat(&prefs)?;
    let label = floating_seat_label(&seat);
    let (pty, _program) = spawn_shell(
        &state,
        FLOAT_TERM,
        rows,
        cols,
        ShellStartup::Plain,
        Some(seat),
    )?;
    state.hold_terminal(FLOAT_TERM, pty);
    *state.float_seat() = Some(label.clone());
    // Same reason as a terminal tab: the shell prints its prompt the moment it
    // starts, and nobody typed to earn a wake. Left out, the first thing this
    // panel ever shows waits out a whole idle nap.
    state.cadence().wake();
    Ok(label)
}

/// Open a shell of its own for a new terminal tab, and say which it is.
///
/// Orca's centre opens terminals as tabs and each one is a separate process
/// (docs/reverse/orca-ui-inventory.md 1-e). The id comes back because the view
/// has to address frames and keystrokes to this shell and no other — several
/// are alive at once, which is the whole point.
#[tauri::command(async)]
pub(crate) fn open_term_tab(
    state: State<'_, AppState>,
    rows: u16,
    cols: u16,
    plain: Option<bool>,
    cwd: Option<String>,
    // A restored leaf's closed-window screen, put back before the shell's
    // first byte is parsed (`replay_stored_screen`).
    restore: Option<super::settings::StoredScreen>,
) -> Result<TermId, String> {
    // 트리의 "Open in Terminal"(Orca :204-212)이 앉힐 자리 — 담장 안의
    // 디렉터리만. 인자가 없으면 지금까지처럼 활성 루트에 앉는다.
    let seat = match cwd {
        Some(asked) => {
            let seat = PathBuf::from(&asked);
            if !file_tree_ops::inside_root(&state.active_root(), &seat) {
                return Err("워크스페이스 밖의 경로입니다".into());
            }
            if !seat.is_dir() {
                return Err("디렉터리가 아닙니다".into());
            }
            Some(seat)
        }
        None => None,
    };
    // The id is taken BEFORE the spawn: the shell's own environment has to
    // carry the pane key it will report under, and a key minted after the child
    // exists is a key the child never saw.
    let id = state.take_term_id();
    let startup = if plain.unwrap_or(false) {
        ShellStartup::Plain
    } else {
        ShellStartup::Configured
    };
    let (mut pty, program) = spawn_shell(&state, id, rows, cols, startup, seat)?;
    if let Some(screen) = &restore {
        super::settings::replay_stored_screen(state.config_root(), screen, pty.terminal_mut());
    }
    state.hold_terminal(id, pty);
    // A terminal whose configured command IS an agent was an agent session
    // from its first frame — the send menu should know without waiting for
    // anything to be typed.
    if let Some(agent) = agent_for_program(&program) {
        state.agent_terms().insert(id, agent);
    }
    // A shell prints its prompt the moment it starts, and that first frame is
    // the one a person is waiting on.
    state.cadence().wake();
    Ok(id)
}

/// A helper's conversation, from `after` onward.
///
/// The page a person opens on a helper row. Orca has no such page — its
/// helpers are rows and nothing else, because a helper has no pty of its own
/// and its output IS its parent's. That is true here too, and this is the
/// other half of the truth: the VENDOR keeps the helper's own transcript on
/// disk, so the conversation exists even though no terminal ever showed it.
///
/// Complete lines only, and never more than [`SUBAGENT_LOG_CHUNK`] at once:
/// the file is being appended to while it is read, and half a line is not yet
/// a fact.
#[tauri::command(async)]
pub(crate) fn subagent_log(
    state: State<'_, AppState>,
    term: TermId,
    id: String,
    after: Option<u64>,
) -> Result<SubagentLog, String> {
    if !is_subagent_id(&id) {
        return Err("helper id가 아닙니다".to_string());
    }
    let missing = || SubagentLog {
        model: None,
        turns: Vec::new(),
        next: after.unwrap_or_default(),
        found: false,
        skipped: false,
        more: false,
        folded: false,
        usage: None,
    };
    // Where the helper's conversation is. The vendor may have said so itself —
    // zo names each running helper's session file in its `subagents` frame and
    // the row remembers it — and failing that, Claude Code's helpers live under
    // the pane's own transcript, the path the agent reported when it started.
    let named = state
        .subagents()
        .get(&term)
        .and_then(|rows| rows.iter().find(|row| row.id == id))
        .and_then(|row| row.transcript.clone());
    let path = match named {
        Some(path) => path,
        None => {
            let Some(transcript) = state
                .pane_sessions()
                .get(&term)
                .and_then(|session| session.transcript_path.clone())
            else {
                return Ok(missing());
            };
            let root = subagent_transcript_root(&transcript);
            let Some(path) = find_subagent_transcript(&root, &id, 3) else {
                return Ok(missing());
            };
            path
        }
    };
    transcript_log_at(&path, after)
}

/// A pane's own conversation, out of the transcript its agent reported when
/// it started (`pane_sessions`): the same bounded, complete-line read the
/// helper's page makes, for a terminal turning its screen over to a
/// conversation view. `found: false` where the agent named no transcript —
/// the view says so instead of guessing at a file.
#[tauri::command(async)]
pub(crate) fn pane_log(
    state: State<'_, AppState>,
    term: TermId,
    after: Option<u64>,
) -> Result<SubagentLog, String> {
    let Some(path) = state
        .pane_sessions()
        .get(&term)
        .and_then(|session| session.transcript_path.clone())
    else {
        return Ok(SubagentLog {
            turns: Vec::new(),
            model: None,
            next: after.unwrap_or_default(),
            found: false,
            skipped: false,
            more: false,
            folded: false,
            usage: None,
        });
    };
    transcript_log_at(&path, after)
}

/// One transcript's turns from `after` on — complete lines only, never more
/// than [`SUBAGENT_LOG_CHUNK`] at once, decoded by core's reader. The one
/// road both doors take (a helper's file, a pane's own), so the two pages
/// cannot learn to read the same file differently. No cursor (`None`) asks
/// for the TAIL: the last chunk, from the first whole line inside it, and
/// `folded` says whether anything stood above it.
pub(crate) fn transcript_log_at(
    path: impl AsRef<std::path::Path>,
    after: Option<u64>,
) -> Result<SubagentLog, String> {
    let path = path.as_ref();
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    let (from, folded) = match after {
        // A file that SHRANK was replaced under us; reading on from a stale
        // offset would splice two conversations together.
        Some(after) => (if after > size { 0 } else { after }, false),
        None => (
            size.saturating_sub(SUBAGENT_LOG_CHUNK),
            size > SUBAGENT_LOG_CHUNK,
        ),
    };
    let read = (size - from).min(SUBAGENT_LOG_CHUNK);
    let mut buffer = vec![0u8; usize::try_from(read).unwrap_or_default()];
    use std::io::{Read, Seek};
    let starts_mid_line = if from > 0 {
        file.seek(std::io::SeekFrom::Start(from - 1))
            .map_err(|error| error.to_string())?;
        let mut preceding = [0u8; 1];
        file.read_exact(&mut preceding)
            .map_err(|error| error.to_string())?;
        preceding[0] != b'\n'
    } else {
        false
    };
    file.seek(std::io::SeekFrom::Start(from))
        .map_err(|error| error.to_string())?;
    file.read_exact(&mut buffer)
        .map_err(|error| error.to_string())?;
    let chunk = zerocode_core::transcript::complete_transcript_chunk(
        &buffer,
        starts_mid_line,
        read == SUBAGENT_LOG_CHUNK,
    );
    let text = String::from_utf8_lossy(chunk.bytes);
    let next = from + u64::try_from(chunk.consumed).unwrap_or_default();
    Ok(SubagentLog {
        turns: zerocode_core::transcript::turns_in(&text),
        model: zerocode_core::transcript::model_in(&text),
        next,
        found: true,
        skipped: chunk.skipped,
        more: next < size,
        folded,
        usage: zerocode_core::transcript::usage_in(&text),
    })
}

/// A pane that READS a nested run's mirror as a terminal.
///
/// Whether a nested run can be mirrored at all — see [`hooks::mirror_ready`].
///
/// The window asks once and keeps the answer: a page that has nothing to show
/// while a run is going has to say WHICH silence it is, and the shim's
/// fall-through is silent by design.
#[tauri::command]
pub(crate) fn mirror_ready() -> bool {
    let _crumb = crate::crumbs::Command::enter("mirror_ready");
    hooks::mirror_ready()
}

/// The program is `tail -c +0 -F` over a file the mirror shim is appending —
/// ANSI intact, so the grid draws colour and cursor movement natively, and
/// `-F` keeps reading across the moments the file is still being born. No
/// shell underneath and nothing typed at anything: the pane IS the command,
/// which is what keeps the hidden-Enter rule untouched. The path check is the
/// same stance as `history_hash`: this channel is the webview's, and only
/// files the mirror directory owns may be read through it.
#[tauri::command(async)]
pub(crate) fn open_mirror_term(
    state: State<'_, AppState>,
    path: String,
    rows: u16,
    cols: u16,
) -> Result<TermId, String> {
    let Some((_, mirrors)) = hooks::mirror_shims() else {
        return Err("미러가 준비되지 않았습니다".to_string());
    };
    let held = std::path::PathBuf::from(&path);
    if !held.starts_with(&mirrors) || !held.is_file() {
        return Err("미러 파일이 아닙니다".to_string());
    }
    let id = state.take_term_id();
    let env = hooks::pty_env(&hooks::pane_key_of(id), None, &state.active_root(), None);
    let pty = PtyLane::spawn(
        "tail",
        &["-c".to_string(), "+0".to_string(), "-F".to_string(), path],
        Some(&state.active_root()),
        &env,
        rows,
        cols,
    )
    .map_err(|error| error.to_string())?;
    note_window_event(
        state.local_data_root(),
        &format!("term {id} spawned tail (mirror)"),
    );
    state.hold_terminal(id, pty);
    state.cadence().wake();
    Ok(id)
}

#[tauri::command(async)]
pub(crate) fn term_has_running_process(state: State<'_, AppState>, term: TermId) -> Option<bool> {
    running_process_from_foreground(
        state
            .terminals()
            .handle(term)
            .and_then(|pty| lock_pty(&pty).foreground_is_child()),
    )
}

/// Every plain terminal currently owned by this application process.
///
/// Orca calls this surface Manage Sessions. ZeroCode has no shared PTY daemon,
/// so the process-owned terminal pool is the exact management authority.
#[tauri::command]
pub(crate) async fn terminal_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<ManagedTerminalSession>, String> {
    // Match the established lock order in `agent_terms`: agent facts first,
    // then the terminal pool. No process inspection runs while both are held.
    let agents = state.agent_terms().clone();
    let entries = state.terminals().entries();
    let mut sessions = entries
        .iter()
        .map(|(term, held)| {
            let pty = lock_pty(held);
            managed_terminal_session(
                *term,
                agents.get(term).copied(),
                pty.foreground_is_child(),
                pty.foreground_programs(),
            )
        })
        .collect::<Vec<_>>();
    sessions.sort_unstable_by_key(|session| session.term);
    Ok(sessions)
}

#[tauri::command(async)]
pub(crate) fn end_terminal_session(
    app: AppHandle,
    state: State<'_, AppState>,
    term: TermId,
) -> bool {
    let retired = retire_terminal(&state, term);
    if retired {
        announce_retired_terminal(&app, term);
    }
    retired
}

#[tauri::command(async)]
pub(crate) fn end_all_terminal_sessions(app: AppHandle, state: State<'_, AppState>) -> usize {
    let terms = state.terminals().terms();
    let mut retired = 0;
    for term in terms {
        if retire_terminal(&state, term) {
            announce_retired_terminal(&app, term);
            retired += 1;
        }
    }
    retired
}

/// Close one shell. Closing the tab is what ends the process.
#[tauri::command(async)]
pub(crate) fn close_term(state: State<'_, AppState>, term: TermId) {
    retire_terminal(&state, term);
}

/// Hand a prompt to whatever is running in this shell, once it is listening.
///
/// This is the door an automation drives an agent through, and the reason it
/// waits rather than writing immediately: an agent TUI that has not drawn its
/// input line yet will drop what arrives, or worse, take it as an answer to
/// whatever it *is* showing. The wait, the envelope and the Enter all live in
/// [`zerocode_pty::PromptDelivery`]; this only registers one and lets the pump
/// turn it (docs/reverse/orca-ui-inventory.md 1-ae).
///
/// A prompt sent while another is on the line WAITS ITS TURN. Two envelopes
/// interleaved in one pty read as a single corrupt paste, and refusing the
/// second — what this door did before — turned every double-send in an
/// automation into an error somebody retried by hand (P0-10). Orca
/// serialises per pty for the same reason (native-chat-pty-send-queue.ts:
/// "Each sequence owns the line until its Enter fires").
///
/// Answers as soon as the prompt is accepted, not when it lands — each
/// landing arrives as its own `term:prompt` event, because waiting here would
/// hold a command open for up to eight seconds a turn.
///
/// # Errors
///
/// When no such shell exists.
#[tauri::command]
pub(crate) fn send_prompt(
    state: State<'_, AppState>,
    term: TermId,
    text: String,
    submit: bool,
    agent: Option<String>,
) -> Result<(), String> {
    let _crumb = crate::crumbs::Command::enter("send_prompt");
    // A RUNNING terminal takes a paste, whatever the registry says about
    // native draft doors. `--prefill` is claude's DRAFT flag — a launch-time
    // door — and for a while its existence refused every later send too, so
    // picking the open claude in the send menu answered a refusal (live
    // report 2026-08-14). The launch road owns the command line; a fresh
    // instruction to a running agent is exactly what pasting is for.
    //
    type_prompt_at_term(
        &state,
        term,
        text,
        submit,
        agent.as_deref(),
        PromptReadiness::Resting,
    )
    .map(|_| ())
}

/// Whether this prompt is waiting for a process to mount its first composer
/// or talking to a composer that already owns the terminal — and, when it is
/// the latter, whether this caller is allowed to empty the line first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptReadiness {
    Mounting,
    Resting,
    /// At rest, and whatever is already on the line stays there.
    ///
    /// The clear keys `Resting` may send are how a running composer is emptied
    /// before a new instruction, and for a caller that knows the line is its
    /// own that is right. A door that types at a pane on somebody else's
    /// behalf does not know that: the words in front of a Codex composer may
    /// be half a thought a person is still writing, and clear keys take them
    /// with no undo. So this variant never sends them.
    RestingBesideADraft,
}

impl PromptReadiness {
    /// What this delivery waits to see before it writes anything.
    fn signal(self, agent: Option<&str>) -> ReadySignal {
        match self {
            Self::Mounting => ready_signal_for(agent),
            // Rest keeps the same glyph as a mounting wait and adds the two
            // signs a running composer actually gives: a cursor shown again,
            // or the stream settling into silence.
            Self::Resting | Self::RestingBesideADraft => {
                ReadySignal::Rest(ready_signal_for(agent).marker())
            }
        }
    }

    /// Whether composer edit keys ride ahead of the paste.
    fn clearing(self, agent: Option<&str>) -> bool {
        match self {
            Self::Mounting | Self::Resting => composer_clear_for(agent),
            Self::RestingBesideADraft => false,
        }
    }

    /// What this delivery refuses to write over, decided at the write.
    ///
    /// The same readiness that refuses clear keys is the one that yields to
    /// everything else a person owns on the line: their draft, the question
    /// parked on the pane, and the hand that arrives while a paste settles.
    /// A delivery that owns its line — a launch briefing, a person's own send
    /// — yields only to a relaunch, which makes the words somebody else's.
    fn guard(self, launch: Option<u64>) -> zerocode_pty::ready::Guard {
        match self {
            Self::Mounting | Self::Resting => zerocode_pty::ready::Guard::for_its_own_line(launch),
            Self::RestingBesideADraft => {
                zerocode_pty::ready::Guard::for_somebody_elses_line(launch)
            }
        }
    }
}

/// The launch a pane holds, as a number the delivery can carry.
///
/// A launch token is a string the window mints per spawn; the delivery's line
/// facts are `Copy` and read every pump round, so the token travels as a
/// fingerprint. `None` for a pane the window recorded no launch for — a shell
/// somebody typed an agent into — which refuses nobody.
pub(crate) fn launch_fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

/// The launch fingerprint of the pane at `term`, from the window's record.
pub(crate) fn launch_of(state: &AppState, term: TermId) -> Option<u64> {
    state
        .launch_tokens()
        .get(&term)
        .map(|token| launch_fingerprint(token))
}

pub(crate) fn prompt_delivery_for(
    text: String,
    submit: bool,
    agent: Option<&str>,
    readiness: PromptReadiness,
    launch: Option<u64>,
    started: Instant,
) -> PromptDelivery {
    let signal = readiness.signal(agent);
    let clearing = readiness.clearing(agent);
    let guard = readiness.guard(launch);
    match readiness {
        PromptReadiness::Mounting => PromptDelivery::with_deadlines(
            text,
            submit,
            signal,
            started,
            ready_quiet_for(agent),
            ready_timeout_for(agent),
        )
        .clearing(clearing)
        .guarded(guard),
        PromptReadiness::Resting | PromptReadiness::RestingBesideADraft => {
            PromptDelivery::new(text, submit, signal, started)
                .clearing(clearing)
                .guarded(guard)
        }
    }
}

/// The sole typed-prompt door for an existing PTY.
///
/// Live sends, orchestration dispatches, and restart continuations all enter
/// here. The caller only chooses whether the composer is mounting or already
/// at rest; readiness, clear keys, bracketed paste, and Enter remain one
/// [`PromptDelivery`] state machine turned by the pump.
pub(crate) fn type_prompt_at_term(
    state: &AppState,
    term: TermId,
    text: String,
    submit: bool,
    agent: Option<&str>,
    readiness: PromptReadiness,
) -> Result<std::sync::mpsc::Receiver<DeliveryOutcome>, String> {
    /* The signal is chosen NOW, not at activation: it names the agent this
     * prompt was addressed to. The launch road waits for the agent's FIRST
     * announcement, and `ready_signal_for` says what that is; a TUI at rest
     * never announces itself again — waiting on codex's `›` spent the whole
     * eight-second budget and delivered nothing (live report 2026-08-14:
     * "코덱스한테는 아무것도 안 감").
     *
     * Both answers come off [`PromptReadiness`] rather than being spelled
     * out here and again in `prompt_delivery_for`. They were, and two copies
     * of one decision is one queued prompt away from disagreeing with the
     * delivery it turns into. */
    let signal = readiness.signal(agent);
    let clearing = readiness.clearing(agent);
    // The launch these words are addressed to, read NOW: the delivery may
    // wait out a readiness window and a queue before it writes, and a pane
    // relaunched in that time holds a program these words were not for.
    let launch = launch_of(state, term);
    let guard = readiness.guard(launch);
    // Liveness is decided under the `deliveries` guard so it cannot race the
    // pump's exit cleanup: the pump removes a dead shell's pool entry while
    // still holding `deliveries`, so a prompt that gets this guard after a
    // death always sees the terminal already gone — a check before the guard
    // could pass and then park a delivery addressed to nobody.
    let mut deliveries = state.deliveries();
    if !state.terminals().contains_key(&term) {
        return Err(format!("터미널 {term}이(가) 없습니다"));
    }
    let (notify, waiting) = std::sync::mpsc::sync_channel(1);
    if deliveries.contains_key(&term) {
        state
            .prompt_queue()
            .entry(term)
            .or_default()
            .push_back(QueuedPrompt {
                text,
                submit,
                signal,
                clearing,
                guard,
                completion: notify,
            });
        return Ok(waiting);
    }
    state.delivery_waiters().insert(term, notify);
    deliveries.insert(
        term,
        prompt_delivery_for(text, submit, agent, readiness, launch, Instant::now()),
    );
    drop(deliveries);
    // The wait is measured in output, so the pump has to be running at display
    // rate to see it — an idle nap would spend most of the readiness window.
    state.cadence().wake();
    Ok(waiting)
}

/// Which terminals hold an agent, for the send menu to offer.
///
/// A READ of the one map, not a second census. The launch records and the
/// hook self-reports write it, and so does `sweep_arrived_agents` — Orca's
/// road (1-g7), the program actually HOLDING each pty, asked of the kernel
/// and matched against the registry's expected processes, which is how a
/// `claude` somebody typed into a plain shell still gets offered (live report
/// 2026-08-14: "열려있는 agent 중에 고를 수 있어야 함").
///
/// This function used to walk the kernel road itself, and that second asker
/// is precisely how the answers came apart: the send menu saw a hand-started
/// agent and the sidebar and board — which read `pane_agents`, the same map
/// without the walk — did not ("zo는 아예 감지도 못함"). One owner writes,
/// every surface reads.
#[tauri::command]
pub(crate) fn agent_terms(state: State<'_, AppState>) -> Vec<(TermId, &'static str)> {
    let _crumb = crate::crumbs::Command::enter("agent_terms");
    let living: HashSet<TermId> = state.terminals().terms().into_iter().collect();
    let mut listed: Vec<(TermId, &'static str)> = state
        .agent_terms()
        .iter()
        .filter(|(term, _)| living.contains(term))
        .map(|(term, agent)| (*term, *agent))
        .collect();
    listed.sort_unstable();
    listed
}

/// Start an agent in a fresh terminal, carrying a prompt.
///
/// Orca's `launchAgentInNewTab` reached from the notes menu: the same launch
/// an automation performs, minus the schedule. The two prompt roads are the
/// registry's fact, not this function's choice — an agent that takes its
/// prompt on the command line gets it there bare and runs it
/// (`buildAgentStartupPlan`); every other agent gets a delivery that waits
/// for its ready signal, exactly as a scheduled prompt would.
///
/// Nine wire arguments, and they stay nine: a `#[tauri::command]`'s payload
/// parameters ARE its wire shape, so folding them into a struct would rename
/// every field the window sends — a payload change to quiet a lint about a
/// payload. `AppHandle` is injected by Tauri and is not a tenth wire field.
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub(crate) fn launch_agent_tab(
    app: AppHandle,
    state: State<'_, AppState>,
    agent: String,
    prompt: String,
    rows: u16,
    cols: u16,
    // `"submit-after-ready"` overrides the injection table and hands the
    // prompt over the way a source-control launch action does. Absent means
    // the table decides, which is right for a prompt somebody typed.
    delivery: Option<String>,
    // A recipe's extra CLI arguments, already split by `launch_plan` — the
    // one door that validated them. Appended after the launch plan's own
    // flags, so a recipe can override a default but not lose it.
    extra_args: Option<Vec<String>>,
    // Which pane asked for this one. Present when the launch came from a
    // pane's own surface — a quick command run from a terminal's menu — and
    // absent when a person opened an agent from a door that belongs to no
    // pane. This is the board's subagent tree at its source; see
    // `AppState::pane_parents`.
    parent: Option<TermId>,
    // A conversation handed over to a new process (measured:
    // ai-vault-resume-command builds `claude --resume <id>`). Its callers —
    // the account handoff, the wire tab's 「화면」 — start the replacement
    // before the pane it replaces closes, so this road is deliberately not
    // judged by `conversation_wake`; a door that RE-ENTERS a conversation
    // takes `resume_session`. Claude-only for now: its session store is the
    // one whose id shape this window knows (`list_claude_sessions`).
    resume: Option<String>,
    // A restored leaf's closed-window screen, put back before the agent's
    // first byte is parsed (`replay_stored_screen`).
    restore: Option<super::settings::StoredScreen>,
) -> Result<TermId, String> {
    let spec =
        agent_spec(&agent).ok_or_else(|| format!("{agent}은(는) 이 창이 모르는 에이전트입니다"))?;
    // Everything below that used to be decided by the agent's name is the
    // row's answer now: the resume selectors and store, the trust menu, the
    // spawn road. A door that asks has no name to get wrong.
    let caps = spec.capabilities();
    let root = state.active_root();
    // Every catalog agent launched with work receives the same provider-neutral
    // delegation law. Empty interactive launches stay empty; providers with a
    // measured context hook also receive it on session start, which covers a
    // person who opens an empty tab and asks it to orchestrate later.
    let prompt = zerocode_core::delegation::with_agent_selection_contract(&prompt).into_owned();
    let mut argv: Vec<String> = spec.launch.split_whitespace().map(str::to_string).collect();
    // What lets the agent actually WORK, before anything about the prompt.
    //
    // A coding agent started from a terminal stops to ask before it edits or
    // runs anything, which is right at a prompt and useless in a pane nobody
    // is watching — the question sits behind another tab and the work never
    // happens. Every agent has its own spelling for "I have already decided",
    // and `zerocode-core::launch` is the table plus the one judgement about
    // what a launch adds up to. Orca hands these by default too
    // (`DEFAULT_TUI_AGENT_ARGS = YOLO_TUI_AGENT_ARGS`), and the window says so
    // on the agent's row rather than leaving it in a table nobody reads.
    let launch_override = stored_launch_override(state.settings(), spec.id)?;
    let plan = zerocode_core::launch_plan(spec.id, launch_override.as_ref());
    if resume.is_some() {
        // One authoritative selector (#12982): a `--continue` or stale
        // `--resume` saved into the launch args must not compete with the
        // session this door was asked to reopen. The selectors are the
        // agent's own row; an agent with none gets its args back untouched.
        argv.extend(caps.resume.launch_args_without_selectors(&plan.args));
    } else {
        argv.extend(plan.args.iter().cloned());
    }
    if let Some(session) = resume.as_deref() {
        // The store-resume road is the row's: only an agent whose session
        // store this window scans has one, and the id has to wear that
        // store's shape before it may reach argv — a crafted "id" can never
        // smuggle an extra argument shape into the launch.
        let Some(store) = caps.resume.store else {
            return Err(format!(
                "{}은(는) 아직 지난 세션 이어가기를 지원하지 않습니다",
                spec.name
            ));
        };
        if !store.id.accepts(session) {
            return Err("이어갈 세션의 이름이 올바르지 않습니다".to_string());
        }
        argv.push(store.flag.to_string());
        argv.push(session.to_string());
    }
    argv.extend(extra_args.unwrap_or_default());
    // And WHICH ACCOUNT it runs as. Appended after the plan's own env so an
    // agent-level override cannot silently point the CLI at another login —
    // the account is the more specific answer to the same question.
    let mut env = plan.env.clone();
    env.extend(account_env_for(state.config_root(), spec.id)?);
    // And where its own home is, when this agent needs telling. Codex on the
    // mirror lane reads its hook from a home this window made; without this the
    // hook is written where nothing looks for it.
    let (agent_env, _auth_launch_lock) =
        hooks::agent_launch_env_with_lock(state.local_data_root(), spec.id);
    env.extend(agent_env);
    // And how it REPORTS BACK. The pane key names which tab an event belongs
    // to; the launch token says which occupant of that tab sent it, so a
    // straggler from the agent that was here before a relaunch cannot repaint
    // the one that is here now. Taken before the spawn for the same reason the
    // terminal id is: a child cannot be handed a key minted after it exists.
    let term = state.take_term_id();
    let launch_token = new_launch_token(term);
    env.extend(hooks::pty_env(
        &hooks::pane_key_of(term),
        Some(&launch_token),
        &root,
        pty_path_in(&env),
    ));
    hooks::mark_selection_seeded(&mut env, &prompt);
    // How the prompt rides along is the injection mode's fact, read off the
    // resolver Orca launches with (`buildAgentStartupPlan`,
    // tui-agent-startup.ts:100-113): argv mode puts it on the command line
    // BARE — behind a separator when the agent wants one (`grok -- …`),
    // positional otherwise (`claude "…"`), and the agent runs it at once.
    // Never behind the draft flag: `--prefill` seeds claude's composer
    // WITHOUT submitting (tui-agent-config.ts:59-60, `draftPromptFlag`), and
    // carrying the launch prompt on it left claude sitting with the
    // instruction unsent — the map's P0-1. The three flag modes spell fixed
    // flags (`--prompt`, `--prompt-interactive`, `-i`); everyone else is
    // typed at after their ready signal, exactly as a scheduled prompt is.
    // Hermes deliberately takes that typed road too: Orca plans a bespoke
    // query invocation for it, and a paste an agent reads is better than a
    // subcommand this window would be guessing at.
    let mut typed_after_start: Option<String> = None;
    // A LAUNCH action overrides the injection table and takes the typed road.
    //
    // Measured: every source-control launch action starts its agent with an
    // EMPTY prompt and has the text pasted and submitted after the ready
    // signal (`promptDelivery: "submit-after-ready"` —
    // fix-checks-agent-launch-DGiOvpEA.js:653, SourceControl-e46DLHZz.js:5827,
    // ChecksPanel-CF1trgAI.js:5410; the dialog says so in its own words at
    // SourceControlAgentActionDialog-Dvhq-Oe8.js:1152). The injection table is
    // right for a prompt somebody typed into the launcher; it is wrong for
    // these, and for a reason that is not only fidelity: a fix-checks prompt
    // carries CI log tails, and a few kilobytes of somebody's build output on
    // a command line is an argv-length limit waiting to be hit and a copy of
    // their logs in every `ps` on the machine.
    let forced = delivery.as_deref() == Some("submit-after-ready");
    if forced && !prompt.trim().is_empty() {
        typed_after_start = Some(prompt.clone());
    } else {
        // The row's road, spelled once in core: `prompt_injection` puts the
        // words on the agent's submit road and answers nothing for an empty
        // prompt — an agent started to be worked with by hand. This door
        // used to keep its own copy of that table, which is how a second
        // launch site and this one could come to disagree.
        match prompt_injection(spec, &prompt) {
            Some(Injected::Argv(words)) => argv.extend(words),
            Some(Injected::AfterStart(text)) => typed_after_start = Some(text),
            None => {}
        }
    }
    // And whether this one may start OTHERS.
    //
    // The teammate flag goes on the command line and the team's coordinates go
    // in the environment, in that order and only for an agent that has such a
    // flag. Last of the three env contributors on purpose: `PATH` here is the
    // one the shim must be found on, and a contributor after it could put a
    // real `tmux` back in front. Off by default — Orca ships it off too
    // (`claudeAgentTeamsMode: "off"`), because a coordinator that starts four
    // more processes is not a thing to turn on behind somebody's back.
    let teams_mode = load_settings_for_boot(state.settings())
        .document
        .agent_teams_mode;
    let mode_args = zerocode_core::agent_teams::teammate_mode_args(&argv, teams_mode);
    if !mode_args.is_empty() {
        // After the program, before everything else — where a flag is read.
        argv.splice(1..1, mode_args);
    }
    // Asked of the PROGRAM, not of the flag we may have just added: somebody
    // who typed `--teammate-mode auto` into the args field themselves wants a
    // team too, and `teammate_mode_args` answers empty for them precisely
    // because they already said it.
    let team_env = agent_teams::open_team(
        state.local_data_root(),
        term,
        teams_mode,
        &pty_path_of(&env),
        argv.first().map(String::as_str).unwrap_or_default(),
    );
    env.extend(team_env.iter().cloned());
    // The workspace was opened here on purpose; repeat that answer in the
    // agent's own trust file BEFORE the spawn, or its first-run "trust this
    // folder?" menu reads the pasted prompt as its keystrokes (P0-8). Orca
    // pre-marks in launch prep the same way, and best-effort the same way —
    // a person can still answer the menu by hand.
    if let Some(preset) = caps.trust_menu()
        && let Err(error) = agent_trust_presets::mark_workspace_trusted(preset, &root, &env)
    {
        eprintln!(
            "zerocode-shell: the {} trust preset was not written: {error}",
            spec.id
        );
    }
    let (program, args) = argv
        .split_first()
        .map(|(program, args)| (program.clone(), args.to_vec()))
        .ok_or("실행할 명령이 없습니다")?;
    // Zo's ordinary terminal launch is also its cold-restore fallback: an old
    // pane-layout record remembers only `running: zo`, so this is the door it
    // calls after a restart. A bare `PtyLane::spawn` recreates the old command
    // line but not the address file, --events-bind readiness fence, channel
    // registry or subscriber. The row says which agent has such a pane; every
    // other agent stays on the unchanged generic PTY road.
    let spawned = if caps.spawn == SpawnRoad::SocketPane {
        let addr_file = std::env::temp_dir().join(format!(
            "zerocode-events-term-{term}-{}.addr",
            LaneId::random()
        ));
        state
            .supervisor()
            .ok_or_else(|| "`zo`가 PATH에 없습니다".to_string())
            .and_then(|supervisor| {
                crate::cmd::project::open_zo_pane_process(
                    supervisor, &root, None, &addr_file, &env, &args, rows, cols,
                )
            })
            .map(|opened| {
                let crate::cmd::project::OpenedZoPane {
                    pty,
                    addr,
                    session_id,
                    token,
                    observation,
                } = opened;
                (pty, Some((addr, token, session_id, observation)))
            })
    } else {
        PtyLane::spawn(&program, &args, Some(&root), &env, rows, cols)
            .map(|pty| (pty.into(), None))
            .map_err(|error| error.to_string())
    };
    let (mut pty, zo_channel) = spawned.map_err(|error| {
        agent_teams::forget_term(term);
        note_window_event(
            state.local_data_root(),
            &format!("term {term} refused {program}: {error}"),
        );
        error
    })?;
    if let Some(screen) = &restore {
        super::settings::replay_stored_screen(state.config_root(), screen, pty.terminal_mut());
    }
    // The husk forensics read a pane's life off two lines, and this road wrote
    // NEITHER. It is the road the launcher takes — the ordinary way an agent
    // pane is born — so a blank pane opened from the launcher could not be
    // told apart from a pane whose shell was never started at all: the rule at
    // the reaper ("whichever of these lines is missing") had no line to miss
    // here. 2026-08-19: terms 4 and 6 stood empty with no spawn and no death
    // in the record, and the record could not say which.
    note_window_event(
        state.local_data_root(),
        &format!("term {term} spawned {program}"),
    );
    state.hold_terminal(term, pty);
    // The leader's own team environment, so a teammate it opens can be handed
    // the same coordinates. Held only for a launch that really opened a team.
    if !team_env.is_empty() {
        state.team_envs().insert(term, env.clone());
    }
    state.agent_terms().insert(term, spec.id);
    state.launch_tokens().insert(term, launch_token);
    if let Some((addr, token, session_id, observation)) = zo_channel {
        // `session.info` is the durable identity the old bare launch never
        // learned. Record it before subscribing: the first history frame may
        // arrive immediately, and the pane-layout save path can now carry this
        // exact id into its next restart.
        {
            // One guard: reading the held session and writing the new one
            // through two calls would lock the same mutex twice.
            let mut held = state.pane_sessions();
            // A launch knows the id and not yet the file. A pane being
            // relaunched into may already have reported one; keep it rather
            // than blanking the conversation view.
            let carried = zerocode_core::ProviderSession {
                key: zerocode_core::SessionKey::SessionId,
                id: session_id.clone(),
                transcript_path: None,
            }
            .carrying_forward(held.get(&term));
            held.insert(term, carried);
        }
        crate::cmd::project::attach_zo_pane_channel(
            &app,
            state.inner(),
            ZoChannelOwner::Term(term),
            addr,
            token,
            session_id,
            Some(observation),
        );
    }
    // Who asked for it, written at the one moment the fact exists. A pane
    // naming itself is refused rather than stored: an id already taken from
    // `take_term_id` cannot be this launch's own, so a self-parent could only
    // arrive from a webview that made one up — and a row that is its own
    // parent is a knot the tree rules would then have to untie every paint.
    if let Some(above) = parent.filter(|above| *above != term) {
        state.pane_parents().insert(term, above);
    }
    if let Some(text) = typed_after_start {
        let launch = launch_of(&state, term);
        state.deliveries().insert(
            term,
            PromptDelivery::with_deadlines(
                text,
                true,
                ready_signal_for(Some(spec.id)),
                Instant::now(),
                ready_quiet_for(Some(spec.id)),
                ready_timeout_for(Some(spec.id)),
            )
            .clearing(false)
            .guarded(zerocode_pty::ready::Guard::for_its_own_line(launch)),
        );
    }
    state.cadence().wake();
    Ok(term)
}

/// Programmatic text — quick commands, setup/default tabs, login helpers.
/// These roads must not make a worker look person-owned.
#[tauri::command(async)]
pub(crate) fn term_text(
    state: State<'_, AppState>,
    term: TermId,
    text: String,
    human: Option<bool>,
) -> Result<(), String> {
    if human == Some(true) {
        with_terminal(&state, term, |pty| {
            pty.terminal_mut().grid_mut().view_to_bottom();
            crate::prompt_transaction::human_write(term, text.as_bytes(), || {
                pty.write_input(text.as_bytes())
            })
        })?;
        orchestration::pane_taken_over(term, now_epoch_ms());
    } else {
        write_terminal_text(&state, term, &text)?;
    }
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn term_key(
    app: AppHandle,
    state: State<'_, AppState>,
    term: TermId,
    press: KeyPress,
) -> Result<(), String> {
    let Some(bytes) = encode_key(&press) else {
        return Ok(());
    };
    with_terminal(&state, term, |pty| {
        // A keystroke is aimed at the program, and the program is at the
        // bottom — reading history ends the moment typing starts.
        pty.terminal_mut().grid_mut().view_to_bottom();
        crate::prompt_transaction::human_write(term, &bytes, || pty.write_input(&bytes))
    })?;
    // A DELIVERED key from this road is a person's hand — paste delivery and
    // every programmatic write take other roads — and a hand in a worker's
    // pane takes the pane over, durably. After the write and only on
    // success, for the same reason as the wait below: a key this window
    // failed to deliver took nothing over.
    orchestration::pane_taken_over(term, now_epoch_ms());
    // The same hand is the notify seat's label (t-6043): a key into a pane
    // that rang inside the last minute says the ring was worth it, and any
    // key says the person is at the window — which is also when the rings
    // the seat held are told, once.
    notify_call::note_hand(&app, term);
    // A person may have just answered a blocked question, which sends no hook
    // at all. Asked AFTER the write and only when it SUCCEEDED: a key this
    // window failed to deliver answered nothing, and clearing a wait on it
    // would paint a ready pane over an agent still blocked.
    //
    // The ENCODED bytes are the subject, not `press.key` — Orca watches what
    // xterm actually sent (`observeSentTerminalInput`), and Enter arrives in
    // five spellings depending on which keyboard protocol is in force. The
    // cheap membership test stands here rather than inside the door because
    // ordinary typing is the hot path and every other key must cost one
    // comparison, not a lock (Orca says the same, one layer out).
    if let Ok(data) = std::str::from_utf8(&bytes)
        && zerocode_core::ask::is_potential_submit_input(data)
    {
        clear_answered_wait(&app, term, &AnswerRoad::Keystroke(data));
    }
    // And the other gesture a person makes at a blocked pane: a stop. Same
    // place and the same warrant — a key this window failed to deliver
    // interrupted nothing. The two are siblings rather than one door because
    // an answer is decided here and a stop may have to SETTLE first.
    observe_stop_gesture(&app, term, &bytes);
    Ok(())
}

/// Move a terminal's view through its own history — positive lines go back
/// toward older output, negative toward live.
///
/// The decisions live here and in the grid, never in the window. On the
/// primary screen `scroll_view` rules: clamped to what the scrollback holds,
/// anchored to content while the program keeps printing. On the ALTERNATE
/// screen there is no history — the program owns its whole surface — so the
/// wheel scrolls by meaning instead: arrow keys, which is xterm's own
/// fallback and what makes a wheel work in a pager or an agent TUI that
/// never asked for the mouse. Capped per event so a flick of inertial
/// scrolling does not type a hundred arrows into a prompt.
/// `with_terminal` wakes the pump so the scrolled frame goes out now rather
/// than at the next natural output.
///
/// "Never asked for the mouse" is the whole of that fallback's warrant, so it
/// is now a condition rather than a sentence. A program that IS tracking has
/// already said how it wants the wheel; this road is the one the window takes
/// when it deliberately withheld the notch (shift — see the wheel handler in
/// `ui/shell.js`), and answering that with arrows would type into the TUI
/// something nobody pressed. Measured on `claude`: a wheel notch scrolls its
/// transcript, an ArrowUp walks the composer's prompt history instead.
#[tauri::command(async)]
pub(crate) fn term_scroll(
    state: State<'_, AppState>,
    term: TermId,
    lines: i32,
) -> Result<(), String> {
    with_terminal(&state, term, |pty| {
        if pty.terminal().grid().alt_screen() {
            if pty.terminal().grid().mouse_tracking() != MouseTracking::Off {
                return Ok(());
            }
            let press = KeyPress {
                key: if lines > 0 { "ArrowUp" } else { "ArrowDown" }.to_string(),
                ctrl: false,
                alt: false,
                shift: false,
            };
            let Some(bytes) = encode_key(&press) else {
                return Ok(());
            };
            for _ in 0..lines.unsigned_abs().min(40) {
                pty.write_input(&bytes)?;
            }
            return Ok(());
        }
        pty.terminal_mut()
            .grid_mut()
            .scroll_view(isize::try_from(lines).unwrap_or(0));
        Ok(())
    })
}

/// Open or close one OSC 7788 fold region in a terminal's history.
///
/// The window hides a collapsed body on the live screen itself, but a
/// scrolled view is an absolute row-for-slot frame it cannot refill — so the
/// grid is told the reader's answer and composes scrollback without the body.
#[tauri::command(async)]
pub(crate) fn term_fold(
    state: State<'_, AppState>,
    term: TermId,
    id: u32,
    collapsed: bool,
) -> Result<bool, String> {
    with_terminal(&state, term, |pty| {
        Ok(pty
            .terminal_mut()
            .grid_mut()
            .set_fold_collapsed(id, collapsed))
    })
}

/// Every place a string occurs in a shell's history and on its screen.
///
/// Searching in Rust rather than in the window is not an optimisation, it is
/// the only place the question can be asked: the window holds the rows it is
/// currently *painting* and nothing else, so a search done there could only
/// ever see the screen — and the line somebody is looking for in a terminal is
/// almost never the one in front of them. This is the same reason the
/// scrollbar needed two new delta fields; the history lives here.
///
/// Literal, with case folding as the one option. A regular expression would
/// mean taking a new dependency for a feature that reads perfectly well
/// without one.
#[tauri::command(async)]
pub(crate) fn term_search(
    state: State<'_, AppState>,
    term: TermId,
    query: String,
    case_sensitive: bool,
) -> Result<Vec<TermHit>, String> {
    with_terminal(&state, term, |pty| {
        Ok(pty
            .terminal()
            .grid()
            .search(&query, case_sensitive, TERM_SEARCH_MAX))
    })
}

/// Put a searched-for line on screen, and answer where the view landed.
///
/// The window needs the answer, not just the move: it draws the scrollbar from
/// `view_offset`, and the frame carrying the new one arrives asynchronously —
/// so a caller that jumped and then had to wait for a frame to know where it
/// was would paint one stale thumb per hit.
#[tauri::command(async)]
pub(crate) fn term_view_to_line(
    state: State<'_, AppState>,
    term: TermId,
    line: usize,
) -> Result<usize, String> {
    with_terminal(&state, term, |pty| {
        Ok(pty.terminal_mut().grid_mut().view_to_line(line))
    })
}

/// The text of a selection, for the window to copy.
///
/// The window paints the rows it can see and nothing else. A selection is a
/// pair of points in the coordinate space `TermHit` speaks — absolute over
/// history and screen — and the one it dragged past the top of the pane
/// names lines the window never held; the grid holds them all, so the grid
/// answers (`TerminalGrid::text_between`), and every rule about what a copied
/// line is — a wide glyph whole, padding dropped, a collapsed fold's body
/// skipped, a newline per row — lives beside the cells rather than being
/// spelled a second time in JavaScript over rows it does not have.
#[tauri::command(async)]
pub(crate) fn term_lines(
    state: State<'_, AppState>,
    term: TermId,
    from: TermPoint,
    to: TermPoint,
) -> Result<String, String> {
    with_terminal(&state, term, |pty| {
        Ok(pty
            .terminal()
            .grid()
            .text_between((from.line, from.col), (to.line, to.col)))
    })
}

/// The lane twin of [`term_lines`].
#[tauri::command(async)]
pub(crate) fn lane_lines(
    state: State<'_, AppState>,
    id: LaneId,
    from: TermPoint,
    to: TermPoint,
) -> Result<String, String> {
    state
        .registry()
        .text_between(id, (from.line, from.col), (to.line, to.col))
        .ok_or_else(|| "레인이 떠 있지 않습니다".to_string())
}

/// Tell the program whether this terminal has the keyboard (`CSI ?1004`).
///
/// The mode was parsed and stored from the beginning and the bytes were never
/// sent — a field read by nothing, which is worse than no support at all: the
/// program's request is accepted and then never answered. An agent TUI asks
/// for this in its opening burst and uses it to stop drawing a caret it does
/// not own.
///
/// Whether to speak is the GRID's answer, not the window's. A window that
/// decided would need to be told the mode and kept in step with a program that
/// changes it while it runs — the same reasoning that keeps mouse reporting's
/// decision next to the modes.
#[tauri::command(async)]
pub(crate) fn term_focus(
    state: State<'_, AppState>,
    term: TermId,
    focused: bool,
) -> Result<(), String> {
    with_terminal(&state, term, |pty| {
        if !pty.terminal().grid().focus_reporting() {
            return Ok(());
        }
        pty.write_input(encode_focus(focused))
    })
}

/// What the pointer did, offered to the program running in this shell.
///
/// Whether it hears about it is ITS decision, recorded in the modes it set —
/// so the answer is worked out here, next to the grid holding those modes,
/// rather than in a window that would have to be told and kept in step.
#[tauri::command(async)]
pub(crate) fn term_mouse(
    state: State<'_, AppState>,
    term: TermId,
    event: MouseEvent,
) -> Result<(), String> {
    with_terminal(&state, term, |pty| {
        let bytes = {
            let grid = pty.terminal().grid();
            encode_mouse(&event, grid.mouse_tracking(), grid.mouse_sgr())
        };
        match bytes {
            Some(bytes) => pty.write_input(&bytes),
            None => Ok(()),
        }
    })
}

#[tauri::command(async)]
pub(crate) fn term_paste(
    app: AppHandle,
    state: State<'_, AppState>,
    term: TermId,
    text: String,
) -> Result<(), String> {
    with_terminal(&state, term, |pty| {
        pty.terminal_mut().grid_mut().view_to_bottom();
        let bracketed = pty.terminal().grid().bracketed_paste();
        let bytes = encode_paste(&text, bracketed);
        crate::prompt_transaction::human_write(term, &bytes, || pty.write_input(&bytes))
    })?;
    // A paste is a person's hand as much as a key is (t-6043).
    notify_call::note_hand(&app, term);
    Ok(())
}

/// Answer an agent's question from a card, by walking its own TUI.
///
/// The keys are the measured protocol (`zerocode_core::ask`), and this
/// command owns only what Orca's renderer owned around them
/// (`sendNativeChatAskAnswer`, index-ftls8Hg_.js:65269): one key group per
/// second — the TUI redraws between groups, and a burst outruns it — text
/// travelling as a paste (`buildNativeChatPasteBytes`: multiline wrapped,
/// a single line sanitized bare), and a new answer for the same pane
/// cancelling the one still being typed (`cancelInFlight`, :69780).
///
/// An agent whose TUI nobody measured gets the answer as a sentence instead
/// (`sendNativeChatMessage`, :65167): the text, then Enter after the submit
/// delay.
///
/// Refused outright when nothing was answered — `hasAskAnswer` is the send
/// button's rule and this side holds it too, because an empty walk would
/// still press Enter at whatever the TUI is showing.
#[tauri::command(async)]
pub(crate) fn answer_ask(
    app: AppHandle,
    state: State<'_, AppState>,
    term: TermId,
    agent: String,
    prompt: zerocode_core::ask::AskPrompt,
    selections: Vec<zerocode_core::ask::AskSelection>,
) -> Result<(), String> {
    use zerocode_core::ask;
    if !ask::has_answer(&prompt, &selections) {
        return Err("고른 답이 없습니다".to_string());
    }
    if !state.terminals().contains_key(&term) {
        return Err("터미널이 떠 있지 않습니다".to_string());
    }
    let (groups, step) = match ask::keys_for(&agent, &prompt, &selections) {
        Some(groups) => (
            groups,
            std::time::Duration::from_millis(ask::QUESTION_STEP_MS),
        ),
        None => ask::line_keys(&ask::format_answer(&prompt, &selections)),
    };
    // The generation this send belongs to. A newer answer for the same pane
    // bumps it, and the older thread sees the bump and stops mid-walk.
    let generation = {
        let mut sends = state.ask_sends();
        let slot = sends.entry(term).or_insert(0);
        *slot += 1;
        *slot
    };
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let same_send = || state.ask_sends().get(&term) == Some(&generation);
        if walk_key_groups(&state, term, &groups, step, same_send).is_err() {
            return;
        }
        // Every group landed, so the walk reached its own submitting Enter —
        // the three `return`s above are the paths where it did not, and none
        // of them arrives here. The card knows what it answered, so its road
        // skips the keystroke test and only the pane's own gates remain;
        // Orca gives this surface a separate door for exactly that reason
        // (`inferQuestionAnsweredFromCurrentStatus`).
        clear_answered_wait(&app, term, &AnswerRoad::Card);
    });
    Ok(())
}

/// Walk key groups into a pane the way a person's answer walks a TUI: each
/// group after `step`, text as a paste and keys as their bytes, through the
/// one door every write to a shell takes (`with_terminal`). `keep_going` is
/// asked before every group — a newer answer for the same pane stops an older
/// walk mid-way. Ok when every group landed; Err names why the walk stopped:
/// a pane that left, a child that stopped reading, or a walk told to stop.
pub(crate) fn walk_key_groups(
    state: &State<'_, AppState>,
    term: TermId,
    groups: &[zerocode_core::ask::KeyGroup],
    step: std::time::Duration,
    keep_going: impl Fn() -> bool,
) -> Result<(), String> {
    use zerocode_core::ask::KeyGroup;
    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(step);
        }
        if !keep_going() {
            return Err("a newer answer took the pane".to_string());
        }
        let bytes = match group {
            KeyGroup::Raw(raw) => raw.clone().into_bytes(),
            KeyGroup::Text(text) => encode_paste(text, text.contains(['\r', '\n'])),
        };
        with_terminal(state, term, |pty| pty.write_input(&bytes))?;
    }
    Ok(())
}

/// Answer a permission request from a card: one key, at once.
///
/// Orca's approval card sends raw and dismisses itself in the same breath
/// (`onChoose` → `sendRaw`, index-ftls8Hg_.js:69583) — no pacing, no walk,
/// because the TUI is sitting on a numbered prompt: Allow is the digit that
/// picks the first row and Deny is escape, which is a refusal everywhere
/// (`parseApprovalFromStatus`, :69216). Which byte means which lives in the
/// tested crate; this command only spends it.
#[tauri::command(async)]
pub(crate) fn answer_approval(
    state: State<'_, AppState>,
    term: TermId,
    allow: bool,
) -> Result<(), String> {
    let key = if allow {
        zerocode_core::ask::APPROVAL_ALLOW
    } else {
        zerocode_core::ask::APPROVAL_DENY
    };
    with_terminal(&state, term, |pty| pty.write_input(key.as_bytes()))
}

#[tauri::command(async)]
pub(crate) fn term_resize(
    state: State<'_, AppState>,
    term: TermId,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    with_terminal(&state, term, |pty| pty.resize(rows, cols))
}

#[tauri::command]
pub(crate) fn focus_lane(
    app: AppHandle,
    state: State<'_, AppState>,
    id: LaneId,
) -> Result<(), String> {
    let _crumb = crate::crumbs::Command::enter("focus_lane");
    let event = {
        let mut registry = state.registry();
        registry.focus(id).map_err(|error| error.to_string())?
    };
    emit_lane_event(&app, event);
    Ok(())
}

#[tauri::command]
pub(crate) fn close_lane(
    app: AppHandle,
    state: State<'_, AppState>,
    id: LaneId,
) -> Result<(), String> {
    let _crumb = crate::crumbs::Command::enter("close_lane");
    let (removed, orphaned_session) = {
        let mut registry = state.registry();
        let Some(removed) = registry.remove(id) else {
            return Err(format!("no lane with id {id}"));
        };
        // Only when no OTHER lane is still showing that session: the same
        // session can be staged twice, and cutting a live lane's only
        // permission path is worse than leaving one thread parked.
        let session = removed.lane.session_id.clone().filter(|session| {
            !registry
                .lanes()
                .any(|lane| lane.session_id.as_deref() == Some(session.as_str()))
        });
        (removed, session)
    };
    // The bell's memory of this lane goes with it — AFTER the registry block
    // above has ended, keeping the lock order (the bell is never taken while
    // the registry is held). A closed lane that kept its entry would hand a
    // stale last-state to nothing forever; the review that found the ring
    // race found this leak beside it.
    state.lane_bell().forget(id);
    zo_integration_runtime::prune_owner(state.inner(), ZoChannelOwner::Lane(id));
    // The subscriber is parked in a blocking read, so this does not join it —
    // it stops the thread from forwarding the next frame, which is what keeps
    // a closed lane from raising a permission modal.
    if let Some(session) = orphaned_session {
        detach_zo_channel_if_owned(state.inner(), &session, ZoChannelOwner::Lane(id));
    }
    let _ = app.emit(
        "lane:closed",
        ClosedPayload {
            lane: id,
            orphaned: removed.orphaned,
        },
    );
    if let Some(refocused) = removed.refocused {
        emit_lane_event(&app, refocused);
    }
    Ok(())
}

/// Typed text — what the IME hands over, already composed. Raw UTF-8 to the
/// child; no encoding table involved.
#[tauri::command(async)]
pub(crate) fn text_input(
    state: State<'_, AppState>,
    id: LaneId,
    text: String,
) -> Result<(), String> {
    state.cadence().wake();
    state
        .registry()
        .write_input(id, text.as_bytes())
        .map_err(|error| error.to_string())
}

/// A named key or a chord. The webview sends the DOM vocabulary and the
/// encoder in `zerocode-pty` owns the bytes; a key it does not know is
/// swallowed here, deliberately.
#[tauri::command(async)]
pub(crate) fn key_input(
    state: State<'_, AppState>,
    id: LaneId,
    press: KeyPress,
) -> Result<(), String> {
    let Some(bytes) = encode_key(&press) else {
        return Ok(());
    };
    state.cadence().wake();
    state
        .registry()
        .write_input(id, &bytes)
        .map_err(|error| error.to_string())
}

/// The lane twin of [`term_scroll`]: history on the primary screen, arrow
/// keys on the alternate one, capped per event the same way. The grid's own
/// rules (clamping, anchoring) ride along through `Registry::scroll_view`,
/// and typing snaps back to live inside `write_input` itself. Including the
/// one condition on that fallback: a program still tracking the mouse is not
/// answered with arrows it never asked for.
#[tauri::command(async)]
pub(crate) fn lane_scroll(
    state: State<'_, AppState>,
    id: LaneId,
    lines: i32,
) -> Result<(), String> {
    state.cadence().wake();
    let mut registry = state.registry();
    if registry.alt_screen(id) == Some(true) {
        if registry
            .mouse_modes(id)
            .is_some_and(|(tracking, _)| tracking != MouseTracking::Off)
        {
            return Ok(());
        }
        let press = KeyPress {
            key: if lines > 0 { "ArrowUp" } else { "ArrowDown" }.to_string(),
            ctrl: false,
            alt: false,
            shift: false,
        };
        let Some(bytes) = encode_key(&press) else {
            return Ok(());
        };
        for _ in 0..lines.unsigned_abs().min(40) {
            registry
                .write_input(id, &bytes)
                .map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    registry
        .scroll_view(id, isize::try_from(lines).unwrap_or(0))
        .map_err(|error| error.to_string())
}

/// The lane twin of [`term_fold`]: the reader's fold answer, for scrollback.
#[tauri::command(async)]
pub(crate) fn lane_fold(
    state: State<'_, AppState>,
    id: LaneId,
    fold: u32,
    collapsed: bool,
) -> Result<bool, String> {
    state
        .registry()
        .set_fold_collapsed(id, fold, collapsed)
        .map_err(|error| error.to_string())
}

/// What the pointer did, offered to the agent running in this lane.
///
/// The lane twin of [`term_mouse`]. Same rule, same encoder: whether the
/// program hears about it is the program's decision, read from the modes it
/// set rather than decided by the window.
#[tauri::command(async)]
pub(crate) fn mouse_input(
    state: State<'_, AppState>,
    id: LaneId,
    event: MouseEvent,
) -> Result<(), String> {
    let registry = state.registry();
    let Some((tracking, sgr)) = registry.mouse_modes(id) else {
        return Ok(());
    };
    let Some(bytes) = encode_mouse(&event, tracking, sgr) else {
        return Ok(());
    };
    drop(registry);
    state.cadence().wake();
    state
        .registry()
        .write_input(id, &bytes)
        .map_err(|error| error.to_string())
}

/// Pasted text, wrapped in bracketed-paste markers when the program asked for
/// them — pasting without the brackets makes a multi-line paste execute line
/// by line.
#[tauri::command(async)]
pub(crate) fn paste_input(
    state: State<'_, AppState>,
    id: LaneId,
    text: String,
) -> Result<(), String> {
    state.cadence().wake();
    let mut registry = state.registry();
    let bracketed = registry.bracketed_paste(id).unwrap_or(false);
    let bytes = encode_paste(&text, bracketed);
    registry
        .write_input(id, &bytes)
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn resize_lane(
    state: State<'_, AppState>,
    id: LaneId,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    // A resize makes the program redraw, which is a frame to catch.
    state.cadence().wake();
    state
        .registry()
        .resize(id, rows, cols)
        .map_err(|error| error.to_string())
}
