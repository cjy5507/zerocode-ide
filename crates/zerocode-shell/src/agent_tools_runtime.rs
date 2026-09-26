use super::*;
use zerocode_core::capabilities::{PointerRoute, SubmitAck};
use zerocode_core::readiness::{AuthState, Purpose};

/// A screen change, as the webview receives it. `delta` obeys the shift
/// contract documented on [`zerocode_pty::GridDelta`].
#[derive(Serialize, Clone)]
pub(super) struct ScreenPayload {
    pub(super) lane: LaneId,
    pub(super) delta: zerocode_pty::GridDelta,
}

/// A shell that exited on its own.
#[derive(Serialize, Clone)]
pub(super) struct TermGone {
    pub(super) term: TermId,
}

/// One live Codex worker whose native pointer fast path was unavailable.
///
/// The PTY fallback keeps delivery working; this event keeps that degradation
/// from existing only in the window's black-box log.
#[derive(Serialize, Clone)]
pub(super) struct CodexRouteDegraded {
    pub(super) term: TermId,
    pub(super) reason: String,
}

/// One live shell shown by Terminal → Manage Sessions.
///
/// The renderer receives only the small facts it can display. Process paths,
/// argv and environment stay behind the native boundary.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(super) struct ManagedTerminalSession {
    pub(super) term: TermId,
    pub(super) running: Option<bool>,
    pub(super) agent: Option<&'static str>,
    pub(super) program: Option<String>,
}

/// A shell rang the bell.
///
/// The same field as [`TermGone`] and deliberately not the same type: these
/// two sentences are as different as a program can make them, and a window
/// reading `TermGone` off a bell would be one careless edit from treating a
/// ring as a death. Announced for every shell on the round it rang
/// (`announce_term_news`): a frame waits for its screen to pull it, and a
/// background tab — the one place a bell means "come here" — is exactly the
/// screen that does not pull.
#[derive(Serialize, Clone)]
pub(super) struct TermBell {
    pub(super) term: TermId,
}

/// A terminal's program renamed itself (`OSC 0`/`OSC 2`).
///
/// Announced for every shell on the round it happened, for the same reason
/// as [`TermBell`]: the tab strip names a hidden tab by this word, and without
/// a line of its own a hidden tab would wear yesterday's title until somebody
/// looked at it. The frame still carries the title as state, which is what a
/// window that starts reading later is seeded with.
#[derive(Serialize, Clone)]
pub(super) struct TermTitle {
    pub(super) term: TermId,
    pub(super) title: String,
}

/// The tail of a shell nobody is reading, for the surface that shows them all.
///
/// A SNAPSHOT, and that is the whole difference from a pulled frame. Taking a
/// delta clears it, so a delta taken and not delivered is gone for good —
/// throttling deltas would leave a card permanently wrong about a screen it
/// can never be told the rest of. These rows are read off the grid instead,
/// which still holds the truth, and they replace whatever the card was
/// showing.
#[derive(Serialize, Clone)]
pub(super) struct TermPreview {
    pub(super) term: TermId,
    pub(super) rows: Vec<zerocode_pty::GridRow>,
}

/// How often a previewed shell's tail crosses to the window.
///
/// The full tier is display rate and this one must not be: four agents at
/// display rate on the thread that owes the next keystroke is the cost the
/// gate below exists to refuse, and re-buying it through a second door would
/// be the same defect wearing a different name. Four beats a second reads as
/// live to a person and is a fraction of the frames it stands in for.
pub(super) const TERM_PREVIEW_INTERVAL_MS: i64 = 250;

/// How many rows of a previewed shell cross with it.
///
/// A card is a few lines tall. Sending a whole screen and cropping it in the
/// window would pay the cost this bound exists to avoid, on the very thread it
/// exists to protect.
pub(super) const TERM_PREVIEW_ROWS: usize = 6;

/// A validated OSC 52 request from either terminal backend.
///
/// The source address is intentionally absent: the system clipboard is global
/// and the renderer's only decision is the live terminal preference. Parsing,
/// bounding, UTF-8 decoding and per-terminal coalescing already happened in
/// the shared grid before this text crossed the webview boundary.
#[derive(Serialize, Clone)]
pub(super) struct ClipboardWritePayload {
    pub(super) text: String,
}

/// A run whose agent just said its turn ended — the row is stamped, and the
/// window refreshes what it shows of the job.
#[derive(Serialize, Clone)]
pub(super) struct AutomationCompleted {
    pub(super) id: String,
    pub(super) run: String,
    pub(super) term: TermId,
}

/// A scheduled job that just started, and the shell it started in.
#[derive(Serialize, Clone)]
pub(super) struct AutomationStarted {
    pub(super) id: String,
    /// Everything the by-hand door already answers with, spelled once.
    ///
    /// Flattened rather than restated field by field: the window adopts a run
    /// the same way whichever road it arrived on, and two records with the
    /// same job would be two records that come to disagree — with the
    /// scheduled one drifting, because nobody is watching it at 9am.
    #[serde(flatten)]
    pub(super) run: StartedRun,
}

/// What starting a run produced — the terminal, what it is called, and where
/// it happens.
#[derive(Serialize, Clone)]
pub(super) struct StartedRun {
    pub(super) term: TermId,
    /// The job's own name, because the tab this shell gets wears it. A run
    /// that shows up on the strip as `터미널 7` is a run nobody can tell from
    /// the shell they opened themselves.
    pub(super) name: String,
    /// The checkout the run ACTUALLY happens in — the freshly cut worktree for
    /// `NewPerRun`, the stored workspace otherwise. The window files the tab
    /// under it, which is what makes the board attribute the card to the
    /// checkout the agent is working in rather than to whatever happened to be
    /// on screen when the schedule fired.
    pub(super) root: String,
    /// The checkout a `NewPerRun` job cut for itself, so the sidebar can show
    /// it and the person can jump. Absent for a run in an existing workspace.
    pub(super) worktree: Option<String>,
}

/// A scheduled job that could not start at all.
#[derive(Serialize, Clone)]
pub(super) struct AutomationFailed {
    pub(super) id: String,
    pub(super) error: String,
}

/// A scheduled firing the precheck deliberately stopped.
#[derive(Serialize, Clone)]
pub(super) struct AutomationSkipped {
    pub(super) id: String,
}

/// One scheduled job, plus what the window cannot work out for itself.
///
/// `cron` and `next_run_at` are derived rather than stored: deriving them here
/// keeps one implementation of "what does this schedule mean" — the window
/// showing a next-run time its backend disagrees with would be worse than
/// showing none.
#[derive(Serialize, Clone)]
pub(super) struct AutomationRow {
    #[serde(flatten)]
    pub(super) automation: Automation,
    pub(super) cron: String,
    /// Whether the next run leaves evidence — the policy read against the
    /// prompt, so the overview can say what "프롬프트에 따라" came to.
    pub(super) leaves_evidence: bool,
    /// Local minutes since the epoch, or `None` when it is switched off or
    /// names no minute in the coming year.
    pub(super) next_run_at: Option<i64>,
    /// How many times this job has run, as far as the ledger remembers.
    ///
    /// On the row for the reason Orca puts it there (`usageSummary.knownRuns >
    /// 0 ? "N/M runs" : …`, AutomationsPage-CJka0o7M.js:29143): a schedule
    /// that has never fired and one that fires nightly look identical from a
    /// next-run time alone, and the difference is the first thing anybody
    /// checks when a job seems not to be working.
    pub(super) runs: usize,
}

/// A prompt that finished its journey into a shell, either way.
///
/// The failure is the half worth carrying: a prompt that timed out left an
/// agent sitting with nothing to do, and without this the window has no way to
/// tell that from a task still being thought about.
#[derive(Serialize, Clone)]
pub(super) struct PromptSettled {
    pub(super) term: TermId,
    pub(super) delivered: bool,
    /// Whether the words reached the composer at all — true for a delivery
    /// that pasted and then withheld its Enter, whose words are on the line
    /// where the person can read them.
    pub(super) pasted: bool,
    /// Why a write was withheld, in the guard's own words, when one was.
    /// `None` for a delivery that landed or simply timed out.
    pub(super) why: Option<&'static str>,
    /// The words that never landed, when they did not — so the person asked
    /// to paste them has them.
    pub(super) text: Option<String>,
}

/// What closing a lane tells the webview.
#[derive(Serialize, Clone)]
pub(super) struct ClosedPayload {
    pub(super) lane: LaneId,
    /// The child refused to die. Surfaced rather than logged, because a
    /// process nobody is showing should be the user's fact, not a secret.
    pub(super) orphaned: bool,
}

pub(super) fn emit_lane_event(app: &AppHandle, event: LaneEvent) {
    // A send to a window that is closing may fail; there is nobody left to
    // paint for, so the error has no consumer either.
    match event {
        LaneEvent::Updated(lane) => {
            // The bell, for the half of the product that does not report
            // through hooks. A supervised lane's stops are the same two stops —
            // and until this, only hook-reporting agents could reach the person
            // outside the window.
            //
            // Ringing follows a CHANGE, not an update: a lane is republished
            // for anything it holds — a title the program just set, a session
            // id arriving — so ringing per update would ring the same
            // permission gate on every repaint. A lane's FIRST appearance does
            // ring: an agent that opens straight into a permission gate is
            // exactly the one nobody is watching.
            let state = app.state::<AppState>();
            // The decision — change or repeat, ring or stay quiet — lives in
            // the core's LaneBell, where tests CALL it; this door only feeds
            // it the truth. And the REGISTRY is the truth, not the event: an
            // Updated is a snapshot, and snapshots queue — the pump can be
            // holding a stale Streaming while a command thread has already
            // gated the lane, rung for it, and recorded it, and bookkeeping
            // from the snapshot would then walk the memory backwards until a
            // harmless title update rings an answered gate again.
            // (Found by review, with the interleaving written out.)
            //
            // The truth is read UNDER the bell's own lock, so two threads
            // emitting for one lane serialize here and the later one reads
            // the newer truth. Lock order is bell → registry, and nothing
            // holds the registry while taking the bell — close_lane's
            // forget runs after its registry block ends.
            let rang = {
                let mut bell = state.lane_bell();
                let current = state.registry().lane(lane.id).map(|held| held.state);
                bell.observe(lane.id, current)
            };
            // The ring happens OUTSIDE the locks — it talks to the OS.
            if let Some(ring) = rang {
                // The lane's own words about what it is doing, and no question:
                // a lane's permission prompt reaches the person as a modal in
                // this window, so the notification's job is to get them back to
                // it, not to restate it on a lock screen.
                let said = (!lane.title.is_empty()).then(|| lane.title.clone());
                let worktree = lane
                    .worktree_id
                    .clone()
                    .unwrap_or_else(|| state.project_root().display().to_string());
                // 레인은 판 번호가 없다 — Reopen의 안내는 워크트리까지다.
                ring_now(
                    app,
                    RingNotice {
                        worktree: &worktree,
                        term: None,
                        agent: lane.agent.slug(),
                        ring,
                        interrupted: LANE_NEVER_INTERRUPTED,
                        ask: None,
                        said,
                    },
                );
            }
            let _ = app.emit("lane:updated", lane);
        }
        LaneEvent::Screen { lane, delta } => {
            if delta.bell {
                app.state::<AppState>()
                    .native_tray()
                    .note_activity(app, native_tray::ActivitySource::TerminalBell);
            }
            let _ = app.emit("lane:screen", ScreenPayload { lane, delta });
        }
        LaneEvent::ClipboardWrite { text, .. } => {
            let _ = app.emit_to(
                MAIN_WINDOW_LABEL,
                TERMINAL_CLIPBOARD_WRITE_EVENT,
                ClipboardWritePayload { text },
            );
        }
    }
}

/* ---- an orchestrating agent's teammates ----
 *
 * The window's end of `zerocode_shell::agent_teams`. A leader asks its fake
 * `tmux` for a pane; the bridge hands the request here; this cuts the pane and
 * tells the window about it. Kept beside `hook_loop` because it is the same
 * shape — a task fed by the bridge, holding the one `AppHandle` that can reach
 * both the terminals and the webview. */

/// What the window is told when a teammate's pane exists.
///
/// `parent` is the shell whose leaf gets divided, never a tab id: the window
/// finds the tab from the shell, which is the one lookup that cannot be wrong
/// after a tab was renamed, moved between groups or restored.
#[derive(Clone, serde::Serialize)]
pub(super) struct TermSplit {
    pub(super) parent: TermId,
    pub(super) term: TermId,
    /// `"vertical"` cuts left|right, `"horizontal"` top over bottom — the
    /// window's own two words.
    pub(super) direction: &'static str,
    /// Which agent was asked for, when one was. The pane's bar starts as that
    /// agent's name and the tab's name otherwise — the measured rule
    /// (terminals.ts hands a split no title of its own) — and the shell's OSC
    /// title overwrites either the moment it speaks. Never the program+pane
    /// pair: "cd %2" on a bar was the reported bug.
    pub(super) agent: Option<String>,
    /// Which helper this pane IS, when the split said — zo's pane lane names
    /// its child by the `SubagentStart` hook's own `agent_id`
    /// (`-e ZO_AGENT_ID=<id>`). The window folds that helper's hook row and
    /// this pane's row into ONE row by it (t-3024); `null` for every other
    /// split, which draws as it always did.
    pub(super) helper: Option<String>,
}

/// An orchestration worker terminal belongs to the checkout it runs in, not
/// to a visual split of the coordinator's tab. The parent is still carried for
/// agent lineage and supervision, while the renderer mounts this terminal as
/// a background tab under `worktree`.
#[derive(Clone, serde::Serialize)]
pub(super) struct TermWorker {
    pub(super) parent: TermId,
    pub(super) term: TermId,
    pub(super) worktree: String,
    pub(super) agent: Option<String>,
    /// `None` for an ordinary summons; restart restorations say whether the
    /// provider conversation resumed or a fresh one received the handoff.
    pub(super) resumed: Option<&'static str>,
    /// Which helper this pane IS, when the split said (`-e ZO_AGENT_ID`) —
    /// the same word `term:split` carries. Without it the window could not
    /// fold a worker-surface helper's roster row onto its pane: the row kept
    /// the parent's clock and its click went to the parent's pane, while the
    /// helper's own tab stood unreached (2026-09-13, "implement#0 눌러도
    /// 안 보임").
    pub(super) helper: Option<String>,
    /// What the placement seat is asked about, for a fresh summons — the
    /// surface asks the door and seats the pane where the answer says when
    /// the seat acts (`judge_worker_room`); `None` seats it as a tab.
    pub(super) seat: Option<agent_teams::WorkerSeatWords>,
}

/// The window carrying out what a team asked for.
///
/// A struct rather than closures because the trait is what lets the protocol
/// be tested without any of this — see `agent_teams`'s own `Recorder`.
pub(super) struct TeamWindow {
    pub(super) app: AppHandle,
}

/// A fresh checkout of the leader's repository, for one summoned worker.
///
/// The same machinery as the automations' per-run checkouts, choice for
/// choice: [`Orchestrator::open`] resolves the repository from whatever
/// checkout the leader sits in, the person's workspace-creation preferences
/// place the tree where every other checkout of theirs goes, and
/// `create_from` with no base cuts from `HEAD`. (The window's 새 워크트리
/// action layers its own branch prefix and setup script on top; those belong
/// to a person clicking, not to a summons — the automations draw the same
/// line.) An `Err` carries git's own refusal — a leader outside any
/// repository, a name space exhausted — and the caller turns it into a
/// refused split rather than a shared tree.
///
/// A leader already seated in a LINKED worktree resolves to that worktree's
/// own top, so the family directory is named after the leader's checkout
/// rather than the main one — cosmetic, and the registry is shared either
/// way: `git worktree list` sees every cut wherever the family sits.
pub(super) fn worker_worktree(
    leader_root: &Path,
    name: &str,
    prefs: &WorkspaceCreationPrefs,
) -> Result<(Orchestrator, PathBuf), String> {
    let orchestrator = Orchestrator::open(leader_root).map_err(|refusal| refusal.to_string())?;
    let orchestrator = apply_workspace_creation_prefs(orchestrator, prefs)?;
    let task = WorktreeTask::from_spec(name, None, None);
    let cut = orchestrator
        .create_from(&task, None)
        .map_err(|refusal| refusal.to_string())?;
    record_cut_base(&Host::for_workspace(leader_root), leader_root, &cut, None);
    Ok((orchestrator, cut.path))
}

/// Ask the one ownership rule who made a listed checkout.
pub(super) fn worktree_ownership(
    orchestrator: &Orchestrator,
    worktree: &Worktree,
) -> zerocode_core::Ownership {
    let root = orchestrator.worktree_root().to_string_lossy();
    let path = worktree.path.to_string_lossy();
    zerocode_core::classify(
        zerocode_core::WorktreeFacts {
            path: &path,
            branch: worktree.branch.as_deref(),
        },
        zerocode_core::WorktreeOurs {
            worktree_root: &root,
            past_roots: &[],
            branch_prefix: orchestrator.branch_prefix(),
        },
    )
}

/// Validate the target before an automatic cleanup can call `remove`.
///
/// The second list inside [`Orchestrator::remove`] closes the lock race. This
/// first list is still necessary: it supplies the ownership fact and keeps a
/// reported worker path from becoming authority to remove an unrelated
/// checkout.
pub(super) fn automatic_cleanup_allowed(
    orchestrator: &Orchestrator,
    path: &Path,
) -> Result<(), String> {
    let known = orchestrator
        .list()
        .map_err(|error| format!("could not inspect automatic cleanup target: {error}"))?
        .into_iter()
        .find(|candidate| same_worktree_path(&candidate.path, path))
        .ok_or_else(|| format!("{} is no longer a listed worktree", path.display()))?;
    if known.locked {
        return Err(format!(
            "automatic cleanup skipped for {}: git reports that the worktree is locked",
            known.path.display()
        ));
    }
    let ownership = worktree_ownership(orchestrator, &known);
    if ownership != zerocode_core::Ownership::ZerocodeManaged {
        return Err(format!(
            "automatic cleanup skipped for {}: ownership is `{}`",
            known.path.display(),
            ownership.slug()
        ));
    }
    Ok(())
}

/// Remove only a clean checkout that this window can prove it owns.
pub(super) fn remove_automatic_worktree(
    orchestrator: &Orchestrator,
    path: &Path,
) -> Result<(), String> {
    automatic_cleanup_allowed(orchestrator, path)?;
    orchestrator
        .remove(path, Removal::ConfirmedIfClean)
        .map_err(|error| error.to_string())
}

/// One line in the window log for one checkout this window took away, naming
/// the road that took it.
///
/// Every removal already writes down its REFUSALS. Not one of the five wrote
/// down a success except the reclaim sweep, and that asymmetry is why a day
/// spent asking what deleted two working checkouts ended without an answer:
/// silence meant "the hand road", "the completed-worker road", "an outside
/// `git worktree remove`" and "the log write failed" all at once, and nothing
/// in the file could tell them apart.
///
/// `source` is the argument rather than a sentence baked into a shared helper
/// on purpose. Folding the five roads into one wording would answer "a
/// worktree went" and leave the only question worth asking — WHICH door —
/// exactly as unanswered as the silence did.
///
/// What goes in: the road, the path, and the short reason it was allowed.
/// What does not: the environment, the command, the branch's contents. An
/// audit line that carries a secret is a worse bug than the one it explains.
pub(super) fn note_worktree_removal(data_root: &Path, source: &str, path: &Path, because: &str) {
    note_window_event(
        data_root,
        &format!("worktree removed [{source}] {} — {because}", path.display()),
    );
}

/// Record one failed worker launch and remove only the clean checkout it made.
///
/// Shared by preflight, spawn and post-spawn readiness: those stages happen
/// on opposite sides of the team-table fence, but cleanup is one policy. A
/// dirty tree survives and is named in the window log rather than discarded.
pub(super) fn cleanup_failed_worker_checkout(
    local_data_root: &Path,
    isolated: Option<&IsolatedWorkerCheckout>,
    reason: &str,
) {
    note_window_event(local_data_root, reason);
    let Some(isolated) = isolated else {
        return;
    };
    match remove_automatic_worktree(&isolated.orchestrator, &isolated.path) {
        Ok(()) => note_worktree_removal(
            local_data_root,
            "failed-worker-launch",
            &isolated.path,
            "the launch that cut it never started",
        ),
        Err(refusal) => note_window_event(
            local_data_root,
            &format!(
                "a worker checkout survived its failed launch at {}: {refusal}",
                isolated.path.display()
            ),
        ),
    }
}

pub(super) fn carry_worker_start_refusal(token: &str, worker_surface: bool, reason: &str) {
    // Placed for every surface, not only a ledger worker's: the split walk
    // takes it back on the refusal road and puts the sentence into the shim's
    // stderr, so a teammate pane's parent reads why. A worker surface reads
    // the same slot through its own ask (`take_worker_host_failure`).
    let _ = worker_surface;
    agent_teams::place_worker_host_failure(token, agent_teams::HostStartFailure::new(reason, None));
}

pub(super) fn schedule_completed_worker_cleanup(
    app: &AppHandle,
    term: TermId,
    cleanup: CompletedWorkerCleanup,
) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let retired = retire_terminal(&state, term);
        if retired {
            announce_retired_terminal(&app, term);
        }
        let Some(isolated) = cleanup.isolated else {
            return;
        };
        let Some(checkout) = cleanup.checkout else {
            note_window_event(
                state.local_data_root(),
                &format!(
                    "worker {} completed in an isolated checkout whose path was never reported",
                    cleanup.worker
                ),
            );
            return;
        };
        if !same_worktree_path(&checkout, &isolated.path) {
            note_window_event(
                state.local_data_root(),
                &format!(
                    "worker {} automatic cleanup skipped: reported checkout {} is not the \
                     checkout created at {}",
                    cleanup.worker,
                    checkout.display(),
                    isolated.path.display()
                ),
            );
            return;
        }
        // The same question every other removal door asks, asked here too.
        // This road retires its OWN terminal a few lines up, so the usual
        // answer is zero — but a checkout can hold more than one pane, and a
        // teammate the reporting worker opened is somebody still working.
        let held = checkout_occupancy(&state, &isolated.path);
        if held > 0 {
            note_window_event(
                state.local_data_root(),
                &format!(
                    "worker {} checkout retained at {}: this window is still \
                     running {held} terminal(s) in it",
                    cleanup.worker,
                    checkout.display()
                ),
            );
            return;
        }
        match remove_automatic_worktree(&isolated.orchestrator, &isolated.path) {
            Ok(()) => {
                note_worktree_removal(
                    state.local_data_root(),
                    "completed-worker",
                    &isolated.path,
                    &format!(
                        "worker {} reported ok on an auto-release seat",
                        cleanup.worker
                    ),
                );
                let _ = app.emit("worktree:removed", checkout.to_string_lossy().into_owned());
            }
            Err(error) => {
                // Dirty or otherwise unsafe means retained, never forced.
                note_window_event(
                    state.local_data_root(),
                    &format!(
                        "worker {} checkout retained at {}: {error}",
                        cleanup.worker,
                        checkout.display()
                    ),
                );
            }
        }
    });
}

/// Where a leader is working, read out of the leader's own environment.
///
/// The team table is held for the whole of [`agent_teams::run`] and a host that
/// reached back into it would lock a `Mutex` its caller already holds (the
/// trait's own preamble). So every fact about a leader travels in the
/// arguments — and the leader's environment is one of those arguments, with the
/// tree it was started in already inside it (`hooks::pty_env` writes
/// `WORKTREE_ID`). Last one wins, the way the environment itself is stacked.
///
/// A tree that has since been removed answers `None` rather than sending a
/// spawn at a directory that is not there: the window's active workspace is a
/// worse answer than the leader's, but it is a real one.
pub(super) fn leader_worktree(env: &[(String, String)]) -> Option<PathBuf> {
    pane_worktree(env).filter(|path| path.is_dir())
}

/// The checkout a pane was launched into, whether or not it still stands.
///
/// The same read as [`leader_worktree`] without the `is_dir` filter, because
/// the two callers want opposite things from a directory that is gone. A
/// spawn wants a place that exists. The occupancy question below wants the
/// path the pane BELIEVES it is in — which is the whole point when something
/// has already deleted it out from under the agent.
pub(super) fn pane_worktree(env: &[(String, String)]) -> Option<PathBuf> {
    env.iter()
        .rev()
        .find(|(name, _)| name == zerocode_hookd::env_var::WORKTREE_ID)
        .map(|(_, value)| PathBuf::from(value))
}

/// The stall probe and the retirement fence use the same PTY quiet rule.
pub(crate) fn quiet_since_output(
    last_output_ms: Option<i64>,
    worker_started_ms: i64,
    now_ms: i64,
) -> Option<i64> {
    // The start of a silence is the wall-clock moment the child last wrote,
    // read as the PTY recorded it — never `now − elapsed`, which this once
    // was: two clocks read a beat apart drift, and the ledger keys a silence
    // by this exact number (measured 2026-09-21: eight beats read eight
    // starts spread over 21 ms, and the stall seat's reading of one silence
    // reached the coordinator eight times in eight seconds).
    match last_output_ms {
        Some(at) => (now_ms.saturating_sub(at) >= zerocode_core::orchestration::QUIET_GRACE_MS)
            .then_some(at),
        None => (now_ms.saturating_sub(worker_started_ms)
            >= zerocode_core::orchestration::QUIET_GRACE_MS)
            .then_some(worker_started_ms),
    }
}

/// Whether a shell pane's own shell is back in front — the agent a person
/// started in it has quit (`agent_teams::Host::shell_in_front`). One kernel
/// question for both of its askers: the ledger's pointer, and the resume
/// judge that lets a quit conversation be reopened (`conversation_wake`).
pub(super) fn shell_in_front_of(state: &AppState, term: TermId) -> bool {
    // Asked only where the pty's child is the shell itself; the set's
    // guard is gone before the terminal's lock is taken.
    let shell_pane = { state.shell_panes().contains(&term) };
    if !shell_pane {
        return false;
    }
    let Some(held) = state.terminals().handle(term) else {
        return false;
    };
    let front = lock_pty(&held).foreground_is_child();
    front == Some(true)
}

impl TeamWindow {
    /// Hand `observe` a quiet worker pane's screen, read under the activity
    /// locks that can invalidate it — the fence a handover's stop is judged
    /// in. The actor holds the team incarnation before asking; hook state
    /// and PTY output stay unchanged until `observe` returns (and, inside
    /// it, whatever it commits answers). A pane that is working, or that
    /// printed since the worker started, is observed as nothing — unless
    /// `hook_outlived_ms` says how long a silent pty outlives the hook's
    /// `working` ([`agent_teams::Host::decline_quiet_since`]).
    fn with_quiet_screen(
        &self,
        term: TermId,
        worker_started_ms: i64,
        hook_outlived_ms: Option<i64>,
        observe: &mut dyn FnMut(&str, i64),
    ) {
        let state = self.app.state::<AppState>();
        let Some(held) = state.terminals().handle(term) else {
            return;
        };
        let states = state.pane_states();
        let working = states
            .get(&term)
            .is_some_and(|pane| pane.state == zerocode_core::hook::HookState::Working);
        if working && hook_outlived_ms.is_none() {
            return;
        }
        let pty = lock_pty(&held);
        let now_ms = now_epoch_ms();
        let Some(since_ms) =
            quiet_since_output(pty.last_output_epoch_ms(), worker_started_ms, now_ms)
        else {
            return;
        };
        if working && hook_outlived_ms.is_some_and(|term| now_ms.saturating_sub(since_ms) < term) {
            return;
        }
        let screen = pty.terminal().grid().visible_text();
        observe(&screen, since_ms);
    }
}

impl agent_teams::Host for TeamWindow {
    /// Who the agent in this terminal is, asked by the ledger road under its
    /// own guard. See [`receipt_actor_of`] for why the answer is a digest of
    /// the agent's session and why `None` is a real answer.
    fn actor_for(&self, term: TermId) -> Option<String> {
        receipt_actor_of(&self.app.state::<AppState>(), term)
    }

    /// The launch this terminal is holding, straight from the window's own
    /// record. See [`agent_teams::Host::launch_token_of`] for why the pointer
    /// road is the one that has to ask.
    fn launch_token_of(&self, term: TermId) -> Option<String> {
        self.app
            .state::<AppState>()
            .launch_tokens()
            .get(&term)
            .cloned()
    }

    /// Which agent this window launched into the terminal, as the catalog
    /// spells it — the launch record, not a title sniff. The mail pointer's
    /// cursor exception reads this.
    fn agent_of(&self, term: TermId) -> Option<String> {
        let state = self.app.state::<AppState>();
        let held = { state.agent_terms().get(&term).copied() };
        held.map(str::to_string)
    }

    /// Which managed login this pane was launched as — the record every
    /// Claude launch road writes when its pane is held (t-7538).
    fn pane_login(&self, term: TermId) -> Option<agent_teams::PaneLogin> {
        self.app
            .state::<AppState>()
            .pane_accounts()
            .get(&term)
            .cloned()
    }

    /// The checkout this window put the pane in, read off the same two
    /// records `answer_team_command` already resolves a caller's tree from:
    /// the pane's launch environment first, its reported seat second.
    fn worktree_of(&self, term: TermId) -> Option<PathBuf> {
        let state = self.app.state::<AppState>();
        let env = { state.team_envs().get(&term).cloned() };
        env.as_deref()
            .and_then(pane_worktree)
            .or_else(|| seated_worktree(&state, term).map(PathBuf::from))
    }

    fn shell_in_front(&self, term: TermId) -> bool {
        shell_in_front_of(&self.app.state::<AppState>(), term)
    }

    fn split(
        &self,
        team: &str,
        leader: TermId,
        from_term: TermId,
        pane: &str,
        direction: zerocode_core::agent_teams::Direction,
        command: &str,
        token: &str,
    ) -> Option<TermId> {
        let state = self.app.state::<AppState>();
        // Which helper this pane is, when the split said (zo's
        // `-e ZO_AGENT_ID`): taken first, so a refusal below leaves nothing
        // for the caller's guard to sweep, and said on `term:split`.
        let helper = agent_teams::take_split_helper(token);
        let leader_env = state.team_envs().get(&leader)?.clone();
        // The teammate works where the leader works. Asked of the leader's own
        // launch rather than of the window's active workspace: by the time a
        // teammate is asked for, the person may be looking at another checkout
        // entirely, and an agent started in the wrong tree is worse than one
        // that never started.
        //
        // That is what this line has always said and, until now, not done — it
        // read the window's active workspace, which is the very answer the
        // paragraph above rules out. The leader's own environment carries the
        // tree it was started in, so the honest answer was already in hand.
        let seated = leader_worktree(&leader_env).unwrap_or_else(|| state.active_root());
        /* A `--worktree` worker gets a fresh checkout of the leader's own
         * repository, cut by the same machinery as the automations' per-run
         * checkouts — the person's workspace-creation preferences included —
         * and refused the same way: a creation git turns down ends the split
         * rather than quietly seating the worker in the shared tree — silent
         * sharing is the exact hazard the flag exists to name. Git's own
         * words go to the window log, because "could not open a pane" cannot
         * distinguish a leader outside any repository from a spawn that
         * failed. The pair travels to the failure path below so a spawn that
         * never happened does not leave an empty checkout behind. */
        let worker_host = agent_teams::take_worker_host_ask(token);
        let worker_surface = worker_host.is_some();
        let seat_words = worker_host.as_ref().and_then(|ask| ask.seat.clone());
        let restored = worker_host.as_ref().and_then(|ask| ask.resumed);
        let placement = worker_host
            .as_ref()
            .map(|ask| ask.placement.clone())
            .unwrap_or(agent_teams::WorkerHostPlacement::Inherited);
        let (isolated, root) = match placement {
            agent_teams::WorkerHostPlacement::Inherited => (None, seated),
            agent_teams::WorkerHostPlacement::New(name) => {
                let prefs = load_settings_for_boot(state.settings())
                    .document
                    .workspace_creation_prefs;
                match worker_worktree(&seated, &name, &prefs) {
                    Ok((orchestrator, path)) => {
                        let isolated = IsolatedWorkerCheckout { orchestrator, path };
                        let root = isolated.path.clone();
                        (Some(isolated), root)
                    }
                    Err(refusal) => {
                        let reason =
                            format!("the --worktree checkout for {name} was refused: {refusal}");
                        note_window_event(state.local_data_root(), &reason);
                        carry_worker_start_refusal(token, worker_surface, &reason);
                        return None;
                    }
                }
            }
            agent_teams::WorkerHostPlacement::Existing(path) => {
                let root = PathBuf::from(&path);
                if !root.is_dir() {
                    let reason = format!("the worker's existing checkout is gone ({path})");
                    note_window_event(state.local_data_root(), &reason);
                    if worker_surface {
                        agent_teams::place_worker_host_failure(
                            token,
                            agent_teams::HostStartFailure::permanent(&reason),
                        );
                    }
                    return None;
                }
                (None, root)
            }
        };
        let worker_isolated = worker_surface && isolated.is_some();
        let term = state.take_term_id();
        // The capability arrives with the request now — the journaled split
        // lane digests it into the operation it records before this host
        // runs, so a token minted here would name a different child than the
        // record does. Registration and its rollback stay this function's.
        let mut env = agent_teams::teammate_env(team, pane, token, &leader_env);
        // An empty command is tmux's own default — a shell. A teammate is
        // normally handed `claude …`, and the shell case is what keeps a
        // leader that opened a scratch pane from failing. Resolved BEFORE the
        // environment is finished, because which agent this is decides part of
        // that environment.
        let words = split_command(command);
        // A summons that is a LINE OF SHELL is run by one, the way tmux runs
        // every split command. Split into argv it began with the program
        // `cd`, and the pane died at birth while its agent lived on
        // in-process, invisible. The argv road stays for everything else:
        // the splitter, not a shell, is the security argument for
        // settings-fed commands, and a shell line arrives only from an
        // agent speaking tmux at this window.
        let shell_line = command_is_shell(&words);
        let (program, mut args) = if shell_line {
            shell_spawn(command)
        } else {
            match words.split_first() {
                Some((program, args)) => (program.clone(), args.to_vec()),
                None => (
                    split_command(
                        &load_settings_for_boot(state.settings())
                            .document
                            .terminal_command,
                    )
                    .first()?
                    .clone(),
                    Vec::new(),
                ),
            }
        };
        // The program the pane MEANS — the judge for agent, account and
        // dialect. For a shell line that is the program the line leaves
        // running, never the shell that carries it: `sh` is nobody's agent.
        let spoken = if shell_line {
            shell_spoken_program(&words)
        } else {
            Some(program.clone())
        };
        // Claude's native install answers to a bare version number
        // (`…/versions/2.1.241`), a name no detect table can know. The
        // summons still says which world sent it: only Agent Teams writes
        // `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` into its own line, and
        // Agent Teams summons claude.
        let team_summons = shell_line
            && words
                .iter()
                .any(|word| word == "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1");
        // Which agent it is, and therefore where its own home is. A teammate
        // is an agent launch like any other: Codex on the mirror lane reads
        // its hook out of a home this window makes, and a launch that skipped
        // this would be an agent whose hook is written where nothing looks.
        let named = spoken
            .as_deref()
            .and_then(agent_for_program)
            .or_else(|| team_summons.then_some("claude"));
        // And what that agent can do, off its row: the trust menu, the
        // pointer route and the acknowledgement transport below are each the
        // table's answer, not a name this door knows.
        let caps = named.and_then(zerocode_core::agent_capabilities);
        // A Codex teammate may share the mirror with workers from another
        // blocking team task, or with a worker from a previous window during
        // restart. Keep its inter-process launch lease alive through the PTY
        // spawn; syncing and then dropping the lease before spawn leaves the
        // exact refresh-token window this gate is meant to close.
        let mut _auth_launch_lock = None;
        if let Some(agent) = named {
            // A worker launch consumes both halves of the saved launch plan.
            // `Catalog` already put the argument half on the command line; the
            // host owns the environment half because it is the process-spawn
            // boundary. Dropping it here made Settings show an environment
            // that every coordinator-summoned worker silently ignored.
            let launch_override = match stored_launch_override(state.settings(), agent) {
                Ok(held) => held,
                Err(error) => {
                    let reason = format!(
                        "a worker launch could not read the {agent} launch settings: {error}"
                    );
                    cleanup_failed_worker_checkout(
                        state.local_data_root(),
                        isolated.as_ref(),
                        &reason,
                    );
                    carry_worker_start_refusal(token, worker_surface, &reason);
                    return None;
                }
            };
            env.extend(zerocode_core::launch_plan(agent, launch_override.as_ref()).env);
            // A teammate is a named launch like any other, so it goes through
            // the one account door (`account_env_for`) that resume, launch and
            // automation go through. Without it the whole of a teammate's
            // account came from `teammate_env`, which copies the LEADER's
            // environment — so a claude summoning a codex handed that codex
            // claude's account variables and none of its own. That is the
            // exact case the person asked for by name: codex calling cc, cc
            // calling codex.
            //
            // After the team's environment and before the agent's own home,
            // which is the launch road's order: the account is the more
            // specific answer than the team's, and the home is more specific
            // still.
            let account_env = match account_env_for(state.config_root(), agent) {
                Ok(env) => env,
                Err(error) => {
                    let reason =
                        format!("a worker launch could not prepare the {agent} account: {error}");
                    cleanup_failed_worker_checkout(
                        state.local_data_root(),
                        isolated.as_ref(),
                        &reason,
                    );
                    carry_worker_start_refusal(token, worker_surface, &reason);
                    return None;
                }
            };
            env.extend(account_env);
            // Authenticate with the program the catalogue actually found.  A
            // provider-native team may be carried by a shell line, so the
            // shell binary itself is not an authentication oracle; `spoken`
            // is the foreground agent and the catalogue remains the common
            // source for aliases and native-version installs.
            let auth_program = agent_program(agent)
                .or_else(|| spoken.clone())
                .unwrap_or_else(|| program.clone());
            // The readiness snapshot at LAUNCH freshness (t-3996), read
            // before the door so a refusal can say what the witnesses saw
            // and how old that word is. The door still decides: an
            // override in a launch environment is a login no file witness
            // can see, and the door's own CLI oracle is the one that may
            // accuse. A snapshot the door repaired or proved wrong is
            // forgotten, so the next look re-observes.
            let readiness = readiness_runtime::ensure(agent, Purpose::Launch);
            if let Err(error) =
                accounts::require_unattended_login(state.config_root(), &auth_program, agent, &env)
            {
                let reason = readiness_runtime::refusal_with_evidence(
                    &format!("an unattended worker cannot start: {error}"),
                    &readiness,
                );
                cleanup_failed_worker_checkout(state.local_data_root(), isolated.as_ref(), &reason);
                carry_worker_start_refusal(token, worker_surface, &reason);
                return None;
            }
            if readiness.auth == AuthState::Unauthorized {
                readiness_runtime::invalidate(agent);
            }
            let (agent_env, launch_lock) =
                hooks::agent_launch_env_with_lock(state.local_data_root(), agent);
            env.extend(agent_env);
            _auth_launch_lock = launch_lock;
        }
        // Its own hook coordinates, so the board sees a teammate as an agent
        // rather than as an unexplained shell — and the launch token so a
        // straggler from whatever was in this pane before cannot repaint it.
        let launch_token = new_launch_token(term);
        env.extend(hooks::pty_env(
            &hooks::pane_key_of(term),
            Some(&launch_token),
            &root,
            pty_path_in(&env),
        ));
        if let Some(ask) = worker_host.as_ref() {
            hooks::mark_selection_seeded(&mut env, &ask.prompt);
        }
        // And its own team shims back on the FRONT of that — last, because
        // `pty_env` decides a `PATH` of its own and the later pair wins at
        // spawn time.
        //
        // Without this line a summoned worker had no `zerocode-orc` at all: its
        // environment began as a copy of the leader's `<shims>:<real>` and
        // ended as `pty_env`'s `<mirror-shims>:<hydrated>`. So the one command
        // every worker's briefing tells it to run — `zerocode-orc send --type
        // worker_done` — was not on its PATH, and no dispatch this window
        // summoned could ever be closed by the worker doing it.
        //
        // Built from the PATH THIS environment ended with, never from the
        // process's: the mirror shims are in that value, and rebuilding from
        // `std::env` would drop them and take the nested-run page with them.
        let dialect = if team_summons {
            // A pane summoned in tmux dialect keeps tmux's furniture — a
            // real tmux never strips `TMUX` from a pane it opened.
            zerocode_core::agent_teams::Dialect::Tmux
        } else {
            match spoken.as_deref() {
                Some(program) => zerocode_core::agent_teams::Dialect::of(program),
                None => zerocode_core::agent_teams::Dialect::Ledger,
            }
        };
        let base = env
            .iter()
            .rev()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value.as_str())
            .unwrap_or_default();
        if let Some(path) = agent_teams::teammate_path(state.local_data_root(), dialect, base) {
            env.push(("PATH".into(), path));
        }
        // A worker that does not speak tmux is not told it is inside one. The
        // three variables come from the LEADER by inheritance (`teammate_env`
        // copies its environment), so a claude leader summoning a codex worker
        // hands it a multiplexer that is not on its PATH and never was — the
        // exact state `team_launch_env` refuses to create for a leader, arrived
        // at from the other side.
        if dialect != zerocode_core::agent_teams::Dialect::Tmux {
            env.retain(|(name, _)| {
                name != "TMUX"
                    && name != "TMUX_PANE"
                    && name != "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"
            });
        }
        // The same pre-mark the launch and resume roads make (P0-8). It matters
        // more here than on either of them: a teammate stopped on a trust menu
        // is waiting for a keystroke from a person who is looking at the
        // leader's screen, and an orchestration that has to be typed at is the
        // opposite of the one that was asked for. Written after the
        // environment is complete because the mark reads the config homes
        // (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`) the lines above may just have
        // handed out.
        if let Some(preset) = caps.and_then(|caps| caps.trust_menu())
            && let Err(error) = agent_trust_presets::mark_workspace_trusted(preset, &root, &env)
        {
            let agent = named.unwrap_or_default();
            let reason = format!("the {agent} trust preset was not written: {error}");
            cleanup_failed_worker_checkout(state.local_data_root(), isolated.as_ref(), &reason);
            carry_worker_start_refusal(token, worker_surface, &reason);
            return None;
        }
        // Codex workers get an optional, per-process app server only after the
        // final environment, account, worktree and trust decisions exist. A
        // capability or sidecar refusal leaves argv untouched and the proven
        // PTY pointer path remains available.
        let (codex_route, codex_route_error) = if worker_surface
            && caps.is_some_and(|caps| caps.pointer_route == Some(PointerRoute::CodexAppServer))
        {
            match codex_queue::prepare(Path::new(&program), &mut args, &root, &env) {
                Ok(route) => (Some(route), None),
                Err(error) => {
                    note_window_event(
                        state.local_data_root(),
                        &format!("Codex native pointer route unavailable; using PTY: {error}"),
                    );
                    (None, error.degrades().then_some(error))
                }
            }
        } else {
            (None, None)
        };
        let previous_pane_token = agent_teams::remember_pane_token(team, pane, token.to_string());
        let pty = match PtyLane::spawn(
            &program,
            &args,
            Some(&root),
            &env,
            PTY_BIRTH_ROWS,
            PTY_BIRTH_COLS,
        ) {
            Ok(pty) => pty,
            Err(error) => {
                let reason = format!("a worker process failed to spawn {program}: {error}");
                agent_teams::restore_pane_token(team, pane, previous_pane_token);
                cleanup_failed_worker_checkout(state.local_data_root(), isolated.as_ref(), &reason);
                carry_worker_start_refusal(token, worker_surface, &reason);
                return None;
            }
        };
        note_window_event(
            state.local_data_root(),
            &format!("term {term} spawned {program}"),
        );
        state.hold_terminal(term, pty);
        if worker_isolated && let Some(checkout) = isolated.as_ref() {
            state.isolated_worker_terms().insert(term, checkout.clone());
        }
        let launch = crate::cmd::terminal::launch_fingerprint(&launch_token);
        state.launch_tokens().insert(term, launch_token);
        if let Some(agent) = named {
            state.agent_terms().insert(term, agent);
        }
        note_pane_account(&state, term, &env);
        if let Some(error) = codex_route_error {
            let _ = self.app.emit(
                "codex-route:degraded",
                CodexRouteDegraded {
                    term,
                    reason: error.to_string(),
                },
            );
        }
        if let Some(route) = codex_route {
            route.commit(term);
        }
        let readiness = worker_host
            .filter(|ask| ask.resumed.is_some() || !ask.prompt.trim().is_empty())
            .map(|prompt| {
                /* A restored process proves its composer is ready without
                 * receiving the task yet. The shell delivers that prose only
                 * after `worker_reseated` is durable; otherwise a fast agent
                 * can report completion while its row is still Sleeping. */
                let restoring = prompt.resumed.is_some();
                let submitted_prompt = (!restoring).then(|| prompt.prompt.clone());
                let timeout = Duration::from_millis(u64::from(prompt.timeout_ms));
                let started = Instant::now();
                let (notify, waiting) = std::sync::mpsc::sync_channel(1);
                state.delivery_waiters().insert(term, notify);
                let submitted = submitted_prompt.as_ref().and_then(|_| {
                    named
                        .and_then(AgentKind::from_slug)
                        .filter(|agent| agent.reports_prompt_submit())
                        .map(|_| {
                            let (notify, waiting) = std::sync::mpsc::sync_channel(1);
                            state.worker_prompt_submits().insert(term, notify);
                            waiting
                        })
                });
                let delivery = PromptDelivery::with_deadlines(
                    submitted_prompt.unwrap_or_default(),
                    !restoring,
                    ready_signal_for(named),
                    started,
                    ready_quiet_for(named),
                    timeout,
                )
                .clearing(false)
                // The briefing owns its composer — new, empty, the window's
                // own — and yields only to the pane being relaunched under
                // it, which would make these words the last occupant's.
                .guarded(zerocode_pty::ready::Guard::for_its_own_line(Some(launch)));
                if !restoring && caps.is_some_and(|caps| caps.submit_ack == SubmitAck::Channel) {
                    // A channel-acknowledged agent's submission receipt is the
                    // channel's turn/start frame. Park the composer delivery
                    // until the subscriber has joined, or a fast turn can
                    // start and end before anything is listening. No clear
                    // keys are sent: the launch composer is empty and Zo
                    // treats C0 as text.
                    state.zo_worker_deliveries().insert(term, delivery);
                } else {
                    state.deliveries().insert(term, delivery);
                }
                (
                    waiting,
                    submitted,
                    started + timeout + zerocode_pty::ready::SUBMIT_ACK_TIMEOUT,
                    restoring,
                )
            });
        // Who started it — the same edge the board's subagent tree is drawn
        // from, written at the one moment it is a fact. A teammate IS a
        // subagent, and this is the road that says so.
        state.pane_parents().insert(term, from_term);
        // And which helper the pane IS, when the split said (zo's pane lane,
        // t-3024): the pane ending is that helper finishing, and the forget
        // door retires it in the parent's roster by this id (t-3098).
        if let Some(helper) = helper.as_deref() {
            state.pane_helpers().insert(term, helper.to_string());
        }
        state.team_envs().insert(term, env);
        // Where the pane was seated, in the diagnostic log beside its spawn
        // line: the tab the sidebar reads (`term:worker` → the worktree card
        // whose path this names) or a split inside its parent. A worker that
        // never showed on a card left no way to tell which of the two it was
        // handed, or under which path (2026-09-20, w-5447).
        note_window_event(
            state.local_data_root(),
            &format!(
                "term {term} seated as worker of term {from_term} in {} ({})",
                root.display(),
                if worker_surface { "own tab" } else { "split" }
            ),
        );
        // A protocol worker owns its own tab even when this process restores
        // an existing conversation. Announcing a restore as a split first
        // briefly seats it inside the coordinator's tab.
        if worker_surface {
            let _ = self.app.emit(
                "term:worker",
                TermWorker {
                    parent: from_term,
                    term,
                    worktree: root.to_string_lossy().into_owned(),
                    agent: named.map(String::from),
                    resumed: restored.map(|one| one.as_str()),
                    helper,
                    seat: seat_words,
                },
            );
        } else {
            let _ = self.app.emit(
                "term:split",
                TermSplit {
                    parent: from_term,
                    term,
                    direction: direction.as_str(),
                    agent: named.map(String::from),
                    helper,
                },
            );
        }
        state.cadence().wake();
        if let Some((waiting, submitted, deadline, restoring)) = readiness {
            state.worker_readiness().insert(
                term,
                WorkerReadinessSlot {
                    pending: Some(PendingWorkerReadiness {
                        waiting,
                        submitted,
                        deadline,
                        team: team.to_string(),
                        pane: pane.to_string(),
                        previous_pane_token,
                        isolated,
                        restoring,
                    }),
                    exited_screen: None,
                },
            );
        }
        // The seat answer, placed on the capability the ask rode in on: the
        // checkout that actually ended up under this pane — the leader's own
        // tree, or the fresh cut `isolated` holds. The split walk takes it
        // back on every road out and lands it on the worker row
        // (`worker_seated`), which is the only road the ledger has to this
        // fact: WHERE a pane opens is the window's placement decision, made
        // through the person's own workspace-creation preferences.
        agent_teams::place_seat_checkout(token, root.to_string_lossy().into_owned());
        Some(term)
    }

    fn provider_session(&self, term: TermId) -> Option<zerocode_core::ProviderSession> {
        self.app
            .state::<AppState>()
            .pane_sessions()
            .get(&term)
            .cloned()
    }

    fn conversation_standing(
        &self,
        agent: &str,
        session: &zerocode_core::ProviderSession,
    ) -> Option<TermId> {
        let wanted = zerocode_core::conversation_key(agent, session)?;
        crate::conversation_wake::holding_pane(&self.app.state::<AppState>(), &wanted)
    }

    fn carry_session(&self, term: TermId, session: &zerocode_core::ProviderSession) {
        self.app
            .state::<AppState>()
            .pane_sessions()
            .entry(term)
            .or_insert_with(|| session.clone());
    }

    fn announce_reseated_worker(
        &self,
        term: TermId,
        worktree: &str,
        agent: &str,
        resumed: zerocode_core::orchestration::WorkerResume,
    ) {
        let state = self.app.state::<AppState>();
        let Some(parent) = state.pane_parents().get(&term).copied() else {
            return;
        };
        // The helper this pane was cut as, if the split said — kept in the
        // same table the split wrote, so a reseat folds the same row.
        let helper = state.pane_helpers().get(&term).cloned();
        let _ = self.app.emit(
            "term:worker",
            TermWorker {
                parent,
                term,
                worktree: worktree.to_string(),
                agent: Some(agent.to_string()),
                resumed: Some(resumed.as_str()),
                helper,
                seat: None,
            },
        );
    }

    fn await_worker_ready(&self, term: TermId) -> Result<(), agent_teams::HostStartFailure> {
        let state = self.app.state::<AppState>();
        let pending = {
            let mut readiness = state.worker_readiness();
            let Some(slot) = readiness.get_mut(&term) else {
                return Ok(());
            };
            let Some(pending) = slot.pending.take() else {
                return Err(agent_teams::HostStartFailure::new(
                    "the worker readiness verdict is already being collected",
                    slot.exited_screen.clone(),
                ));
            };
            pending
        };
        let delivered = pending
            .waiting
            .recv_timeout(pending.deadline.saturating_duration_since(Instant::now()));
        let terminal_is_live = state.terminals().contains_key(&term);
        let accepted = if delivered == Ok(DeliveryOutcome::Delivered) && terminal_is_live {
            await_prompt_submission(
                pending.submitted.as_ref(),
                pending.deadline,
                zerocode_pty::ready::QUIET,
                || {
                    let retried = state
                        .terminals()
                        .handle(term)
                        .is_some_and(|held| lock_pty(&held).write_input(b"\r").is_ok());
                    if retried {
                        state.cadence().wake();
                    }
                    retried
                },
            )
        } else {
            false
        };
        if accepted && state.terminals().contains_key(&term) {
            state.worker_readiness().remove(&term);
            return Ok(());
        }
        // A restored worker that is alive and at work is seated, briefing or
        // no briefing. The restore's wait carries no words — it only asks
        // the resumed TUI to show it is ready — and a signal is a thing a
        // busy program need not give (claude's was then a moment's silence,
        // which a worker that picked its task straight back up never gives:
        // it resumed, read its transcript, and went on typing; its composer
        // glyph since t-4530 is drawn once, and a wait can still miss it).
        // The failure road below then tore the pane down and killed
        // the process — a worker mid-task, gone for having been busy (live
        // report 2026-08-30: term 3 spawned 19:08:37, edited a file 19:09:07,
        // "did not accept its briefing: TimedOut" 19:09:37; the ledger left
        // sleeping). At work is the handshake on, or a write within the
        // launch patience; a resumed shell silent that long with no TUI
        // behind it is the case the road below is for.
        if pending.restoring && terminal_is_live {
            let at_work = state.terminals().handle(term).is_some_and(|held| {
                let pty = lock_pty(&held);
                pty.terminal().grid().bracketed_paste()
                    || pty
                        .last_output_at()
                        .is_some_and(|at| at.elapsed() < zerocode_pty::ready::PATIENCE)
            });
            if at_work {
                state.deliveries().remove(&term);
                state.delivery_waiters().remove(&term);
                state.worker_readiness().remove(&term);
                return Ok(());
            }
        }

        let reason = if delivered == Ok(DeliveryOutcome::Delivered) && terminal_is_live {
            "the worker TUI never acknowledged the submitted briefing".to_string()
        } else if delivered == Ok(DeliveryOutcome::Delivered) {
            "the worker exited while accepting its briefing".to_string()
        } else {
            // Asked while the delivery still waits, before the teardown below
            // takes it: the wait knows which door stayed shut.
            let unmet = state
                .deliveries()
                .get(&term)
                .and_then(|delivery| delivery.unmet(Instant::now()));
            briefing_refusal(&delivered, unmet)
        };
        let removed = state.terminals().remove(&term);
        let announce_exit = removed.is_some();
        let screen = removed
            .map(|pty| lock_pty(&pty).terminal().grid().visible_text())
            .or_else(|| {
                state
                    .worker_readiness()
                    .get(&term)
                    .and_then(|slot| slot.exited_screen.clone())
            });
        state.deliveries().remove(&term);
        state.zo_worker_deliveries().remove(&term);
        state.worker_prompt_submits().remove(&term);
        state.prompt_queue().remove(&term);
        agent_teams::restore_pane_token(&pending.team, &pending.pane, pending.previous_pane_token);
        // This re-enters the team table, which is why the whole method is an
        // explicit post-fence host phase rather than part of `split`.
        forget_term_state(&state, term, TermLedgerSettlement::Unrecorded);
        cleanup_failed_worker_checkout(state.local_data_root(), pending.isolated.as_ref(), &reason);
        state.worker_readiness().remove(&term);
        if announce_exit {
            let _ = self.app.emit("term:exited", TermGone { term });
        }
        Err(agent_teams::HostStartFailure::new(reason, screen))
    }

    fn send(&self, term: TermId, text: &str) -> bool {
        let state = self.app.state::<AppState>();
        state.cadence().wake();
        let Some(held) = state.terminals().handle(term) else {
            return false;
        };
        let mut pty = lock_pty(&held);
        pty.terminal_mut().grid_mut().view_to_bottom();
        pty.write_input(text.as_bytes()).is_ok()
    }

    fn paste(&self, term: TermId, text: &str) -> bool {
        let state = self.app.state::<AppState>();
        let agent = state.agent_terms().get(&term).copied();
        // A dispatch preamble is typed on the ledger's behalf, at a pane the
        // run summoned — and a pane a person has reached is not the run's to
        // type into. The draft-preserving readiness yields to their hand, a
        // parked question and a relaunch, at the write.
        let Ok(waiting) = crate::cmd::terminal::type_prompt_at_term(
            &state,
            term,
            text.to_string(),
            true,
            agent,
            crate::cmd::terminal::PromptReadiness::RestingBesideADraft,
        ) else {
            return false;
        };
        waiting.recv_timeout(zerocode_pty::ready::TIMEOUT + zerocode_pty::ready::SUBMIT_ACK_TIMEOUT)
            == Ok(DeliveryOutcome::Delivered)
    }

    /// The integration record's verdict on this pane's zo, as the one sentence
    /// a ledger road reads before pasting a briefing at it.
    fn delivery_refusal(&self, term: TermId) -> Option<String> {
        zo_integration_runtime::delivery_refusal(&self.app, term)
    }

    fn point(
        &self,
        term: TermId,
        line: &str,
        submit: bool,
    ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
        let state = self.app.state::<AppState>();
        let agent = state.agent_terms().get(&term).copied();
        // The beat never blocks: the delivery is registered with the pump
        // and the receipt goes back to the pointer pass, which reads it on a
        // later beat. The words are advice typed into a composer a person
        // may be using, so the draft-preserving readiness is the only one
        // this road ever takes.
        crate::cmd::terminal::type_prompt_at_term(
            &state,
            term,
            line.to_string(),
            submit,
            agent,
            crate::cmd::terminal::PromptReadiness::RestingBesideADraft,
        )
        .ok()
    }

    fn pane_exists(&self, term: TermId) -> bool {
        self.app.state::<AppState>().terminals().contains_key(&term)
    }

    fn jev_wire(&self) -> Option<crate::systemone::Wire> {
        Some(crate::systemone::Wire::of_this_machine())
    }

    fn off_the_beat(&self, job: Box<dyn FnOnce() + Send>) {
        // A thread that cannot be had asks nothing: running the job here
        // would hold the beat for as long as its socket waits.
        if let Err(why) = std::thread::Builder::new()
            .name("jev-off-the-beat".to_string())
            .spawn(job)
        {
            eprintln!("orchestration: a Jev question was not asked, no thread for it: {why}");
        }
    }

    fn quiet_since(&self, term: TermId, worker_started_ms: i64, now_ms: i64) -> Option<i64> {
        let state = self.app.state::<AppState>();
        let held = state.terminals().handle(term)?;
        // A hook that still says `working` outranks screen quiet: a long tool
        // call can legitimately print nothing for minutes. Absence is not a
        // claim of work; hookless agents therefore use the same PTY evidence.
        if state
            .pane_states()
            .get(&term)
            .is_some_and(|pane| pane.state == zerocode_core::hook::HookState::Working)
        {
            return None;
        }
        let last_output = lock_pty(&held).last_output_epoch_ms();
        quiet_since_output(last_output, worker_started_ms, now_ms)
    }

    fn decline_quiet_since(
        &self,
        term: TermId,
        worker_started_ms: i64,
        now_ms: i64,
        hook_outlived_ms: i64,
    ) -> Option<i64> {
        if let Some(since) = self.quiet_since(term, worker_started_ms, now_ms) {
            return Some(since);
        }
        // The hook says `working`: only a pty silent past its term outranks
        // it — a turn that is really working redraws its spinner
        // (`orchestration::note_paused_declines` has the numbers).
        let state = self.app.state::<AppState>();
        let held = state.terminals().handle(term)?;
        let last_output = lock_pty(&held).last_output_epoch_ms();
        quiet_since_output(last_output, worker_started_ms, now_ms)
            .filter(|since| now_ms.saturating_sub(*since) >= hook_outlived_ms)
    }

    fn capture(&self, term: TermId) -> Option<String> {
        let state = self.app.state::<AppState>();
        let held = state.terminals().handle(term)?;
        let text = lock_pty(&held).terminal().grid().visible_text();
        Some(text)
    }

    fn ask_usage(&self, gauge: &str) {
        crate::cmd::usage::ask_usage(self.app.state::<AppState>(), gauge);
    }

    fn quota_wall_marker(
        &self,
        term: TermId,
        agent: &str,
    ) -> Option<zerocode_core::orchestration::QuotaWallMarker> {
        // The table is asked before the grid is copied: an agent nobody
        // measured costs nothing per beat.
        if !crate::quota_wall::has_rule(agent, crate::quota_wall::StallCause::QuotaWall) {
            return None;
        }
        let transcript = self
            .provider_session(term)
            .and_then(|session| session.transcript_path)
            .map(std::path::PathBuf::from);
        let screen = self.capture(term);
        crate::quota_wall::marker_for(agent, screen.as_deref(), transcript.as_deref())
    }

    fn pane_wall(&self, term: TermId, agent: &str) -> Option<crate::quota_wall::PaneWall> {
        // The table is asked inside, before the file is opened: an agent
        // whose walls are screen words only, or that nobody measured, costs
        // a map lookup and no read.
        let transcript = self.provider_session(term)?.transcript_path?;
        crate::quota_wall::pane_wall_for(agent, std::path::Path::new(&transcript))
    }

    fn with_quota_wall_observation(
        &self,
        term: TermId,
        worker_started_ms: i64,
        agent: &str,
        commit: &mut dyn FnMut(zerocode_core::orchestration::QuotaWallMarker),
    ) {
        if !crate::quota_wall::has_rule(agent, crate::quota_wall::StallCause::QuotaWall) {
            return;
        }
        let transcript = self
            .provider_session(term)
            .and_then(|session| session.transcript_path)
            .map(std::path::PathBuf::from);
        self.with_quiet_screen(term, worker_started_ms, None, &mut |screen, _| {
            if let Some(marker) =
                crate::quota_wall::marker_for(agent, Some(screen), transcript.as_deref())
            {
                commit(marker);
            }
        });
    }

    fn classifier_decline_reading(
        &self,
        term: TermId,
        agent: &str,
    ) -> Option<crate::quota_wall::DeclineReading> {
        // The table is asked before the grid is copied: an agent nobody
        // measured costs nothing per beat.
        if !crate::quota_wall::has_rule(agent, crate::quota_wall::StallCause::ClassifierDecline) {
            return None;
        }
        let transcript = self
            .provider_session(term)
            .and_then(|session| session.transcript_path)
            .map(std::path::PathBuf::from);
        let screen = self.capture(term);
        crate::quota_wall::decline_reading_for(agent, screen.as_deref(), transcript.as_deref())
    }

    fn with_classifier_decline_observation(
        &self,
        term: TermId,
        worker_started_ms: i64,
        agent: &str,
        commit: &mut dyn FnMut(crate::quota_wall::DeclineReading, i64),
    ) {
        if !crate::quota_wall::has_rule(agent, crate::quota_wall::StallCause::ClassifierDecline) {
            return;
        }
        let transcript = self
            .provider_session(term)
            .and_then(|session| session.transcript_path)
            .map(std::path::PathBuf::from);
        let hook_outlived = Some(zerocode_core::orchestration::DECLINE_DIALOG_UNANSWERED_MS);
        self.with_quiet_screen(
            term,
            worker_started_ms,
            hook_outlived,
            &mut |screen, since_ms| {
                if let Some(reading) = crate::quota_wall::decline_reading_for(
                    agent,
                    Some(screen),
                    transcript.as_deref(),
                ) {
                    commit(reading, since_ms);
                }
            },
        );
    }

    fn focus(&self, term: TermId) -> bool {
        self.app.emit("term:focus-pane", term).is_ok()
    }

    /// The pane's process group and its leader's start identity, read while
    /// the leader still stands, so a later look at a group that outlived a
    /// close can tell it from a program that took its pid afterwards.
    fn exit_witness(&self, term: TermId) -> Option<agent_teams::ExitWitness> {
        let root = self
            .app
            .state::<AppState>()
            .terminals()
            .handle(term)
            .and_then(|held| lock_pty(&held).pid())?;
        Some(agent_teams::ExitWitness {
            group: root,
            started: crate::resource_usage::process_start_identity(root).ok(),
        })
    }

    /// The pane's program read before the close ([`Self::exit_witness`]) and
    /// watched after it, on the clock the hand-over already keeps for a
    /// pane's CLI leaving (`HAND_OVER_EXIT_WAIT`, polled every
    /// `HAND_OVER_EXIT_POLL`).
    fn close_gone(&self, term: TermId) -> agent_teams::PaneExit {
        let witness = self.exit_witness(term);
        self.close(term);
        match witness {
            Some(witness)
                if !crate::cmd::wait_process_group_gone(
                    witness.group,
                    crate::cmd::HAND_OVER_EXIT_WAIT,
                    crate::cmd::HAND_OVER_EXIT_POLL,
                ) =>
            {
                agent_teams::PaneExit::Lingering(witness)
            }
            _ => agent_teams::PaneExit::Gone,
        }
    }

    fn exit_seen(&self, witness: &agent_teams::ExitWitness) -> bool {
        crate::cmd::program_left(witness)
    }

    fn close(&self, term: TermId) {
        let state = self.app.state::<AppState>();
        // Through the same door the tab close uses, whole: the process is
        // stopped, every map that names the shell forgets it, and the window
        // is TOLD. This road used to name four maps by hand and so kept the
        // session, the last reported state, the lineage and the helpers of
        // every teammate a leader ever retired — and it never told
        // `agent_teams`, so the pane stayed in the team's own table and the
        // leader's `list-panes` went on naming a window that had been shut.
        //
        // The telling cannot be left to the pump. The pump announces a shell
        // that ended INSIDE the map — its `gone` is read off the entries it
        // walks — and this shell is taken out of the map by the retirement,
        // so it is never walked again. A close that only forgot left the
        // window holding a `worker-release`d worker's tab on its last frame
        // and its row in the sidebar, with no process behind either
        // (reported 2026-08-30: "이건 계속 멈춰있어"). Announced only when a
        // shell was actually here: a term the reaper already announced is not
        // told twice.
        if retire_terminal(&state, term) {
            state.cadence().wake();
            announce_retired_terminal(&self.app, term);
        } else {
            forget_term_state(&state, term, TermLedgerSettlement::Recorded(None));
            state.cadence().wake();
        }
    }
}

/// Give every standing order a beat, off the render path.
///
/// The pump owns the clock and nothing else here: a beat may cut a pane, which
/// forks and execs, and doing that on the thread that turns terminal frames
/// would stall every pane in the window to start one. So the pump only decides
/// WHEN, and the beat itself goes to a blocking thread — the same place a team
/// verb goes, because it IS a team verb.
///
/// The launch terms are read fresh at each beat for the reason `teams_loop`
/// reads them fresh at each verb: a permission answered a minute ago has to
/// reach the next worker summoned, and this one is summoned by a clock rather
/// than by somebody who could be asked to restart.
/// Who a pane is, for the purposes of a receipt.
///
/// A receipt has to survive a window restart — that is the case it exists for —
/// and the pane's own address does not: `open_team` mints a fresh random team
/// id on every launch, so a resumed coordinator used to arrive as a stranger to
/// its own receipts and re-run what it had already done.
///
/// The agent's own session is what survives. A conversation resumed reports the
/// same `(key, id)` (`resume_session` puts it back into `pane_sessions`), and a
/// respawn that starts a NEW conversation reports a different one — which is
/// right: a new conversation has not made those requests and must not inherit
/// their answers.
///
/// An agent with no resumable session falls back to the launch this window gave
/// it. That does not cross a restart, and nothing about such an agent does.
/// `None` means the agent has started but has not reported yet, and the ledger
/// road refuses mutations under it rather than filing them under "somebody".
///
/// The raw session id never leaves this function: only a digest reaches the
/// ledger.
pub(super) fn receipt_actor_of(state: &AppState, term: TermId) -> Option<String> {
    let agent = { state.agent_terms().get(&term).copied()? };
    let session = { state.pane_sessions().get(&term).cloned() };
    if let Some(session) = session {
        return Some(zerocode_core::orchestration::receipt_actor(
            agent,
            session.key,
            &session.id,
        ));
    }
    /* An agent that WILL report a session has no identity until it does.
     *
     * The fallback below is for everyone whose launch is all there is: the
     * five hook vendors that never put a resumable id in their events, and
     * every catalog vendor OUTSIDE the hook enum, from which no session is
     * ever coming at all. Giving a stand-in to a claude or codex that simply
     * has not reached its first `SessionStart` would be worse than giving it
     * nothing — the actor would change the moment the real session arrived,
     * and everything filed under the stand-in would be stranded. But asking
     * the enum and treating its ABSENCE as "wait" was the opposite mistake:
     * two dozen summonable vendors were seated, briefed, and then refused
     * the very `worker_done` their briefing told them to run — forever,
     * under a sentence that promised "yet". `session_will_come` draws the
     * line where the truth is.
     */
    if zerocode_core::session_will_come(agent) {
        return None;
    }
    let token = { state.launch_tokens().get(&term).cloned() };
    token.map(|token| zerocode_core::orchestration::incarnation_actor(agent, &token))
}

/// Queue the restart-restoration pass off the renderer and bridge paths.
/// Multiple triggers are harmless: the native pass serializes them and reads
/// Sleeping rows fresh before each walk.
pub(super) fn schedule_orchestration_restore(app: &AppHandle, leader: TermId) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let actor = receipt_actor_of(&state, leader);
        let overrides = stored_launch_overrides(state.settings())
            .unwrap_or_default()
            .into_iter()
            .collect();
        let window = TeamWindow { app: app.clone() };
        orchestration::reseat_sleeping(&window, overrides, leader, actor.as_deref());
    });
}

/// A restored coordinator tab has mounted. The renderer supplies only the
/// terminal it just opened; native state proves whether it is a current team
/// leader and which durable run its provider conversation is bound to.
#[tauri::command]
pub(super) fn restore_orchestration_workers(app: AppHandle, leader: TermId) -> bool {
    let _crumb = crate::crumbs::Command::enter("restore_orchestration_workers");
    schedule_orchestration_restore(&app, leader);
    true
}

/// One standing-order job may be queued or running. The permit is acquired
/// before submitting to the existing blocking lane, and released on unwind too.
pub(super) struct StandingBeat;
static STANDING_BEAT_BUSY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
impl StandingBeat {
    pub(super) fn enter() -> Option<Self> {
        STANDING_BEAT_BUSY
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .ok()
            .map(|_| Self)
    }
}
impl Drop for StandingBeat {
    fn drop(&mut self) {
        STANDING_BEAT_BUSY.store(false, std::sync::atomic::Ordering::Release);
    }
}

pub(super) fn beat_standing_orders(app: &AppHandle) {
    let Some(permit) = StandingBeat::enter() else {
        return;
    };
    let beating = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        let overrides: Vec<_> = {
            let state = beating.state::<AppState>();
            stored_launch_overrides(state.settings())
                .unwrap_or_default()
                .into_iter()
                .collect()
        };
        let window = TeamWindow {
            app: beating.clone(),
        };
        // The beat goes down the same road a typed verb does, and that road
        // asks this window who is in the seat — under its own guard, where the
        // answer cannot go stale between the asking and the acting.
        let now_ms = now_epoch_ms();
        orchestration::tick(&window, &overrides, now_ms);
        // The way out, on the same beat (t-6428): a 「끝나면」 armed goes at
        // the first gap the census finds, and a question nobody answers
        // goes when its time is up.
        crate::cmd::appearance::beat_leaving(&beating);
        // The crash that becomes a task (t-3014 §2.4), on this same beat and
        // through the same road: it presents a seated leader's capability
        // the way the beat above presents one for `worker-start`.
        crash_triage::sweep(&beating, &window, &overrides, now_ms);
        // A QA scenario's failing verdict becomes a task the same way (§4).
        crate::qa_triage::sweep(&beating, &window, &overrides, now_ms);
        // The scoreboard beat's deferred findings become tasks the same way
        // (docs/design/scoreboard-beat-20260911.md): the launchd clock has no
        // pane identity, so the window files what it left.
        crate::scoreboard_inbox::sweep(&beating, &window, &overrides, now_ms);
        crate::scm_observer::sweep(&beating, now_ms);
        // Publish both board readings before emitting the change. This lane
        // owns authority waits; the main thread only clones the completed view.
        orchestration::refresh_board_ledger();
        // The ledger moved since the last beat: tell the window ONCE, so the
        // surfaces that read ledger facts — the navigator's task titles and
        // its verified/merged words — follow the ledger instead of a snapshot
        // taken at boot. The webview coalesces the repaint; this beat only
        // says that there is something to read, never what.
        if let Some(revision) = orchestration::ledger_revision() {
            static LAST_TOLD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if LAST_TOLD.swap(revision, std::sync::atomic::Ordering::SeqCst) != revision {
                let _ = beating.emit("ledger:changed", revision);
            }
        }
        // And the checkouts finished workers left behind, on the same beat and
        // AFTER it: `tick` is what settles a seat whose terminal has just
        // gone, so a sweep placed ahead of it would judge the world one
        // second stale for no reason. Here rather than inside `tick` because
        // the questions it asks are the window's — where the file surfaces
        // are standing, which repository owns a path, what git says about a
        // directory — and `tick` is handed a `Host`, not this window's state.
        // See [`worktree_reclaim`] for what the pass costs when it finds
        // nothing, which is the reason it can ride a one-second beat at all.
        worktree_reclaim::sweep(&beating, now_ms);
    });
}

/// Fixed credits for the bridge's blocking team work.
///
/// The total bounds process-spawning/fsync work submitted to Tokio's blocking
/// pool. Long-poll checks take a second credit as well, so they can occupy at
/// most half the total and the `send` or control request that wakes them keeps
/// a reserved road forward.
#[derive(Clone)]
pub(super) struct TeamWorkBudget {
    pub(super) active: Arc<tokio::sync::Semaphore>,
    pub(super) waiting: Arc<tokio::sync::Semaphore>,
}

impl TeamWorkBudget {
    pub(super) fn new() -> Self {
        Self {
            active: Arc::new(tokio::sync::Semaphore::new(
                zerocode_hookd::TEAM_ACTIVE_LIMIT,
            )),
            waiting: Arc::new(tokio::sync::Semaphore::new(zerocode_hookd::TEAM_WAIT_LIMIT)),
        }
    }

    pub(super) fn try_admit(
        &self,
        request: &zerocode_hookd::TeamRequest,
    ) -> Option<TeamWorkPermit> {
        // The narrower credit first. If the total is then full, returning drops
        // this one immediately; a rejected wait never withholds a future wait
        // seat while occupying no active seat.
        let waiting = if request.reserves_wait_slot() {
            Some(Arc::clone(&self.waiting).try_acquire_owned().ok()?)
        } else {
            None
        };
        let active = Arc::clone(&self.active).try_acquire_owned().ok()?;
        Some(TeamWorkPermit {
            _active: active,
            _waiting: waiting,
        })
    }
}

/// Credits travel into the blocking closure and are returned by `Drop`,
/// including unwinding and every early-return/refusal path.
pub(super) struct TeamWorkPermit {
    pub(super) _active: tokio::sync::OwnedSemaphorePermit,
    pub(super) _waiting: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// Admit or refuse one dequeued request without parking another task behind a
/// semaphore. A refused request is never handed to `spawn`, which is the
/// structural guarantee that it cannot run late as a surprise effect.
pub(super) fn schedule_team_request(
    budget: &TeamWorkBudget,
    request: zerocode_hookd::TeamRequest,
    spawn: impl FnOnce(TeamWorkPermit, zerocode_hookd::TeamRequest),
) {
    if request.answer.is_closed() {
        return;
    }
    match budget.try_admit(&request) {
        Some(permit) => spawn(permit, request),
        None => {
            let _ = request.answer.send(zerocode_hookd::TeamAnswer::busy());
        }
    }
}

/// Answer a team's requests, forever — in either of the two dialects that reach
/// the same panes.
///
/// One road, because both are the same animal: a blocked CLI waiting on the
/// window, an argv in, a stdout/stderr/exit out. Which dialect owns a verb is
/// settled against the table `help` prints, so a verb cannot be advertised by
/// one and refused by the other.
///
/// Each admitted verb is its own blocking task, but admission is fixed: at most
/// [`zerocode_hookd::TEAM_ACTIVE_LIMIT`] effects exist, and at most
/// [`zerocode_hookd::TEAM_WAIT_LIMIT`] of them are `check --wait`. A wait held
/// inline would still put the `send` that ends it behind itself; an unbounded
/// task per request put the whole Tokio blocking pool behind it instead.
///
/// A caller that hung up before execution is not carried out. Once an effect
/// has begun, that claim stops: the bridge timeout does not cancel blocking
/// work, and this layer reports no terminal outcome for it.
///
/// Ordering is not lost by this. A shim blocks its own shell until the answer
/// comes back, so a single pane never has two verbs in flight; between panes
/// there was never an order to keep, and the tables both dialects write are
/// behind their own locks either way.
/// The federation road's pump: one call at a time, each on the blocking
/// pool — an attach cuts a pane and may wait a minute on readiness, and the
/// wire's caller is already holding its own deadline.
pub(super) async fn federation_loop(
    app: AppHandle,
    mut calls: tokio::sync::mpsc::UnboundedReceiver<zerocode_hookd::FederationRequest>,
) {
    while let Some(call) = calls.recv().await {
        let window = TeamWindow { app: app.clone() };
        let answered = tauri::async_runtime::spawn_blocking(move || {
            orchestration::federation_serve(&window, &call.home, &call.body)
        })
        .await
        .unwrap_or_else(|_| {
            serde_json::json!({ "refused": "the window's blocking half died" }).to_string()
        });
        let _ = call.answer.send(answered);
    }
}

pub(super) async fn teams_loop(
    app: AppHandle,
    mut requests: tokio::sync::mpsc::Receiver<zerocode_hookd::TeamRequest>,
) {
    let budget = TeamWorkBudget::new();
    while let Some(request) = requests.recv().await {
        let app = app.clone();
        schedule_team_request(&budget, request, move |permit, request| {
            tauri::async_runtime::spawn_blocking(move || {
                let _permit = permit;
                if request.answer.is_closed() {
                    return;
                }
                let answered = answer_team_command(&app, &request);
                // A leader that stopped waiting is not an error. This send may
                // fail after an admitted effect completed; the dropped receiver
                // is not evidence that the effect failed.
                let _ = request.answer.send(answered.answer);
                // Only after the answer has left this blocking road. The
                // worker's final Done hook is the later acknowledgement that
                // the reporting command actually returned to its TUI.
                if let Some((term, cleanup)) = answered.cleanup {
                    app.state::<AppState>()
                        .completed_worker_cleanups()
                        .insert(term, cleanup);
                }
            });
        });
    }
}

pub(super) struct TeamCommandResult {
    pub(super) answer: zerocode_hookd::TeamAnswer,
    pub(super) cleanup: Option<(TermId, CompletedWorkerCleanup)>,
}

/// Carry out one team verb in whichever dialect owns it.
///
/// Split out of [`teams_loop`] so the answering is one thing and the deciding
/// where to answer it is another — and so the blocking task that carries it
/// has a name to appear under.
pub(super) fn answer_team_command(
    app: &AppHandle,
    request: &zerocode_hookd::TeamRequest,
) -> TeamCommandResult {
    let speaks_orchestration = orchestration::speaks_here(&request.argv);
    // Neither runner is authorized HERE any more. Both take the capability the
    // request presented and prove it under the same team-table guard that
    // plans and carries out the effect — a check here would let go of that
    // table before either of them takes it, and a pane respawned in that gap
    // would receive an effect authorized against the incarnation it replaced.
    let window = TeamWindow { app: app.clone() };
    if speaks_orchestration {
        let verb = request.argv.first().map(String::as_str);
        let term = {
            let teams = agent_teams::teams();
            teams
                .get(&request.team_id)
                .and_then(|team| team.term_of(&request.pane))
        };
        let state = app.state::<AppState>();
        let actor = term.and_then(|term| receipt_actor_of(&state, term));
        let caller_worktree = term.and_then(|term| {
            let env = state.team_envs().get(&term).cloned();
            env.as_deref()
                .and_then(leader_worktree)
                .or_else(|| seated_worktree(&state, term).map(PathBuf::from))
        });
        let caller_identity = caller_worktree
            .as_deref()
            .and_then(zerocode_core::git_dir::identity);
        let bound_run = matches!(verb, Some("worker-start" | "dispatch"))
            .then(|| orchestration::bound_run(&request.team_id, &request.pane, actor.as_deref()))
            .flatten();
        let completed = is_worker_done(&request.argv)
            .then(|| orchestration::seat_assignment(&request.team_id, &request.pane))
            .flatten();
        let isolated = term.and_then(|term| state.isolated_worker_terms().get(&term).cloned());
        // The person's own launch terms, read fresh at each verb rather
        // than held from boot: a permission answer changed a minute ago has
        // to reach the next worker summoned, and a worker is summoned by a
        // process, not by somebody who could be asked to restart.
        let overrides = {
            let state = app.state::<AppState>();
            stored_launch_overrides(state.settings())
                .unwrap_or_default()
                .into_iter()
                .collect()
        };
        let answer = orchestration::run(
            &window,
            overrides,
            &request.team_id,
            &request.pane,
            &request.pane_token,
            &request.argv,
            now_epoch_ms(),
        );
        // A worker that reported done has no more use for what it borrowed
        // (t-6336): the devices the emulator door booted for its pane go
        // down now, not when somebody notices the memory they hold.
        if answer.exit_code == 0
            && is_worker_done(&request.argv)
            && let Some(term) = term
        {
            crate::emulator::borrower_gone(term, crate::emulator::LoanEnd::WorkerDone);
        }
        if answer.exit_code == 0
            && verb == Some("run-use")
            && let Some(term) = term
        {
            // A coordinator whose provider conversation could not be resumed
            // has no durable actor binding until it explicitly chooses the run.
            // That successful choice is the same native restoration trigger as
            // a mounted resumed tab.
            schedule_orchestration_restore(app, term);
        }
        if answer.exit_code == 0 {
            observe_linked_orchestration(
                app,
                verb,
                &request.argv,
                &answer,
                caller_identity.as_ref(),
                bound_run.as_deref(),
                completed.as_ref(),
            );
        }
        let cleanup = if answer.exit_code == 0
            && completed.as_ref().is_some_and(|seat| seat.auto_release)
            && orchestration_arg(&request.argv, "--body")
                .and_then(|body| zerocode_core::orchestration::worker_done_succeeded(body).ok())
                == Some(true)
        {
            term.zip(completed.as_ref()).map(|(term, seat)| {
                (
                    term,
                    CompletedWorkerCleanup {
                        worker: seat.worker_id.clone(),
                        checkout: seat.checkout.as_deref().map(PathBuf::from),
                        isolated,
                    },
                )
            })
        } else {
            None
        };
        TeamCommandResult { answer, cleanup }
    } else {
        TeamCommandResult {
            answer: agent_teams::run(
                &window,
                &request.team_id,
                &request.pane,
                &request.pane_token,
                &request.argv,
            ),
            cleanup: None,
        }
    }
}

pub(super) fn orchestration_arg<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    argv.windows(2)
        .rev()
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

pub(super) fn is_worker_done(argv: &[String]) -> bool {
    argv.first().is_some_and(|verb| verb == "send")
        && orchestration_arg(argv, "--type") == Some("worker_done")
}

pub(super) fn observe_linked_orchestration(
    app: &AppHandle,
    verb: Option<&str>,
    argv: &[String],
    answer: &zerocode_hookd::TeamAnswer,
    caller_identity: Option<&zerocode_core::git_dir::Identity>,
    bound_run: Option<&str>,
    completed: Option<&orchestration::SeatAssignment>,
) {
    let state = app.state::<AppState>();
    let now = now_epoch_ms();
    let parsed: serde_json::Value = serde_json::from_str(answer.stdout.trim()).unwrap_or_default();

    if matches!(verb, Some("worker-start" | "dispatch"))
        && let (Some(identity), Some(run)) = (caller_identity, bound_run)
    {
        let task = parsed.get("taskId").and_then(serde_json::Value::as_str);
        let dispatch = parsed.get("dispatchId").and_then(serde_json::Value::as_str);
        if task.is_some() || dispatch.is_some() {
            match work_item_store::observe_orchestration(
                state.settings(),
                &identity.worktree_id,
                run,
                task,
                dispatch,
                now,
            ) {
                Ok(Some(link)) => {
                    let _ = app.emit("work-item:changed", link);
                }
                Ok(None) => {}
                Err(error) => note_window_event(
                    state.local_data_root(),
                    &format!("linked orchestration observation was not saved: {error}"),
                ),
            }
        }
    }

    let Some(completed) = completed else { return };
    // A completion must carry a message id, but the Jira issue owns one
    // lifecycle: the outbox keys the event on the run, not the message, so a
    // run's many workers settle the issue once (see `enqueue_worker_done`).
    if parsed
        .get("messageId")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        return;
    }
    let verdict: serde_json::Value = orchestration_arg(argv, "--body")
        .and_then(|body| serde_json::from_str(body).ok())
        .unwrap_or_default();
    let Some(succeeded) = verdict.get("ok").and_then(serde_json::Value::as_bool) else {
        return;
    };
    let summary = verdict
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let direct_identity = completed
        .checkout
        .as_deref()
        .and_then(|path| zerocode_core::git_dir::identity(Path::new(path)));
    if let Some(identity) = direct_identity.as_ref() {
        let _ = work_item_store::observe_orchestration(
            state.settings(),
            &identity.worktree_id,
            &completed.run_id,
            Some(&completed.task_id),
            Some(&completed.dispatch_id),
            now,
        );
    }
    let worktree_id = direct_identity
        .as_ref()
        .map(|identity| identity.worktree_id.as_str())
        .or_else(|| caller_identity.map(|identity| identity.worktree_id.as_str()));
    match work_item_store::enqueue_worker_done(
        state.settings(),
        worktree_id,
        Some(&completed.dispatch_id),
        &completed.run_id,
        succeeded,
        summary,
        now,
    ) {
        Ok(Some(event_id)) => schedule_jira_sync(app, event_id),
        Ok(None) => {}
        Err(error) => note_window_event(
            state.local_data_root(),
            &format!("Jira worker completion sync was not queued: {error}"),
        ),
    }
}

/// Answer an agent's `zerocode-browser …`, forever (1-g4).
///
/// The agents' half of the browser feature: Orca's CLI drives its embedded
/// browser over a local RPC (`BROWSER_CORE_METHODS`), and this loop is that
/// dispatcher on our chassis — same loopback bridge as the hooks and teams,
/// same token, and the same server-side distrust as the window's own IPC:
/// the verbs walk the SAME `browsable`/`browser_pane_of` guards, because a
/// shim on loopback is exactly as unvouched-for as a webview.
pub(super) async fn browser_loop(
    app: AppHandle,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<zerocode_hookd::BrowserRequest>,
) {
    while let Some(request) = requests.recv().await {
        // Each request is its own task: a `read` waiting five seconds on a
        // slow page must not queue every `goto` behind it, and a caller
        // whose bridge deadline already passed must not have its command
        // run late as a surprise side effect — `is_closed` is that caller
        // hanging up (1-g4 리뷰 발견 3).
        let steering = app.clone();
        tauri::async_runtime::spawn(async move {
            if request.answer.is_closed() {
                return;
            }
            // A pane with no fenced run leaves its browser steps where its
            // computer steps already go — the window's session folder — so
            // `recipe-save` keeps them and `evidence` shows the
            // `--value-stdin` line without its value (t-4228).
            let evidence = run_evidence::fenced_dir(
                steering.state::<AppState>().local_data_root(),
                request.evidence.as_deref(),
            )
            .or_else(|| computer_use::evidence::session_dir(now_epoch_ms()));
            let answer = browser_step(
                &steering,
                request.pane.as_deref(),
                evidence.as_deref(),
                &request.argv,
                &request.argv,
            )
            .await;
            let _ = request.answer.send(answer);
        });
    }
}

/// Dispatch launched-agent Computer Use requests while keeping the native
/// provider and its element-index cache in this window process.
pub(super) async fn computer_loop(
    app: AppHandle,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<zerocode_hookd::ComputerRequest>,
) {
    install_confirm_asker(app.clone());
    let sequences = computer_use::sequence::SequenceGate::default();
    while let Some(request) = requests.recv().await {
        let steering = app.clone();
        let sequences = sequences.clone();
        tauri::async_runtime::spawn(async move {
            if request.answer.is_closed() {
                return;
            }
            let argv = request.argv;
            let local_data_root = steering.state::<AppState>().local_data_root().to_path_buf();
            computer_use::evidence::set_root(&local_data_root);
            // A verb that reads or writes the step log waits for the steps
            // already answered to be written: the writer is behind the
            // answers by a settle and a frame.
            let parsed = zerocode_core::computer_use::parse_command(&argv).ok();
            // This guard outlives spawn_blocking and every walked step. The
            // provider mutex alone covered only one step of a batch.
            let _sequence = if let Some(command) = parsed.as_ref() {
                match sequences.enter(command) {
                    Ok(owner) => owner,
                    Err(error) => {
                        // Preserve a person's stop/confirmation over contention,
                        // and preserve the same evidence road as other refusals.
                        let answer = refused_at_the_door(command).unwrap_or_else(|| {
                            computer_cli_error(command.json, &error.code, &error.message)
                        });
                        leave_step(
                            &steering,
                            &local_data_root,
                            request.evidence.as_deref(),
                            &argv,
                            &argv,
                            &answer,
                        );
                        let _ = request.answer.send(answer);
                        return;
                    }
                }
            } else {
                None
            };
            if parsed
                .as_ref()
                .is_some_and(|command| evidence_runtime::reads_the_log(command.method))
            {
                evidence_runtime::written().await;
            }
            let evidence = request.evidence;
            let cwd = request.cwd;
            let pane = request.pane;
            let reply = request.answer;
            // A walk — a batch, a recipe — is the loop's to run: each step
            // goes down the lone command's road, and the loop knows when its
            // caller has gone.
            if let Some(command) = parsed
                .as_ref()
                .filter(|command| {
                    use zerocode_core::computer_use::ComputerMethod;
                    matches!(
                        command.method,
                        ComputerMethod::Batch | ComputerMethod::RecipeRun | ComputerMethod::Walk
                    )
                })
                .cloned()
            {
                use computer_use::confirm::Asking;
                let (app, root) = (steering.clone(), local_data_root.clone());
                let deadline_ms = zerocode_core::computer_use::computer_deadline_ms(&argv);
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    let _sequence = _sequence;
                    let answer = if command.method
                        == zerocode_core::computer_use::ComputerMethod::Batch
                    {
                        run_batch(
                            &command,
                            deadline_ms,
                            |step| {
                                desktop_step(
                                    &app,
                                    &root,
                                    evidence.as_deref(),
                                    step,
                                    step,
                                    Asking::Person,
                                )
                            },
                            |step, refusal| {
                                leave_step(&app, &root, evidence.as_deref(), step, step, refusal)
                            },
                            || !reply.is_closed(),
                        )
                    } else {
                        // The walk's folder, known before its first step: the
                        // run's, else the session's (born and catalogued the
                        // way a lone command's first step would).
                        let dir = arena_folder_adopted(&command).or_else(|| {
                            run_evidence::fenced_dir(&root, evidence.as_deref())
                                .or_else(session_folder_adopted)
                        });
                        // One set of roads for both walks: a recipe's steps
                        // and a goal walk's presses reach a screen the same
                        // way, so neither can acquire a road of its own.
                        let roads = RecipeRoads::new(
                            |tool, step, logged| match tool {
                                zerocode_core::computer_recipe::RecipeTool::Computer => {
                                    desktop_step(
                                        &app,
                                        &root,
                                        evidence.as_deref(),
                                        step,
                                        logged,
                                        Asking::HandBack,
                                    )
                                }
                                zerocode_core::computer_recipe::RecipeTool::Browser => {
                                    tauri::async_runtime::block_on(browser_step(
                                        &app,
                                        None,
                                        dir.as_deref(),
                                        step,
                                        logged,
                                    ))
                                }
                                // A walk is asked through the Computer Use
                                // door, which names no pane: like its browser
                                // steps, its `open` has no checkout to go to.
                                zerocode_core::computer_recipe::RecipeTool::Emulator => {
                                    tauri::async_runtime::block_on(emulator_step(
                                        &app,
                                        None,
                                        dir.as_deref(),
                                        cwd.as_deref().map(Path::new),
                                        step,
                                        logged,
                                    ))
                                }
                            },
                            |step, logged, refusal| {
                                leave_step(&app, &root, evidence.as_deref(), step, logged, refusal)
                            },
                            |level| {
                                if let Some(dir) = dir.as_deref() {
                                    evidence_runtime::leave_walk_begin(
                                        dir,
                                        level,
                                        serde_json::json!({
                                            "dir": dir, "name": command.params.get("name"),
                                            "kind": command.method.verb_name(),
                                            "cwd": cwd,
                                        }),
                                    );
                                }
                            },
                            |mut report| {
                                if let Some(dir) = dir.as_deref() {
                                    report["cwd"] = serde_json::json!(cwd);
                                    evidence_runtime::leave_walk_report(dir, report);
                                }
                            },
                        );
                        let desk =
                            || computer_use::recipe_run::LiveDesk::new().in_window(app.clone());
                        // The second reader (`--rescue`, t-6132 S3): the
                        // frontier, headless, under the window's own login —
                        // built here because only this loop holds the state
                        // the launch environment is read from.
                        let rescue = zerocode_core::computer_use::walk_rescues(&command.params)
                            .then(|| {
                                computer_use::errand::team::TeamJudge::new(
                                    app.state::<AppState>().config_root(),
                                    cwd.as_deref().map(Path::new),
                                )
                            })
                            .flatten();
                        if command.method == zerocode_core::computer_use::ComputerMethod::Walk {
                            // The value seat's writer (t-6720): the key a person set for it,
                            // read only when a walk's look has a field to type into.
                            let writer = computer_use::errand::value::LiveWriter::window();
                            run_goal(
                                &command,
                                deadline_ms,
                                dir.as_deref(),
                                cwd.as_deref().map(Path::new),
                                roads,
                                desk,
                                rescue,
                                writer,
                                // The window's own judge, built by the walk.
                                None,
                            )
                        } else {
                            run_recipe(
                                &command,
                                deadline_ms,
                                dir.as_deref(),
                                cwd.as_deref().map(Path::new),
                                roads,
                                desk,
                                || !reply.is_closed(),
                                rescue,
                            )
                        }
                    };
                    let _ = reply.send(answer);
                })
                .await;
                return;
            }
            // A verdict — and `evidence`, which reads and verifies the same
            // folder — is the loop's to answer: only here is the run's
            // folder known (§4). Rendering the report is file work, off
            // the loop's thread.
            let folder_verb = parsed.filter(|command| {
                use zerocode_core::computer_use::ComputerMethod;
                matches!(
                    command.method,
                    ComputerMethod::Verdict | ComputerMethod::Evidence
                )
            });
            let answer = if let Some(command) = folder_verb {
                let (app, root, line) = (steering.clone(), local_data_root.clone(), argv.clone());
                tauri::async_runtime::spawn_blocking(move || {
                    let run_dir = run_evidence::fenced_dir(&root, evidence.as_deref());
                    let dir =
                        run_dir.or_else(|| computer_use::evidence::session_dir(now_epoch_ms()));
                    let answer =
                        if command.method == zerocode_core::computer_use::ComputerMethod::Verdict {
                            answer_verdict(&command, &line, dir.as_deref())
                        } else {
                            said_envelope(&command, evidence_report(&command, dir.as_deref()))
                        };
                    leave_step(&app, &root, evidence.as_deref(), &line, &line, &answer);
                    answer
                })
                .await
                .unwrap_or_else(|join| computer_refused(format!("computer task failed: {join}")))
            } else if argv.first().is_some_and(|word| word == "emulator") {
                // The lone emulator command and a walk's emulator steps take
                // one road: the door, then the device's frame in the run's
                // folder when the shell presented one.
                let dir = run_evidence::fenced_dir(&local_data_root, evidence.as_deref());
                emulator_step(
                    &steering,
                    pane.as_deref(),
                    dir.as_deref(),
                    cwd.as_deref().map(Path::new),
                    &argv[1..],
                    &argv[1..],
                )
                .await
            } else if argv.first().is_some_and(|word| word == "ssh") {
                let answer = answer_ssh_command(&steering, &argv[1..]).await;
                leave_step(
                    &steering,
                    &local_data_root,
                    evidence.as_deref(),
                    &argv,
                    &argv,
                    &answer,
                );
                answer
            } else {
                let (app, root, desktop) =
                    (steering.clone(), local_data_root.clone(), argv.clone());
                tauri::async_runtime::spawn_blocking(move || {
                    let _sequence = _sequence;
                    desktop_step(
                        &app,
                        &root,
                        evidence.as_deref(),
                        &desktop,
                        &desktop,
                        computer_use::confirm::Asking::Person,
                    )
                })
                .await
                .unwrap_or_else(|join| computer_refused(format!("computer task failed: {join}")))
            };
            let _ = reply.send(answer);
        });
    }
}

/// One command line answered down the desktop road, and everything that
/// follows an answer: the one function a lone command and every step of a
/// walk take, so each is stopped, counted, confirmed and logged the same way.
/// `logged` is the line the log keeps — the command itself, or a recipe's
/// own words, so a value the walk was given never reaches the log.
pub(super) fn desktop_step(
    app: &AppHandle,
    local_data_root: &Path,
    presented_evidence: Option<&str>,
    argv: &[String],
    logged: &[String],
    asking: computer_use::confirm::Asking,
) -> zerocode_hookd::TeamAnswer {
    let began = std::time::Instant::now();
    // A command that panicked is answered and left like any refusal: its
    // line, the operator's memory and the band still hear of it.
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        answer_computer_command(argv, Some(app), asking)
    }))
    .unwrap_or_else(|panic| {
        computer_refused(format!(
            "computer task failed: {}",
            crate::system_runtime::panic_payload(panic.as_ref())
        ))
    });
    run_evidence::observing(
        run_evidence::measured(
            run_evidence::observation(),
            "computer",
            argv,
            began.elapsed(),
        )
        .unwrap_or_default(),
        || {
            leave_step(
                app,
                local_data_root,
                presented_evidence,
                argv,
                logged,
                &answer,
            )
        },
    );
    answer
}

/// What follows every answered command line (§1.4): its evidence — in the
/// run's folder when the shell presented one, else the session's — and, for
/// a desktop action, the band's count, the operator's memory of its hands,
/// and the activity event. `argv` is what ran; `logged` what the log and the
/// operator's memory keep of it.
pub(super) fn leave_step(
    app: &AppHandle,
    local_data_root: &Path,
    presented_evidence: Option<&str>,
    argv: &[String],
    logged: &[String],
    answer: &zerocode_hookd::TeamAnswer,
) {
    let run_dir = run_evidence::fenced_dir(local_data_root, presented_evidence);
    let is_desktop = !argv
        .first()
        .is_some_and(|word| word == "emulator" || word == "ssh");
    let session_dir = (run_dir.is_none() && is_desktop)
        .then(session_folder_adopted)
        .flatten();
    let capped = run_dir.is_none();
    if let Some(dir) = run_dir.or(session_dir) {
        evidence_runtime::leave_computer_evidence(
            &dir,
            logged,
            answer,
            capped,
            run_evidence::observation(),
        );
    }
    // The band: an action just happened (or was refused), here is where
    // the operator stands.
    if is_desktop
        && let Ok(command) = zerocode_core::computer_use::parse_command(argv)
        && command.method.acts()
    {
        let verb = command.method.verb_name();
        if answer.exit_code == 0 {
            computer_use::guard::note_action(verb, now_epoch_ms());
        }
        // The operator's memory of its hands (§7.2): what it did, whether
        // it worked — beside the evidence, for status.
        let refused = (answer.exit_code != 0).then(|| answer.stderr.trim().to_string());
        computer_use::state::note_action(
            verb,
            logged,
            answer.exit_code == 0,
            refused.as_deref(),
            now_epoch_ms(),
            computer_use::evidence::session_dir(now_epoch_ms()).as_deref(),
        );
        let _ = app.emit(
            "computer:activity",
            computer_use::guard::activity_report(Some(verb)),
        );
    }
}

/// The session's evidence folder — born now if none stands, and a new one
/// joins the artifacts catalogue as evidence, the way a run's does.
fn session_folder_adopted() -> Option<PathBuf> {
    computer_use::evidence::session_dir_born(now_epoch_ms()).map(|(dir, born)| {
        if born {
            crate::artifact_runtime::adopt_evidence(
                &dir.to_string_lossy(),
                EVIDENCE_AUTOMATION_ID,
                None,
            );
        }
        dir
    })
}

/// The automation the operator's own evidence folders are catalogued under.
const EVIDENCE_AUTOMATION_ID: &str = "computer-use";

/// An arena walk's folder (`recipe-run --arena`): born beside the session
/// folders for this walk, and catalogued as evidence the way a session's is
/// — so a rehearsal's report is an artifact and its lines are nobody's
/// desk's. None when the command asks for no arena.
fn arena_folder_adopted(command: &zerocode_core::computer_use::ComputerCommand) -> Option<PathBuf> {
    command.params.get("arena")?;
    let dir = computer_use::evidence::arena_dir(now_epoch_ms())?;
    crate::artifact_runtime::adopt_evidence(&dir.to_string_lossy(), EVIDENCE_AUTOMATION_ID, None);
    Some(dir)
}

/// One command line answered down the browser road, and its evidence after
/// the answer (§1.4): the one function the shim's request and every browser
/// step of a walk take, so each is dispatched and logged the same way.
/// `logged` is the line the log keeps — the command itself, or a recipe's
/// own words, so a value the walk was given never reaches the log.
pub(super) async fn browser_step(
    app: &AppHandle,
    pane: Option<&str>,
    dir: Option<&Path>,
    argv: &[String],
    logged: &[String],
) -> zerocode_hookd::TeamAnswer {
    let observation = run_evidence::observation();
    let began = std::time::Instant::now();
    let answer = answer_browser_command(app, argv, pane).await;
    if let Some(dir) = dir {
        evidence_runtime::leave_browser_evidence(
            app,
            dir,
            logged,
            &answer,
            run_evidence::measured(observation, "browser", argv, began.elapsed()),
        );
    }
    answer
}

/// One command line answered down the emulator road, and its evidence after
/// the answer (§1.4): the one function the lone `zerocode-emulator` command
/// and every emulator step of a walk take, so each is dispatched and logged
/// the same way (the browser door's twin). `argv`/`logged` are the words after
/// the door (no `emulator`); `logged` is what the log keeps — a recipe's own
/// words, so a value the walk was given never reaches the log. `cwd` is where
/// the shell that asked stands, when its door said so: a relative `--out`
/// is taken from there. `pane` is the pane key that door named, as on
/// [`browser_step`]: an `open` is seated in that pane's checkout.
pub(super) async fn emulator_step(
    app: &AppHandle,
    pane: Option<&str>,
    dir: Option<&Path>,
    cwd: Option<&Path>,
    argv: &[String],
    logged: &[String],
) -> zerocode_hookd::TeamAnswer {
    let observation = run_evidence::observation();
    let began = std::time::Instant::now();
    let answer = answer_emulator_command(app, argv, cwd, pane).await;
    if let Some(dir) = dir {
        evidence_runtime::leave_emulator_evidence(
            dir,
            logged,
            &answer,
            run_evidence::measured(observation, "emulator", argv, began.elapsed()),
        );
    }
    answer
}

/// A batch (§2.5): its steps in order, each down the road a lone command
/// takes (`step`), stopping at the first refusal, at a stop, or before a step
/// that could not finish before the bridge gives up at `deadline_ms` (or once
/// the caller has gone) — and answering every step that ran. A batch the stop
/// refuses whole leaves its first refused action with `refused`, as that
/// action would have left itself alone.
pub(super) fn run_batch(
    command: &zerocode_core::computer_use::ComputerCommand,
    deadline_ms: u64,
    mut step: impl FnMut(&[String]) -> zerocode_hookd::TeamAnswer,
    refused: impl FnOnce(&[String], &zerocode_hookd::TeamAnswer),
    caller_waits: impl Fn() -> bool,
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use::{
        Next, WalkBudget, batch_answer, batch_steps, batch_text, parse_command, walk,
        walk_step_holds_ms, walk_step_report,
    };
    let commands = batch_steps(command);
    if let Some(refusal) = refused_at_the_door(command) {
        if let Some((first, answer)) = commands.iter().find_map(|argv| {
            let at_the_door = refused_at_the_door(&parse_command(argv).ok()?)?;
            Some((argv, at_the_door))
        }) {
            refused(first, &answer);
        }
        return refusal;
    }
    /// Why a batch ended early: its bridge's time, or a step refused.
    #[derive(Clone, PartialEq)]
    enum Halt {
        Late,
        Refused,
    }
    let started = std::time::Instant::now();
    let walked = walk(
        &commands,
        &WalkBudget {
            deadline_ms,
            out_of_time: Halt::Late,
        },
        |_, argv| walk_step_holds_ms(argv, true),
        |_, argv: &Vec<String>| Next::Run(argv.clone()),
        |n, _, argv| {
            let answer = step(argv);
            let report = walk_step_report(
                n,
                argv,
                answer.exit_code == 0,
                evidence_runtime::envelope(&answer).as_ref(),
                &answer.stderr,
            );
            let halt = (report["ok"] != serde_json::Value::Bool(true)).then_some(Halt::Refused);
            (report, halt)
        },
        caller_waits,
        || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    let late = matches!(walked.halted, Some((_, Halt::Late)));
    let answer = batch_answer(walked.reports, commands.len(), walked.elapsed_ms, late);
    let text = if command.json {
        answer.to_string()
    } else {
        batch_text(&answer)
    };
    if answer["ok"] == serde_json::Value::Bool(true) {
        computer_said(format!("{text}\n"))
    } else {
        computer_refused(text)
    }
}

/// What a recipe walk is handed from the loop (§1.4): the road each door's
/// step goes down, what a refused first act leaves, and the one evidence
/// writer's two words for a walk — that it begins in its folder at its
/// Flow's evidence level, and its record when it ends. Handed in, never
/// written by the walk itself.
pub(super) struct RecipeRoads<S, L, B, E> {
    step: S,
    refused: L,
    begun: B,
    ended: E,
}

impl<S, L, B, E> RecipeRoads<S, L, B, E>
where
    S: FnMut(
        zerocode_core::computer_recipe::RecipeTool,
        &[String],
        &[String],
    ) -> zerocode_hookd::TeamAnswer,
    L: FnMut(&[String], &[String], &zerocode_hookd::TeamAnswer),
    B: FnMut(zerocode_core::computer_flow::EvidenceLevel),
    E: FnMut(serde_json::Value),
{
    /// The roads, by the bounds a closure is read against.
    pub(super) fn new(step: S, refused: L, begun: B, ended: E) -> Self {
        Self {
            step,
            refused,
            begun,
            ended,
        }
    }
}

/// A recipe (§7.2): its saved steps walked in one call, each down the road a
/// lone command takes (`step`, which hands a guarded press back, given the
/// line to run and the line to log), stopping where the person or a fresh
/// look is needed — and answering the step to run again from. A walk that
/// stopped before its end is refused like a batch that did, its report the
/// payload. A stopped operator's recipe is refused at the door, like any
/// action, and leaves its first step's refusal with `refused`, as that step
/// would have left itself alone; a walk that stops at the person's step
/// leaves that line with it too. A recipe that cannot start moves nothing.
///
/// The walk speaks to the one evidence writer (plan D4·D10): it says when it
/// begins in `dir` at its Flow's evidence level, and hands its record over
/// at the end. A Flow's verdict is written to the folder first (the judge's
/// word, as `verdict` would write it), then the walk's record lands after
/// every line, and only then is the folder's report rendered from its files.
///
/// `--repeat` walks round after round (`computer_use::repeat`): the
/// document's trigger is asked down the same step road between rounds, each
/// round is this same walk with a desk of its own (`desk_of`) and the time
/// the call has left, and the answer counts the rounds. `--arena` walks the
/// same document over a recorded folder (`computer_use::arena`): every step
/// answered as recorded, the desk the arena's, the evidence in the arena's
/// own folder (`dir`, chosen by the loop), the record of kind `arena` — and
/// no door asked, since nothing on a desk moves.
///
/// A walk that stops at a failed step or check may be judged
/// (`computer_use::recover`), and the Jev door judges it for `workspace` —
/// the folder the walk was asked from, as the Computer Use door said it. A
/// walk asked from nowhere known is judged for no workspace, which the door
/// refuses.
#[allow(clippy::too_many_arguments)] // The walk's roads, its desk, its caller and its second reader are each one seam a test replaces on its own.
pub(super) fn run_recipe(
    command: &zerocode_core::computer_use::ComputerCommand,
    deadline_ms: u64,
    dir: Option<&Path>,
    workspace: Option<&Path>,
    roads: RecipeRoads<
        impl FnMut(
            zerocode_core::computer_recipe::RecipeTool,
            &[String],
            &[String],
        ) -> zerocode_hookd::TeamAnswer,
        impl FnMut(&[String], &[String], &zerocode_hookd::TeamAnswer),
        impl FnMut(zerocode_core::computer_flow::EvidenceLevel),
        impl FnMut(serde_json::Value),
    >,
    mut desk_of: impl computer_use::arena::DeskOf,
    caller_waits: impl Fn() -> bool,
    mut rescue: Option<computer_use::errand::team::TeamJudge>,
) -> zerocode_hookd::TeamAnswer {
    use computer_use::arena::{self, Arena, ArenaDesk, Stage};
    use computer_use::recipe_run::{self, Desk as _, Run};
    use zerocode_core::computer_use::RepeatUntil;
    use zerocode_core::computer_use_protocol::error_code;
    let RecipeRoads {
        mut step,
        mut refused,
        mut begun,
        mut ended,
    } = roads;
    let Some(root) = computer_use::evidence::root() else {
        return computer_cli_error(
            command.json,
            error_code::ACCESSIBILITY_ERROR,
            RECIPES_ROOT_UNKNOWN,
        );
    };
    let name = command
        .params
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let (file, text) = match computer_use::recipes::show(&root, name) {
        Ok(shown) => shown,
        Err(error) => {
            return computer_cli_error(command.json, error_code::INVALID_ARGUMENT, &error.message);
        }
    };
    let file = file.display().to_string();
    // The arena: the recorded folder answers every step and stands in for
    // the desk; the road and the desk are its, the folder the loop's.
    let mut arena = match Arena::of(command) {
        Ok(arena) => arena,
        Err(why) => return computer_cli_error(command.json, error_code::INVALID_ARGUMENT, &why),
    };
    let rehearsal = arena.is_some();
    let pages = arena.as_ref().map(Arena::pages);
    let rehearsed = arena
        .as_ref()
        .map(|arena| arena.from().display().to_string());
    let mut arena_road = arena.as_mut().map(|arena| arena.road(dir));
    let mut road = |tool: zerocode_core::computer_recipe::RecipeTool,
                    argv: &[String],
                    logged: &[String]| match arena_road.as_mut() {
        Some(road) => road(tool, argv, logged),
        None => step(tool, argv, logged),
    };
    let mut leave = |argv: &[String], logged: &[String], answer: &zerocode_hookd::TeamAnswer| {
        if rehearsal {
            arena::leave(dir, logged, answer);
        } else {
            refused(argv, logged, answer);
        }
    };
    let mut stage = || match &pages {
        Some(pages) => Stage::Arena(ArenaDesk::new(pages.clone())),
        None => Stage::Live(desk_of.desk()),
    };
    // One round: the door (a stopped operator refuses a walk that acts; an
    // arena acts on nothing), the walk, and the folder's own words after
    // it — the verdict first, the record, the report from the files.
    let mut round = |road: &mut _,
                     desk: Stage<_>,
                     deadline_ms: u64,
                     start: Option<usize>|
     -> Result<serde_json::Value, computer_use::ComputerUseError> {
        if !rehearsal && let Some(door) = door_refusal(command) {
            if let Some((first, logged, at_the_door)) = recipe_run::first_act(command, &text)
                .and_then(|(first, logged)| {
                    let at_the_door = refused_at_the_door(
                        &zerocode_core::computer_use::parse_command(&first).ok()?,
                    )?;
                    Some((first, logged, at_the_door))
                })
            {
                leave(&first, &logged, &at_the_door);
            }
            return Err(door);
        }
        // An internal resume keeps the original request range and its own cursor.
        let mut report = recipe_run::run(
            &Run {
                command,
                resume_from: start,
                file: &file,
                text: &text,
                deadline_ms,
                cwd: workspace,
                evidence_dir: dir,
            },
            &mut *road,
            &mut leave,
            &mut begun,
            desk,
            &caller_waits,
        )
        .map_err(|refused| computer_use::ComputerUseError::new(refused.code, refused.message))?;
        if let Some(from) = &rehearsed {
            report["kind"] = serde_json::json!(arena::WALK_KIND);
            report[arena::WALK_KIND] = serde_json::json!({ "from": from });
        }
        if let Some(dir) = dir {
            let verdict = recipe_run::verdict_of(&report);
            if let Some((pass, reason)) = &verdict
                && let Err(why) = computer_use::evidence::write_verdict(
                    dir,
                    &verdict_words(*pass, reason.as_deref()),
                    *pass,
                    reason.as_deref(),
                    now_epoch_ms(),
                )
            {
                eprintln!("recipe-run: the Flow's verdict was not written: {why}");
            }
            ended(report.clone());
            if verdict.is_some() {
                // The report is rendered from the folder's files once the
                // walk's record is among them (§3.5) — after the writer,
                // never before.
                tauri::async_runtime::block_on(evidence_runtime::written());
                match computer_use::report::write(dir) {
                    Ok(Some(file)) => {
                        report["reportFile"] = serde_json::json!(file.display().to_string());
                    }
                    Ok(None) => {}
                    Err(why) => report["reportError"] = serde_json::json!(why),
                }
            }
        }
        Ok(report)
    };
    if command.params.get("repeat") != Some(&serde_json::Value::Bool(true)) {
        let mut report = match round(&mut road, stage(), deadline_ms, None) {
            Ok(report) => report,
            Err(door) => return computer_cli_error(command.json, &door.code, &door.message),
        };
        // A walk that stopped because a step or a check failed may try to
        // clear what stopped it: the screen already numbers what a person
        // could press, and one of those numbers — chosen, never invented —
        // is pressed before the stopped step is walked again. Off by default,
        // and off is byte-for-byte today's walk.
        if command.params.get("end").is_some() {
            return recipe_answer(command, &report);
        }
        let seat = computer_use::errand::seat_of(computer_use::errand::Surface::Page);
        let mode = computer_use::errand::mode_now(seat);
        if mode != computer_use::errand::Mode::Off {
            let lines = recipe_run::decide(command, &text)
                .map(|(lines, _, _)| lines)
                .unwrap_or_default();
            // The Jev door sends only for a workspace a person consented to,
            // and a walk's workspace is the folder it was asked from; asked
            // from nowhere known, the door refuses and the row says so
            // (docs/design/jev-settings-20260917.md §3).
            // A recipe walked again is a repeated run by what it is: its
            // stopped step asks the questions a walk of it asked before
            // (t-6385).
            let mut judge = computer_use::errand::live::LiveJudge::new(
                &crate::api_routers::Keychain::of_this_machine(),
                workspace,
                seat,
            )
            .in_run(zerocode_core::jev::Run::Repeated);
            // No key, nothing to ask — and so no reason to measure the page or
            // number its controls first.
            if let Some(read) =
                computer_use::errand::read_report(&report, &lines).filter(|_| judge.armed())
            {
                let flow = zerocode_core::computer_flow::parse_flow(&text)
                    .ok()
                    .flatten();
                let at = computer_use::errand::Errand {
                    goal: name,
                    why: computer_use::errand::Why::Cleared {
                        stop: read.stop,
                        step: &read.step,
                        refusal: &read.refusal,
                        next: read.next,
                    },
                    flow: flow.as_ref(),
                    moves_money: zerocode_core::computer_recipe::money_step(&lines)
                        .ok()
                        .flatten()
                        .is_some(),
                };
                // The address is read once, here, outside the judgment's own
                // deadline — a `marks` answer does not carry it.
                let page = {
                    let mut desk = stage();
                    computer_use::errand::walk::page_of(&desk.pages(), &read.pane)
                };
                let spent = report["elapsedMs"].as_u64().unwrap_or_default();
                let mut walk_again = |road: &mut _, left: u64, step: usize| {
                    round(road, stage(), left, Some(step)).ok()
                };
                let mut world = computer_use::errand::walk::WalkWorld::new(
                    &mut road,
                    &mut walk_again,
                    &read.pane,
                    page,
                    deadline_ms,
                    spent,
                );
                // Whether the seat presses: a person's `on`, or an `auto`
                // its own ledger has promoted — read off the wire the
                // questions go down, so the standing and the answers come
                // from one settings file and one ledger root.
                let acting = crate::systemone::applies(judge.wire(), seat);
                let options = computer_use::errand::Options {
                    overlap: false,
                    rescue: rescue.is_some(),
                    act_line: crate::systemone::act_line(judge.wire(), seat),
                };
                let recovered = computer_use::errand::run_with(
                    mode,
                    acting,
                    computer_use::errand::Branching::OFF,
                    &at,
                    &mut judge,
                    &mut world,
                    options,
                    rescue
                        .as_mut()
                        .map(|team| team as &mut dyn computer_use::errand::ActionJudge),
                );
                computer_use::errand::write_rows(
                    seat,
                    judge.wire(),
                    dir,
                    &recovered.rows,
                    crate::project_runtime::now_epoch_ms(),
                );
                judge.write_memo_rows(dir, crate::project_runtime::now_epoch_ms());
                if let Some(walked) = recovered.report {
                    report = walked;
                }
            }
        }
        return recipe_answer(command, &report);
    }
    // The repeat: the document's trigger (a document that does not read is
    // the round's to refuse), its bound, the local clock for a time of day.
    let flow = zerocode_core::computer_flow::parse_flow(&text)
        .ok()
        .flatten();
    let plan = computer_use::repeat::Plan {
        trigger: flow.as_ref().and_then(|spec| spec.trigger.as_ref()),
        until: command
            .params
            .get("until")
            .and_then(serde_json::Value::as_str)
            .and_then(|word| RepeatUntil::parse(word).ok()),
        deadline_ms,
        local_offset_secs: crate::automation_runtime::local_offset_secs(),
    };
    // A repeat walks whole rounds, so its round never resumes from a step.
    let round_once = |road: &mut _, desk, deadline| round(road, desk, deadline, None);
    let repeated = computer_use::repeat::repeat(
        &plan,
        stage,
        road,
        round_once,
        || !rehearsal && door_refusal(command).is_some(),
        &caller_waits,
    );
    repeat_answer(command, &repeated)
}

/// A walk toward a goal (`walk`): the one step of a procedure whose next
/// press cannot be written down in advance.
///
/// Everything the caller already knows the shape of belongs in a `batch`,
/// which spends no judgment at all; this is where the screen decides, and it
/// is the only place a judgment is asked. The loop owns it for the same
/// reason it owns a batch and a recipe: it drives several commands down the
/// lone command's road, and only here is the folder the walk was asked from
/// known — which is the workspace the Jev door reads consent for.
///
/// The seat is the surface's, never the verb's: a walk at a browser pane is
/// under the browser row of the Jev use table and a walk at an app is under
/// the desktop row, so neither switch can turn the other's surface on.
#[allow(clippy::too_many_arguments)] // Its roads, desk, second reader, value writer and judge are each one seam.
pub(super) fn run_goal(
    command: &zerocode_core::computer_use::ComputerCommand,
    deadline_ms: u64,
    dir: Option<&Path>,
    workspace: Option<&Path>,
    roads: RecipeRoads<
        impl FnMut(
            zerocode_core::computer_recipe::RecipeTool,
            &[String],
            &[String],
        ) -> zerocode_hookd::TeamAnswer,
        impl FnMut(&[String], &[String], &zerocode_hookd::TeamAnswer),
        impl FnMut(zerocode_core::computer_flow::EvidenceLevel),
        impl FnMut(serde_json::Value),
    >,
    mut desk_of: impl computer_use::arena::DeskOf,
    mut rescue: Option<computer_use::errand::team::TeamJudge>,
    writer: computer_use::errand::value::LiveWriter,
    judge: Option<computer_use::errand::live::LiveJudge>,
) -> zerocode_hookd::TeamAnswer {
    use computer_use::errand::{self, desk};
    use computer_use::recipe_run::Desk as _;
    let RecipeRoads {
        step,
        mut begun,
        mut ended,
        ..
    } = roads;
    let mut road = step;
    begun(zerocode_core::computer_flow::EvidenceLevel::Full);
    let mut said = |value: serde_json::Value| -> zerocode_hookd::TeamAnswer {
        let mut report = value.clone();
        report["kind"] = serde_json::json!("walk");
        report["at_epoch_ms"] = serde_json::json!(now_epoch_ms());
        ended(report);
        if let Some(dir) = dir {
            tauri::async_runtime::block_on(evidence_runtime::written());
            if let Err(why) = computer_use::report::write(dir) {
                eprintln!("walk: the report was not written: {why}");
            }
        }
        if command.json {
            computer_said(format!(
                "{}\n",
                serde_json::json!({ "ok": true, "result": value })
            ))
        } else {
            computer_said(format!("{}\n", goal_text(&value)))
        }
    };
    // A stopped operator refuses a walk exactly as it refuses one press.
    if let Some(door) = refused_at_the_door(command) {
        return door;
    }
    let word = |key: &str| {
        command
            .params
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let goal = word("goal").unwrap_or_default();
    let (aim, page) = match (word("pane"), word("app"), word("device")) {
        (Some(label), None, None) => {
            // The pane's address, read once before the walk: a `marks` answer
            // does not carry it, and a round trip for it inside the
            // judgment's own deadline would spend the clock the walk needs.
            let mut desk = desk_of.desk();
            let (host, path) = errand::walk::page_of(&desk.pages(), &label);
            (desk::Aim::Pane { label }, errand::Seen::Page { host, path })
        }
        (None, Some(name), None) => (desk::Aim::App { name }, errand::Seen::default()),
        (None, None, Some(device)) => {
            let Some(platform) = word("platform").and_then(|value| value.parse().ok()) else {
                return computer_cli_error(
                    command.json,
                    zerocode_core::computer_use_protocol::error_code::INVALID_ARGUMENT,
                    "a mobile walk needs --platform ios|android",
                );
            };
            (
                desk::Aim::Phone { platform, device },
                errand::Seen::default(),
            )
        }
        // The parser already refuses neither and both; this is the shape the
        // type system cannot be told about.
        _ => {
            return computer_cli_error(
                command.json,
                zerocode_core::computer_use_protocol::error_code::INVALID_ARGUMENT,
                "a walk looks at one screen: name --app, --pane, or --platform with --device",
            );
        }
    };
    let seat = errand::seat_of(aim.surface());
    let at = errand::Errand {
        goal: &goal,
        why: errand::Why::Goal {
            steps: zerocode_core::computer_use::walk_steps(&command.params),
        },
        flow: None,
        // A goal walk carries no document, so nothing here declares a money
        // step. What keeps it off the money is the door every press goes
        // through — the same guard, the same confirmation — and the fact that
        // it can only press, never type an amount or a recipient.
        moves_money: false,
    };
    // The judge this walk asks: one its caller built — a test's, across a
    // socket of its own — else the window's, the key the settings pane keeps,
    // for the folder the walk was asked from and the surface's own seat.
    let mut judge = judge
        .unwrap_or_else(|| {
            errand::live::LiveJudge::new(
                &crate::api_routers::Keychain::of_this_machine(),
                workspace,
                seat,
            )
        })
        .in_run(zerocode_core::computer_use::walk_run(&command.params));
    // Every switch this walk reads, it reads from the settings file its judge
    // asks through — for the window's judge the file `errand::mode_now`
    // reads (`zo_settings_path`), so the seat's mode and whether it acts come
    // from one reading of one file.
    let settings = judge.wire().settings_root();
    let mode = seat.mode_in(&settings);
    if mode == errand::Mode::Off || !judge.armed() {
        // Off is today's product exactly: no look is taken, nothing is sent,
        // and the answer says plainly that nothing walked.
        return said(serde_json::json!({
            "goal": goal,
            "mode": mode.key(),
            "pressed": 0,
            "reached": false,
            "steps": [],
        }));
    }
    let acting = crate::systemone::applies(judge.wire(), seat);
    // The branching seat's standing (t-6044), read off the same wire and the
    // same settings file as the screen seat's: whether a phone step whose
    // judgment ranked two or more controls is forked, and whether the
    // comparison's pick is the one pressed.
    let forks = &zerocode_core::jev::BRANCHING;
    let branching = errand::Branching {
        mode: forks.mode_in(&settings),
        acting: crate::systemone::applies(judge.wire(), forks),
        act_line: crate::systemone::act_line(judge.wire(), forks),
    };
    let snapshots: Box<dyn desk::Snapshots> = Box::new(AvdSnapshots {
        device: word("device").unwrap_or_default(),
    });
    let options = errand::Options {
        overlap: zerocode_core::computer_use::walk_overlaps(&command.params),
        rescue: rescue.is_some(),
        act_line: crate::systemone::act_line(judge.wire(), seat),
    };
    let mut world = desk::GoalWorld::new(&mut road, aim, page, word("until"), deadline_ms, 0)
        .with_snapshots(snapshots)
        .previewing(options.overlap)
        .writing(Box::new(writer));
    let walked = errand::run_with(
        mode,
        acting,
        branching,
        &at,
        &mut judge,
        &mut world,
        options,
        rescue
            .as_mut()
            .map(|team| team as &mut dyn errand::ActionJudge),
    );
    errand::write_rows(
        seat,
        judge.wire(),
        dir,
        &walked.rows,
        crate::project_runtime::now_epoch_ms(),
    );
    errand::write_rows(
        forks,
        judge.wire(),
        dir,
        &walked.forks,
        crate::project_runtime::now_epoch_ms(),
    );
    judge.write_memo_rows(dir, crate::project_runtime::now_epoch_ms());
    said(serde_json::json!({
        "goal": goal,
        "mode": mode.key(),
        "pressed": walked.pressed,
        "reached": walked.reached.unwrap_or_default(),
        "steps": walked.rows,
    }))
}

/// An Android AVD's saved states, as a forked step drives them (t-6044): the
/// device the walk was aimed at by the name `list` shows, resolved to its
/// live serial the way every other Android verb resolves it
/// (`active_android_serial`), then the emulator console's own `avd snapshot`
/// road. Built for every mobile walk; the world keeps it only for Android.
struct AvdSnapshots {
    device: String,
}

impl AvdSnapshots {
    fn drive(&self, verb: crate::emulator::AvdSnapshot, name: &str) -> Result<u64, String> {
        let serial = tauri::async_runtime::block_on(active_android_serial(&self.device))?;
        crate::emulator::android_avd_snapshot(&serial, verb, name)
    }
}

impl computer_use::errand::desk::Snapshots for AvdSnapshots {
    fn save(&mut self, name: &str) -> Result<u64, String> {
        self.drive(crate::emulator::AvdSnapshot::Save, name)
    }

    fn load(&mut self, name: &str) -> Result<u64, String> {
        self.drive(crate::emulator::AvdSnapshot::Load, name)
    }

    fn delete(&mut self, name: &str) {
        if let Err(why) = self.drive(crate::emulator::AvdSnapshot::Delete, name) {
            eprintln!("walk: a fork's snapshot was left on the device: {why}");
        }
    }
}

/// A walk's answer in words, for a caller that did not ask for JSON.
///
/// The count carries a reason after it when one of the rows gave one: read
/// alone, `0 press(es)` says "nothing on that screen was worth pressing",
/// which is exactly what a walk under a recording seat does not mean
/// (t-5455).
pub(super) fn goal_text(said: &serde_json::Value) -> String {
    let pressed = said["pressed"].as_u64().unwrap_or_default();
    let reached = said["reached"] == serde_json::Value::Bool(true);
    let goal = said["goal"].as_str().unwrap_or_default();
    let got = if reached { "reached" } else { "not reached" };
    let why = said["steps"]
        .as_array()
        .and_then(|steps| computer_use::errand::no_press_reason(steps))
        .map_or_else(String::new, |reason| format!(" — {reason}"));
    format!("{goal}: {got} after {pressed} press(es){why}")
}

/// One walk's answer: the report when it walked to its end, else the stop
/// as a refusal with the report as its payload.
fn recipe_answer(
    command: &zerocode_core::computer_use::ComputerCommand,
    report: &serde_json::Value,
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use_protocol::error_code;
    let text = computer_use::recipe_run::text(report);
    if report["done"] == serde_json::Value::Bool(true) {
        return if command.json {
            computer_said(format!(
                "{}\n",
                serde_json::json!({ "ok": true, "result": report })
            ))
        } else {
            computer_said(format!("{text}\n"))
        };
    }
    if command.json {
        let stopped = text.lines().last().unwrap_or_default().to_string();
        computer_refused(
            serde_json::json!({
                "ok": false,
                "error": { "code": error_code::RECIPE_STOPPED, "message": stopped },
                "result": report,
            })
            .to_string(),
        )
    } else {
        computer_refused(text)
    }
}

/// A repeat's answer: its rounds, counted, when it ended as asked — its
/// bound, the person, the call's clock — else the fail ceiling as a
/// refusal with the rounds as its payload. The last round's report rides
/// along either way.
fn repeat_answer(
    command: &zerocode_core::computer_use::ComputerCommand,
    repeated: &computer_use::repeat::Repeated,
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use_protocol::error_code;
    let result = repeated.value();
    let text = repeated
        .last
        .as_ref()
        .filter(|last| last.get("ran").is_some())
        .map(computer_use::recipe_run::text)
        .into_iter()
        .chain(std::iter::once(repeated.text()))
        .collect::<Vec<_>>()
        .join("\n");
    match (repeated.ended.as_asked(), command.json) {
        (true, true) => computer_said(format!(
            "{}\n",
            serde_json::json!({ "ok": true, "result": result })
        )),
        (true, false) => computer_said(format!("{text}\n")),
        (false, true) => computer_refused(
            serde_json::json!({
                "ok": false,
                "error": { "code": error_code::RECIPE_STOPPED, "message": repeated.text() },
                "result": result,
            })
            .to_string(),
        ),
        (false, false) => computer_refused(text),
    }
}

/// A stop the helper answers with, taken in by the window (`guard::adopt`).
fn adopt_helper_stop(helper: &serde_json::Value) {
    if helper.get("stopped").and_then(serde_json::Value::as_bool) == Some(true)
        && let Some(reason) = helper.get("reason").and_then(serde_json::Value::as_str)
    {
        computer_use::guard::adopt(reason);
    }
}

/// The person's stop, when one stands — the window's own (its button, or a
/// chord it took in) or the helper's chord.
fn persons_stop_standing() -> Option<String> {
    let persons = |reason: &String| zerocode_core::computer_use::persons_stop(reason);
    computer_use::guard::stopped_reason()
        .filter(persons)
        .or_else(|| {
            computer_use::session_stands()
                .then(|| computer_use::call("status", serde_json::json!({})).ok())
                .flatten()
                .and_then(|helper| helper.get("reason")?.as_str().map(str::to_string))
                .filter(persons)
        })
}

/// Where the recipes live is the window's to say; before it has, no recipe
/// verb has a folder to read.
const RECIPES_ROOT_UNKNOWN: &str = "the window has not told the operator where its data lives yet";

/// The one hand (§1.3): while stopped, every action is refused at the door,
/// before anything goes near the helper; looks still answer. So is every
/// action while the person is being asked (§1.5) — a press or a key sent
/// meanwhile could land on the question's own Allow.
/// Why a command that acts is refused at the door, if it is: the operator
/// is stopped (the hotkey, `stop`, a budget), or the person is being asked
/// about a step (or has the desk). A look is never refused here.
fn door_refusal(
    command: &zerocode_core::computer_use::ComputerCommand,
) -> Option<computer_use::ComputerUseError> {
    if !command.method.acts() {
        return None;
    }
    match computer_use::guard::stopped_reason() {
        Some(reason) => Some(computer_use::guard::refusal(&reason)),
        None if computer_use::confirm::asking() => Some(computer_use::ComputerUseError::new(
            zerocode_core::computer_use_protocol::error_code::PERSON_ASKED,
            "the person is being asked about a step (or has the desk); no other action goes until they answer — wait for that command's answer",
        )),
        None => None,
    }
}

/// The door's refusal as the command's own answer.
fn refused_at_the_door(
    command: &zerocode_core::computer_use::ComputerCommand,
) -> Option<zerocode_hookd::TeamAnswer> {
    door_refusal(command)
        .map(|refusal| computer_cli_error(command.json, &refusal.code, &refusal.message))
}

/// The agent's mobile road. `open` crosses into the frontend because the tab,
/// splitter placement and binary frame channel are window-owned; every later
/// action calls the same Rust backend as that tab. No desktop click is used to
/// manipulate Simulator or Android Emulator chrome.
pub(super) async fn answer_emulator_command(
    app: &AppHandle,
    argv: &[String],
    cwd: Option<&Path>,
    pane: Option<&str>,
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use::{
        EmulatorMethod, EmulatorPlatform, emulator_usage, parse_emulator_command,
    };

    if argv
        .first()
        .is_some_and(|word| matches!(word.as_str(), "help" | "-h" | "--help"))
    {
        return computer_said(format!("{}\n", emulator_usage()));
    }
    let wants_json = argv.iter().any(|word| word == "--json");
    let command = match parse_emulator_command(argv) {
        Ok(command) => command,
        Err(error) => return emulator_cli_error(wants_json, "invalid_argument", &error),
    };
    let platform = command.platform;
    let device = command.device.clone();
    if let (Some(platform), Some(device)) = (platform, device.as_deref()) {
        crate::emulator::used_through_the_door(platform, device);
    }
    let result: Result<serde_json::Value, String> = match command.method {
        EmulatorMethod::Marks
        | EmulatorMethod::Click
        | EmulatorMethod::Find
        | EmulatorMethod::Foreground => {
            return answer_emulator_observation(command).await;
        }
        EmulatorMethod::List => {
            let (ios, android) =
                tokio::join!(mobile_emulators_direct(), android_emulators_direct());
            match (ios, android) {
                (Ok(ios), Ok(android)) => Ok(json!({
                    "surface": "zerocode-built-in",
                    "ios": ios,
                    "android": android,
                })),
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        }
        EmulatorMethod::Open => {
            let platform = platform.expect("parser requires a platform");
            // Seated where it was asked for, not where the person is looking
            // (t-6379), the way the browser door's `open` is.
            let payload = crate::emulator::AgentOpen::asked(platform, device, pane);
            app.emit_to("main", "emulator:agent-open", payload)
                .map(|()| {
                    json!({
                        "surface": "zerocode-built-in",
                        "opening": true,
                        "platform": platform.as_str(),
                    })
                })
                .map_err(|error| error.to_string())
        }
        EmulatorMethod::Tree => {
            let device = device.expect("parser requires a device");
            match platform.expect("parser requires a platform") {
                EmulatorPlatform::Ios => ios_accessibility_tree_direct(device).await,
                EmulatorPlatform::Android => match active_android_serial(&device).await {
                    Ok(serial) => android_accessibility_tree_direct(serial).await,
                    Err(error) => Err(error),
                },
            }
        }
        EmulatorMethod::Tap => {
            let device = device.expect("parser requires a device");
            let x = command.x.expect("parser requires x");
            let y = command.y.expect("parser requires y");
            let done = match platform.expect("parser requires a platform") {
                EmulatorPlatform::Ios => ios_tap_direct(device, x, y).await,
                EmulatorPlatform::Android => match active_android_serial(&device).await {
                    Ok(serial) => android_tap_direct(serial, x, y).await,
                    Err(error) => Err(error),
                },
            };
            done.map(|()| json!({ "performed": true }))
        }
        EmulatorMethod::Swipe => {
            let device = device.expect("parser requires a device");
            let (x1, y1, x2, y2) = (
                command.x1.expect("parser requires x1"),
                command.y1.expect("parser requires y1"),
                command.x2.expect("parser requires x2"),
                command.y2.expect("parser requires y2"),
            );
            let done = match platform.expect("parser requires a platform") {
                EmulatorPlatform::Ios => ios_swipe_direct(device, x1, y1, x2, y2, command.ms).await,
                EmulatorPlatform::Android => match active_android_serial(&device).await {
                    Ok(serial) => android_swipe_direct(serial, x1, y1, x2, y2, command.ms).await,
                    Err(error) => Err(error),
                },
            };
            done.map(|()| json!({ "performed": true }))
        }
        EmulatorMethod::Text => {
            let device = device.expect("parser requires a device");
            let text = command.text.expect("parser requires text");
            let done = match platform.expect("parser requires a platform") {
                EmulatorPlatform::Ios => ios_text_direct(device, text).await,
                EmulatorPlatform::Android => match active_android_serial(&device).await {
                    Ok(serial) => android_text_direct(serial, text).await,
                    Err(error) => Err(error),
                },
            };
            done.map(|()| json!({ "performed": true }))
        }
        EmulatorMethod::Button => {
            let device = device.expect("parser requires a device");
            let name = command.name.expect("parser requires a button");
            let done = match platform.expect("parser requires a platform") {
                EmulatorPlatform::Ios => ios_button_direct(device, name).await,
                EmulatorPlatform::Android => match active_android_serial(&device).await {
                    Ok(serial) => android_button_direct(serial, name).await,
                    Err(error) => Err(error),
                },
            };
            done.map(|()| json!({ "performed": true }))
        }
        EmulatorMethod::Rotate => {
            let device = device.expect("parser requires a device");
            let rotation = command.rotation.expect("parser requires rotation");
            let done = match platform.expect("parser requires a platform") {
                EmulatorPlatform::Ios => ios_rotate_direct(device, rotation).await,
                EmulatorPlatform::Android => match active_android_serial(&device).await {
                    Ok(serial) => android_rotate_direct(serial, rotation).await,
                    Err(error) => Err(error),
                },
            };
            done.map(|()| json!({ "performed": true }))
        }
        EmulatorMethod::Screenshot => {
            let device = device.expect("parser requires a device");
            let platform = platform.expect("parser requires a platform");
            capture_emulator_screenshot(platform, device, command.out.as_deref(), cwd).await
        }
    };

    emulator_answer(
        command.method,
        command.json,
        result.map_err(crate::emulator::marks::backend_error),
    )
}

pub(super) async fn answer_emulator_observation(
    command: zerocode_core::computer_use::EmulatorCommand,
) -> zerocode_hookd::TeamAnswer {
    use crate::emulator::marks;
    use zerocode_core::computer_use::{EmulatorMethod, EmulatorPlatform};
    let result = async {
        let platform = command.platform.ok_or_else(|| {
            zerocode_core::computer_use_protocol::ProviderError::invalid_argument(
                "missing --platform",
            )
        })?;
        let device = command.device.ok_or_else(|| {
            zerocode_core::computer_use_protocol::ProviderError::invalid_argument(
                "missing --device",
            )
        })?;
        let device = match platform {
            EmulatorPlatform::Android => active_android_mark_target(&device)
                .await
                .map_err(marks::backend_error)?,
            EmulatorPlatform::Ios => marks::Device {
                identity: device.clone(),
                address: device,
            },
        };
        match command.method {
            EmulatorMethod::Marks => {
                marks::observe(platform, device, command.text.as_deref()).await
            }
            EmulatorMethod::Find | EmulatorMethod::Foreground => {
                crate::emulator::checks::observe(
                    platform,
                    device,
                    command.method,
                    if command.method == EmulatorMethod::Find {
                        command.text.as_deref()
                    } else {
                        command.app.as_deref()
                    }
                    .unwrap_or_default(),
                )
                .await
            }
            _ => {
                marks::click(
                    platform,
                    device,
                    command.mark.unwrap_or_default(),
                    command.look.as_deref().unwrap_or_default(),
                    command.text.as_deref(),
                    command.preview,
                )
                .await
            }
        }
    }
    .await;
    emulator_answer(command.method, command.json, result)
}

pub(super) fn emulator_answer(
    method: zerocode_core::computer_use::EmulatorMethod,
    json: bool,
    result: Result<serde_json::Value, zerocode_core::computer_use_protocol::ProviderError>,
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use::EmulatorMethod;
    let device_data = matches!(
        method,
        EmulatorMethod::Marks
            | EmulatorMethod::Click
            | EmulatorMethod::Find
            | EmulatorMethod::Foreground
    );
    match result {
        Ok(mut result) if json => {
            if device_data {
                result[zerocode_core::untrusted::JSON_FLAG] = true.into();
            }
            computer_said(format!("{}\n", json!({ "ok": true, "result": result })))
        }
        Ok(result) => {
            let text = emulator_pretty(method, &result);
            computer_said(if device_data {
                zerocode_core::untrusted::fence("emulator", &text, text.len())
            } else {
                format!("{text}\n")
            })
        }
        Err(error) => emulator_cli_error(json, &error.code, &error.message),
    }
}

/// The frame of one emulator, as PNG bytes, from the backend that paints it.
pub(super) async fn emulator_screenshot_bytes(
    platform: zerocode_core::computer_use::EmulatorPlatform,
    device: &str,
) -> Result<Vec<u8>, String> {
    use zerocode_core::computer_use::EmulatorPlatform;

    match platform {
        EmulatorPlatform::Ios => ios_screenshot_direct(device.to_string()).await,
        EmulatorPlatform::Android => {
            let serial = active_android_serial(device).await?;
            android_screenshot_direct(serial).await
        }
    }
}

pub(super) async fn capture_emulator_screenshot(
    platform: zerocode_core::computer_use::EmulatorPlatform,
    device: String,
    out: Option<&str>,
    cwd: Option<&Path>,
) -> Result<serde_json::Value, String> {
    let bytes = emulator_screenshot_bytes(platform, &device).await?;
    let byte_count = bytes.len();
    let path = write_agent_screenshot("emulator", &bytes, out, cwd)?;
    Ok(json!({
        "surface": "zerocode-built-in",
        "platform": platform.as_str(),
        "device": device,
        "path": path,
        "bytes": byte_count,
    }))
}

/// Android actions address the AVD name shown by `list`; the native input
/// backend addresses its live `emulator-NNNN` serial. Resolve the former only
/// after the built-in pane has started it, and never start a headless/external
/// emulator behind the pane's back.
pub(super) async fn active_android_serial(device: &str) -> Result<String, String> {
    active_android_mark_target(device)
        .await
        .map(|target| target.address)
}

async fn active_android_mark_target(
    device: &str,
) -> Result<crate::emulator::marks::Device, String> {
    let devices = serde_json::to_value(android_emulators_direct().await?)
        .map_err(|error| error.to_string())?;
    android_mark_target(&devices, device)
}

pub(super) fn android_mark_target(
    devices: &serde_json::Value,
    device: &str,
) -> Result<crate::emulator::marks::Device, String> {
    devices
        .as_array()
        .and_then(|devices| {
            devices.iter().find_map(|candidate| {
                let avd = candidate.get("avd")?.as_str()?.trim();
                let serial = candidate.get("serial")?.as_str()?.trim();
                (candidate.get("booted")?.as_bool()? && !avd.is_empty() && !serial.is_empty()
                    && (avd == device || serial == device))
                    .then(|| crate::emulator::marks::Device {
                        address: serial.to_string(),
                        identity: avd.to_string(),
                    })
            })
        })
        .ok_or_else(|| {
            format!(
                "Android device `{device}` is not active in a ZeroCode emulator pane; open its listed ID with `zerocode-emulator open --platform android --device <listed-id>` first"
            )
        })
}

/// The agent's road to the machines this window can already reach.
///
/// Every verb lands in a PANE, and that is the whole design. An SSH session
/// here IS a terminal (`SshConnection::open_pty`), so the door that opens one
/// also opens a local shell and a remote workspace — one vocabulary for every
/// surface ZeroCode owns — and nothing an agent does on a remote machine
/// happens anywhere the person cannot watch it, scroll it and kill it. No
/// desktop click is used to drive this window's own chrome.
pub(super) async fn answer_ssh_command(
    app: &AppHandle,
    argv: &[String],
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use::{SshMethod, parse_ssh_command, ssh_usage};

    if argv
        .first()
        .is_some_and(|word| matches!(word.as_str(), "help" | "-h" | "--help"))
    {
        return computer_said(format!("{}\n", ssh_usage()));
    }
    let wants_json = argv.iter().any(|word| word == "--json");
    let command = match parse_ssh_command(argv) {
        Ok(command) => command,
        Err(error) => return ssh_cli_error(wants_json, "invalid_argument", &error),
    };
    let result = match command.method {
        SshMethod::List => ssh_agent_list(app).await,
        SshMethod::Open => ssh_agent_open(app, &command).await,
        SshMethod::Send => ssh_agent_send(app, &command).await,
        SshMethod::Read => ssh_agent_read(app, &command),
    };
    match result {
        Ok(result) if command.json => {
            computer_said(format!("{}\n", json!({ "ok": true, "result": result })))
        }
        Ok(result) => computer_said(format!("{}\n", ssh_pretty(command.method, &result))),
        Err(error) => ssh_cli_error(command.json, "ssh_error", &error),
    }
}

/// Every machine this window can dial and every pane it currently holds.
///
/// The host rows are TRIMMED on purpose: an agent needs an id to open and a
/// name to recognise, not the pinned key material the settings pane reads.
pub(super) async fn ssh_agent_list(app: &AppHandle) -> Result<serde_json::Value, String> {
    let (repository, service) = {
        let state = app.state::<AppState>();
        (Arc::clone(state.settings()), state.ssh_hosts().clone())
    };
    let (hosts, targets) = run_remote_host_task(move || {
        let hosts = ssh_hosts_report(&repository, &service)?;
        let targets = ssh_targets_list(&repository);
        Ok((hosts, targets))
    })
    .await?;
    let hosts: Vec<serde_json::Value> = hosts
        .hosts
        .iter()
        .map(|host| {
            json!({
                "id": host.id,
                "kind": "ssh-host",
                "label": host.label,
                "address": host.host,
                "port": host.port,
                "user": host.user,
                "credential": host.credential_status,
            })
        })
        .chain(targets.iter().map(|target| {
            json!({
                "id": target.id,
                "kind": "remote-workspace",
                "label": target.label,
                "address": if target.config_host.is_empty() { &target.host } else { &target.config_host },
                "port": target.port,
                "user": target.username,
                "source": target.source,
            })
        }))
        .collect();
    let panes = {
        let state = app.state::<AppState>();
        let entries = state.terminals().entries();
        let mut panes: Vec<(TermId, serde_json::Value)> = entries
            .iter()
            .map(|(term, held)| {
                let pty = lock_pty(held);
                let grid = pty.terminal().grid();
                (
                    *term,
                    json!({
                        "pane": term,
                        "rows": grid.rows(),
                        "cols": grid.cols(),
                        "pid": pty.pid(),
                        "running": pty.foreground_programs(),
                    }),
                )
            })
            .collect();
        // Ascending, because a map's order is not an order and an agent that
        // reads this twice must see the same list twice.
        panes.sort_by_key(|(term, _)| *term);
        panes
            .into_iter()
            .map(|(_, row)| row)
            .collect::<Vec<serde_json::Value>>()
    };
    Ok(json!({
        "surface": "zerocode-built-in",
        "hosts": hosts,
        "panes": panes,
    }))
}

/// Stand a terminal on the asked-for machine and hand back the pane it became.
///
/// The connection is made HERE rather than in the frontend, because that is
/// what makes `open` able to answer with a pane id at all — the window is then
/// only asked to mount a shell that already exists, which is the same order
/// the emulator door uses and the reason neither leaves an empty tab standing.
pub(super) async fn ssh_agent_open(
    app: &AppHandle,
    command: &zerocode_core::computer_use::SshCommand,
) -> Result<serde_json::Value, String> {
    let (rows, cols) = command.grid();
    let (term, label, remote) = if command.local {
        let cwd = command.cwd.clone();
        let term = {
            let state = app.state::<AppState>();
            // PLAIN, deliberately: the configured startup is how a person's new
            // terminal becomes an agent, and an agent that opened one would be
            // launching a second agent instead of getting the shell it asked
            // for. `--cwd` is still fenced to the workspace by `open_term_tab`.
            open_term_tab(state, rows, cols, Some(true), cwd, None)?
        };
        (term, None, None)
    } else {
        let id = command.host.clone().expect("parser requires --host");
        // Which store owns this name decides which road opens it, and an
        // unknown name is answered with the list rather than a dial attempt.
        let (repository, service) = {
            let state = app.state::<AppState>();
            (Arc::clone(state.settings()), state.ssh_hosts().clone())
        };
        let found = run_remote_host_task({
            let id = id.clone();
            move || {
                let host = ssh_hosts_report(&repository, &service)?
                    .hosts
                    .into_iter()
                    .find(|host| host.id == id)
                    .map(|host| host.label);
                let target = ssh_targets_list(&repository)
                    .into_iter()
                    .find(|target| target.id == id)
                    .map(|target| target.label);
                Ok((host, target))
            }
        })
        .await?;
        match found {
            (Some(label), _) => {
                let term = open_ssh_terminal(app.clone(), id.clone(), rows, cols).await?;
                (term, Some(label), Some(id))
            }
            (None, Some(label)) => {
                let term = {
                    let state = app.state::<AppState>();
                    ssh_open_remote_term(state, id.clone(), rows, cols).await?
                };
                (term, Some(label), Some(id))
            }
            (None, None) => {
                return Err(format!(
                    "`{id}` is not a saved SSH host or remote workspace; run `zerocode-ssh list --json` for the names this window can open"
                ));
            }
        }
    };
    // The tab is the window's to place, the same as the emulator's. Revalidated
    // on the other side: this payload is data, never an instruction.
    let seat = json!({ "term": term, "label": label, "remoteHost": remote });
    app.emit_to("main", "ssh:agent-open", seat)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "surface": "zerocode-built-in",
        "pane": term,
        "rows": rows,
        "cols": cols,
        "label": label,
        "host": remote,
    }))
}

/// How long this door waits for a pane to take a line and answer for it.
///
/// The delivery machine has its own readiness budget; this is the outer fence
/// so a wedged pane cannot hold a caller's verb open forever. One second past
/// the readiness timeout and its submission window, which is the longest the
/// two of them can honestly take.
const SSH_SEND_PATIENCE: Duration = Duration::from_secs(1);

/// What one `zerocode-ssh send` actually achieved.
///
/// Three facts, not one. They used to be a single `submitted` copied straight
/// off the `--enter` FLAG, which is why a caller was told `submitted: true`
/// over a composer that was holding an unsent draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SendReceipt {
    /// The words reached the pane's input.
    pasted: bool,
    /// The submitting Enter was written after them.
    enter_sent: bool,
    /// The agent itself reported taking the prompt. `None` where the provider
    /// has no way to say so — an honest unknown, never a `true`.
    acknowledged: Option<bool>,
    /// Why the Enter was withheld, when the words went in and it did not.
    withheld: Option<zerocode_pty::ready::Refusal>,
}

/// Put a line on a pane's input the way the window's own prompt road does.
///
/// # What was wrong
///
/// This wrote `text` and its carriage return in ONE call and then answered
/// `submitted: command.enter` — a claim taken from the caller's flag rather
/// than from the pane. Both halves were wrong at once. Glued to its text, a
/// return is the last byte of a paste, and an agent TUI still assembling the
/// line either swallows it or takes it as a newline in a draft; and even when
/// it did submit, nothing had asked. A person watching found their composer
/// holding words the caller had already been told were sent.
///
/// # What it does now
///
/// It uses the machine every other prompt in this window goes through
/// ([`PromptDelivery`]): wait until the composer is actually listening, send
/// the words inside the pane's own bracketed-paste envelope, and let the
/// submitting Enter follow after the gap. Then it reports what happened —
/// pasted, enter sent, and, for providers that can say so, acknowledged.
///
/// # What it refuses
///
/// A pane holding a question is not typed into: an approval or an ask belongs
/// to the person it was put to, and a line arriving there answers it. A line
/// holding a person's unsent words is not pasted onto, and an Enter is not
/// written after a hand reached the line while the paste settled — both
/// decided by the delivery at the write ([`zerocode_pty::ready::Guard`]),
/// not by this door before it. And no composer is ever CLEARED — clear keys
/// erase a draft somebody is writing, and this door has no way to know whose
/// words are already on the line.
pub(super) async fn ssh_agent_send(
    app: &AppHandle,
    command: &zerocode_core::computer_use::SshCommand,
) -> Result<serde_json::Value, String> {
    let pane = command.pane.expect("parser requires --pane");
    let text = command.typed_text();
    let submits = command.submits();

    /* One send at a time per pane, for the whole transaction.
     *
     * The words and the Enter are ONE delivery now, so the pane's prompt
     * queue orders them against every other producer; what the queue does
     * not order is the receipt around them — the submit slot claimed before
     * and the acknowledgement waited on after — and two of these calls racing
     * would read each other's. The lease is held across the whole call. */
    let _transaction = crate::ssh_send_guard::send_lease(pane).lock_owned().await;

    /* A pane with no agent in it is a SHELL, and a shell has no composer.
     *
     * The delivery machine waits for a program to announce that it reads keys
     * (`?2004h`) before it writes anything, which is right for a TUI and
     * wrong for `sh`: a line and its return are simply how a person runs a
     * command there, there is no draft to protect, and there is nothing that
     * will ever acknowledge one. Routed through that wait, a working
     * `zerocode-ssh send --text ls --enter` would become an eight-second
     * timeout on any shell that does not happen to speak bracketed paste.
     *
     * So the shell keeps the keystroke road it always had — and keeps it
     * honestly. */
    let agent = {
        let state = app.state::<AppState>();
        ssh_pane_may_be_typed_at(&state, pane)?;
        state.agent_terms().get(&pane).copied()
    };
    if agent.is_none() {
        return ssh_send_keystrokes(app, pane, &text, submits);
    }

    /* One delivery, guarded at the write.
     *
     * Not clearing a draft is not preserving it: a paste lands on whatever is
     * already there and the Enter submits both, so a person's half-written
     * thought would go to the agent with our words stuck to the end of it.
     * The window knows when a hand reached this pane and when a prompt was
     * reported going in ([`crate::human_input`]); between those two it does
     * not know what is on the line, and a door that does not know does not
     * write.
     *
     * This door used to read those facts HERE, before registering the words
     * and then the Enter as two separate jobs — and the pump acted on its
     * answers a readiness wait later, about a line that had since moved, with
     * room between the two jobs for another producer's prompt (integration
     * review 2026-09-05, item 2). So the facts are now read by the delivery
     * itself at each write ([`zerocode_pty::ready::Guard`]): the paste is
     * refused onto a draft, a parked question or a relaunched pane, and the
     * Enter is withheld when a hand reaches the line while the words are
     * landing. One delivery is the whole transaction, and the pane's prompt
     * queue orders it against every other producer. */
    let submitted = {
        let state = app.state::<AppState>();
        // Typing scrolls back to the live edge, the way it does for a person —
        // otherwise the answer lands somewhere the next `read` cannot see.
        if let Some(held) = state.terminals().handle(pane) {
            lock_pty(&held).terminal_mut().grid_mut().view_to_bottom();
        }
        /* The acknowledgement channel, armed only where the provider has a
         * way to report a prompt it took. Its absence is what makes
         * `acknowledged` a `None` rather than a guess — and the launch road
         * owns this map during a worker's birth, so a pane already mid-launch
         * keeps its own slot and this send simply reports one fact fewer.
         * Claimed BEFORE the delivery is registered, so the provider's report
         * cannot slip in between the Enter and the listener for it. */
        submits
            .then(|| {
                agent
                    .and_then(AgentKind::from_slug)
                    .filter(|kind| kind.reports_prompt_submit())
                    .and_then(|_| {
                        crate::ssh_send_guard::claim_submit_receipt(
                            &mut state.worker_prompt_submits(),
                            pane,
                        )
                    })
            })
            .flatten()
    };
    let owns_submit_slot = submitted.is_some();
    let release_slot = |app: &AppHandle| {
        if owns_submit_slot {
            /* Only this call's own slot. Removing it unconditionally destroyed
             * the launch receipt it was written to protect: a worker mid-birth
             * owns this seat, and a send that did not install it has nothing
             * here to take back. */
            app.state::<AppState>()
                .worker_prompt_submits()
                .remove(&pane);
        }
    };
    let waiting = {
        let state = app.state::<AppState>();
        match ssh_register_delivery(&state, pane, agent, text, submits) {
            Ok(waiting) => waiting,
            Err(refusal) => {
                release_slot(app);
                return Err(refusal);
            }
        }
    };
    let outcome = ssh_await_delivery(waiting).await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(why) => {
            release_slot(app);
            return Err(why);
        }
    };
    let receipt = match outcome {
        DeliveryOutcome::Delivered => SendReceipt {
            pasted: true,
            enter_sent: submits,
            acknowledged: None,
            withheld: None,
        },
        DeliveryOutcome::Unsubmitted(why) => SendReceipt {
            pasted: true,
            enter_sent: false,
            acknowledged: None,
            withheld: Some(why),
        },
        DeliveryOutcome::Refused(why) => {
            release_slot(app);
            return Err(format!(
                "pane {pane}: {}; nothing was typed and nothing was submitted",
                why.says()
            ));
        }
        DeliveryOutcome::TimedOut => {
            release_slot(app);
            return Err(format!(
                "pane {pane} never showed a composer ready to take a line, so nothing \
                 was typed and nothing was submitted"
            ));
        }
    };
    let acknowledged = match submitted {
        None => None,
        Some(_) if !receipt.enter_sent => Some(false),
        Some(submitted) => Some(
            tokio::task::spawn_blocking(move || {
                submitted
                    .recv_timeout(zerocode_pty::ready::SUBMIT_ACK_TIMEOUT)
                    .is_ok()
            })
            .await
            .map_err(|error| error.to_string())?,
        ),
    };
    release_slot(app);
    Ok(ssh_send_receipt(
        pane,
        SendReceipt {
            acknowledged,
            ..receipt
        },
    ))
}

/// The keystroke road, for a pane holding no agent.
///
/// What this door always did, minus the one thing that was wrong with it: the
/// return is written separately from the words, and the answer says which of
/// the two actually happened rather than repeating the caller's flag.
/// `acknowledged` is null because a shell has no way to say.
fn ssh_send_keystrokes(
    app: &AppHandle,
    pane: TermId,
    text: &str,
    submits: bool,
) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();
    let held = state
        .terminals()
        .handle(pane)
        .ok_or_else(|| ssh_no_such_pane(pane))?;
    let mut pty = lock_pty(&held);
    pty.terminal_mut().grid_mut().view_to_bottom();
    pty.write_input(text.as_bytes())
        .map_err(|error| error.to_string())?;
    let enter_sent = submits && pty.write_input(b"\r").is_ok();
    drop(pty);
    state.cadence().wake();
    if submits && !enter_sent {
        return Err(format!(
            "pane {pane} took the line but its return could not be written, so \
             nothing was submitted"
        ));
    }
    Ok(ssh_send_receipt(
        pane,
        SendReceipt {
            pasted: true,
            enter_sent,
            acknowledged: None,
            withheld: None,
        },
    ))
}

/// Whether this pane is one this door may put a line into, right now.
///
/// Asked before the paste AND again before the Enter, because the answer can
/// change in between and the second one is the one that submits.
fn ssh_pane_may_be_typed_at(state: &AppState, pane: TermId) -> Result<(), String> {
    if !state.terminals().contains_key(&pane) {
        return Err(ssh_no_such_pane(pane));
    }
    if state
        .pane_states()
        .get(&pane)
        .is_some_and(|held| held.state == zerocode_core::hook::HookState::NeedsAttention)
    {
        return Err(format!(
            "pane {pane} is holding a question or an approval; answering it \
             belongs to the person it was put to"
        ));
    }
    Ok(())
}

/// Register one delivery for this door and hand back its completion.
///
/// The window's one typed-prompt door does the work
/// ([`crate::cmd::terminal::type_prompt_at_term`]): readiness, the pane's own
/// bracketed envelope, the submit gap and the queue behind a prompt already on
/// the line are all its, and a second copy of any of that here is a second
/// answer waiting to disagree with the first.
///
/// What this door chooses is the one thing it knows and that door cannot:
/// nothing on the line is ours. Clear keys erase whatever is in front of the
/// composer, and the words there may be half a thought a person is still
/// writing — so the readiness says so, and no edit key is ever sent.
fn ssh_register_delivery(
    state: &AppState,
    pane: TermId,
    agent: Option<&'static str>,
    text: String,
    submit: bool,
) -> Result<std::sync::mpsc::Receiver<DeliveryOutcome>, String> {
    crate::cmd::terminal::type_prompt_at_term(
        state,
        pane,
        text,
        submit,
        agent,
        crate::cmd::terminal::PromptReadiness::RestingBesideADraft,
    )
}

/// Wait for one registered delivery, off the async runtime's own threads.
///
/// The outcome itself, not a flag: what the delivery actually did is the
/// receipt, and a `bool` would fold "pasted and left unsubmitted" into one of
/// the two answers it is neither of. A receiver that goes quiet — the pane
/// died under the delivery and its waiter with it — reads as a timeout, which
/// is what the pump reports for a dead shell as well.
async fn ssh_await_delivery(
    waiting: std::sync::mpsc::Receiver<DeliveryOutcome>,
) -> Result<DeliveryOutcome, String> {
    let deadline = zerocode_pty::ready::TIMEOUT + SSH_SEND_PATIENCE;
    tokio::task::spawn_blocking(move || {
        waiting
            .recv_timeout(deadline)
            .unwrap_or(DeliveryOutcome::TimedOut)
    })
    .await
    .map_err(|error| error.to_string())
}

/// The three facts, said separately — and the fourth, when a write was
/// withheld: why, in the guard's own words, so a caller reading
/// `submitted: false` beside `pasted: true` knows the line is sitting in the
/// composer as a draft and what put it there.
fn ssh_send_receipt(pane: TermId, receipt: SendReceipt) -> serde_json::Value {
    json!({
        "surface": "zerocode-built-in",
        "pane": pane,
        "sent": receipt.pasted,
        "pasted": receipt.pasted,
        // The Enter this window WROTE. Whether the agent took it is the next
        // field, and where the provider cannot say, that field is null rather
        // than a comfortable true.
        "submitted": receipt.enter_sent,
        "acknowledged": receipt.acknowledged,
        "withheld": receipt.withheld.map(zerocode_pty::ready::Refusal::says),
    })
}

/// The pane's screen as text — the same grid the person is looking at.
pub(super) fn ssh_agent_read(
    app: &AppHandle,
    command: &zerocode_core::computer_use::SshCommand,
) -> Result<serde_json::Value, String> {
    let pane = command.pane.expect("parser requires --pane");
    let state = app.state::<AppState>();
    let held = state
        .terminals()
        .handle(pane)
        .ok_or_else(|| ssh_no_such_pane(pane))?;
    let pty = lock_pty(&held);
    let grid = pty.terminal().grid();
    Ok(json!({
        "surface": "zerocode-built-in",
        "pane": pane,
        "rows": grid.rows(),
        "cols": grid.cols(),
        "screen": grid.visible_text(),
    }))
}

pub(super) fn ssh_no_such_pane(pane: TermId) -> String {
    format!(
        "pane {pane} is not open in this window; run `zerocode-ssh list --json` for the panes it holds"
    )
}

pub(super) fn ssh_cli_error(json: bool, code: &str, message: &str) -> zerocode_hookd::TeamAnswer {
    if json {
        computer_refused(
            json!({ "ok": false, "error": { "code": code, "message": message } }).to_string(),
        )
    } else {
        computer_refused(format!("zerocode-ssh: {message}"))
    }
}

pub(super) fn ssh_pretty(
    method: zerocode_core::computer_use::SshMethod,
    value: &serde_json::Value,
) -> String {
    use zerocode_core::computer_use::SshMethod;

    match method {
        // The screen is the answer; wrapping it in JSON to be read by a human
        // would put escape sequences where the lines are.
        SshMethod::Read => value
            .get("screen")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        SshMethod::List => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        SshMethod::Open => value
            .get("pane")
            .and_then(serde_json::Value::as_u64)
            .map_or_else(
                || "Opened.".to_string(),
                |pane| format!("Opened pane {pane}."),
            ),
        SshMethod::Send => "Done.".into(),
    }
}

pub(super) fn emulator_cli_error(
    json: bool,
    code: &str,
    message: &str,
) -> zerocode_hookd::TeamAnswer {
    if json {
        computer_refused(
            json!({ "ok": false, "error": { "code": code, "message": message } }).to_string(),
        )
    } else {
        computer_refused(format!("zerocode-emulator: {message}"))
    }
}

pub(super) fn emulator_pretty(
    method: zerocode_core::computer_use::EmulatorMethod,
    value: &serde_json::Value,
) -> String {
    use zerocode_core::computer_use::EmulatorMethod;
    match method {
        EmulatorMethod::Marks | EmulatorMethod::Click => {
            use zerocode_core::computer_use_protocol::marks::{LEGEND_KEY, LOOK_ID_KEY};
            format!(
                "{LOOK_ID_KEY}: {}\n{}",
                value[LOOK_ID_KEY].as_str().unwrap_or_default(),
                value[LEGEND_KEY].as_str().unwrap_or_default()
            )
        }
        EmulatorMethod::Open => "Opening in ZeroCode's built-in emulator pane…".into(),
        EmulatorMethod::Tap
        | EmulatorMethod::Swipe
        | EmulatorMethod::Text
        | EmulatorMethod::Button
        | EmulatorMethod::Rotate => "Done.".into(),
        EmulatorMethod::Screenshot => value
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || "Screenshot saved.".to_string(),
                |path| format!("Screenshot: {path}"),
            ),
        EmulatorMethod::List
        | EmulatorMethod::Tree
        | EmulatorMethod::Find
        | EmulatorMethod::Foreground => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
    }
}

pub(super) fn answer_computer_command(
    argv: &[String],
    permission_window: Option<&tauri::AppHandle>,
    asking: computer_use::confirm::Asking,
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use::{ComputerMethod, parse_command, usage};

    if argv
        .first()
        .is_some_and(|word| matches!(word.as_str(), "help" | "-h" | "--help"))
    {
        return computer_said(format!("{}\n", usage()));
    }
    let wants_json = argv.iter().any(|word| word == "--json");
    let command = match parse_command(argv) {
        Ok(command) => command,
        Err(error) => return computer_cli_error(wants_json, "invalid_argument", &error),
    };
    if let Some(refusal) = refused_at_the_door(&command) {
        return refusal;
    }
    let result = if command.method == ComputerMethod::Stop {
        Ok(computer_use::guard::stop(
            zerocode_core::computer_use::STOP_REASON_REQUEST,
        ))
    } else if let Some(reason) = (command.method == ComputerMethod::Resume)
        .then(persons_stop_standing)
        .flatten()
    {
        // The person's stop is theirs: the window's resume lifts it, never
        // an agent's `resume`.
        Err(computer_use::guard::refusal(&reason))
    } else if command.method == ComputerMethod::Resume {
        computer_use::guard::lift();
        let reset = command
            .params
            .get("resetBudget")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        if computer_use::session_stands() {
            computer_use::call("resume", serde_json::json!({ "resetBudget": reset }))
                .map(|helper| serde_json::json!({ "resumed": true, "helper": helper }))
        } else {
            Ok(serde_json::json!({ "resumed": true, "helper": "idle" }))
        }
    } else if command.method == ComputerMethod::Status {
        let helper = if computer_use::session_stands() {
            computer_use::call("status", serde_json::json!({}))
                .unwrap_or_else(|error| serde_json::json!({ "error": error.code }))
        } else {
            serde_json::Value::String("idle".into())
        };
        adopt_helper_stop(&helper);
        // The pace is the window's table, answered whether or not a helper
        // session stands — a caller spacing its actions needs it before the
        // first one starts a session. Unlimited says so (`mode`), with no
        // stand-in rate.
        Ok(serde_json::json!({
            "stopped": computer_use::guard::stopped_reason(),
            "hotkey": zerocode_core::computer_use::COMPUTER_STOP_HOTKEY,
            "pace": zerocode_core::computer_use::pace_table(
                zerocode_core::computer_use::COMPUTER_PACE
            ),
            "helper": helper,
            "state": computer_use::state::snapshot(),
            "confirming": computer_use::confirm::open_questions(),
        }))
    } else if matches!(
        command.method,
        ComputerMethod::RecipeSave | ComputerMethod::RecipeList | ComputerMethod::RecipeShow
    ) {
        desktop_recipe(&command)
    } else if command.method == ComputerMethod::Observe {
        computer_use::observe::observe(&command.params.as_object().cloned().unwrap_or_default())
    } else if command.method == ComputerMethod::Handoff {
        use computer_use::confirm::Decision;
        use zerocode_core::computer_use_protocol::error_code;
        let reason = command
            .params
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let timeout = command
            .params
            .get("timeoutMs")
            .and_then(serde_json::Value::as_u64);
        let timeout = zerocode_core::computer_use::handoff_ms(timeout);
        match computer_use::confirm::handoff(reason, timeout) {
            Decision::Allowed => Ok(serde_json::json!({ "resumed": true, "reason": reason })),
            Decision::Refused => Err(computer_use::ComputerUseError::new(
                error_code::CONFIRMATION_REFUSED,
                format!("the person cancelled the handoff ({reason})"),
            )),
            Decision::TimedOut => Err(computer_use::ComputerUseError::new(
                error_code::CONFIRMATION_TIMEOUT,
                format!("nobody took over within {timeout} ms ({reason})"),
            )),
        }
    } else if matches!(
        command.method,
        ComputerMethod::ReflexStart
            | ComputerMethod::ReflexStatus
            | ComputerMethod::ReflexStop
            | ComputerMethod::Capabilities
    ) {
        // A live reflex run is the window's to admit and to watch, and
        // whether one is supported here and enabled is the window's to say
        // beside the helper's handshake; the person's setting is read only
        // when a start, a status or the capabilities ask.
        computer_use::reflex::answer(&command, || {
            permission_window.is_some_and(|app| {
                crate::settings_runtime::computer_live_reflex(app.state::<AppState>().settings())
            })
        })
    } else if command.method == ComputerMethod::Compare {
        desktop_compare(&command.params)
    } else if matches!(
        command.method,
        ComputerMethod::Batch | ComputerMethod::RecipeRun
    ) {
        // A walk is the loop's to run, step by step down this road.
        Err(computer_use::ComputerUseError::new(
            zerocode_core::computer_use_protocol::error_code::INVALID_ARGUMENT,
            format!(
                "a {} is the loop's to walk — ask it through the pane's {}",
                command.method.verb_name(),
                zerocode_core::computer_use::COMPUTER_CLI
            ),
        ))
    } else if command.method == ComputerMethod::Verdict {
        // The loop that knows the run's folder answers a verdict; reaching
        // here means it was asked from nowhere a folder could be.
        Err(computer_use::ComputerUseError::new(
            zerocode_core::computer_use_protocol::error_code::INVALID_ARGUMENT,
            "a verdict needs an evidence folder — ask it from the pane of a run or of a session",
        ))
    } else if command.method == ComputerMethod::Evidence {
        let dir = computer_use::evidence::session_dir(now_epoch_ms());
        Ok(evidence_report(&command, dir.as_deref()))
    } else if command.method == ComputerMethod::Permissions {
        let id = command
            .params
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::parse::<zerocode_core::computer_use::ComputerPermissionId>)
            .transpose();
        match id {
            Ok(id) => computer_use::setup_permission(
                id,
                command
                    .params
                    .get("reset")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            )
            .and_then(|report| {
                serde_json::to_value(report).map_err(|error| computer_use::ComputerUseError {
                    code: "accessibility_error".into(),
                    message: error.to_string(),
                })
            }),
            Err(error) => Err(computer_use::ComputerUseError {
                code: "invalid_argument".into(),
                message: error,
            }),
        }
    } else if command.method == ComputerMethod::WaitFor {
        // A wait-for is the window's: it looks through the helper until what
        // it waits for is there (or gone), bounded by the table.
        desktop_wait_for(&command.params)
    } else if command.method == ComputerMethod::SoundWait {
        // So is a sound-wait: the window asks the helper what it heard.
        computer_use::listen::sound_wait(&command.params, |after| {
            computer_use::call("soundRead", serde_json::json!({ "after": after }))
        })
    } else if command.method == ComputerMethod::Watch {
        // And a watch: the window reads the eye's repaints, no pictures.
        computer_use::eye::watch_desktop(&command.params)
    } else if let Some(seen) = (command.method == ComputerMethod::Screenshot)
        .then(|| computer_use::eye::screenshot(&command.params))
        .flatten()
    {
        // A desktop screenshot is the eye's newest frame when the eye is open.
        Ok(seen)
    } else if command.method == ComputerMethod::Read {
        // A read's text is capped by the table here, in the one place every
        // helper's answer passes. A desktop read keeps the eye open, so the
        // helper reads again only what repainted since it last read, and
        // leaves out what ZeroCode's own windows show.
        computer_use::call("readText", computer_use::eye::reading(&command.params)).map(
            |mut answer| {
                let (kept, cut) = zerocode_core::computer_use::desktop_read_text(
                    answer
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                );
                answer["text"] = serde_json::Value::String(kept);
                answer["truncated"] = serde_json::Value::Bool(cut);
                answer
            },
        )
    } else if command.method == ComputerMethod::Run {
        // A run is the window's to answer too: a program, no pane, its head of
        // output as the result — the terminal a person would have opened.
        Ok(desktop_run(&command.params))
    } else if command.method == ComputerMethod::Wait {
        // A wait is the window's to answer: a bounded pause, no helper round trip.
        let asked = command
            .params
            .get("ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let (waited, capped) = zerocode_core::computer_use::desktop_wait_ms(asked);
        std::thread::sleep(std::time::Duration::from_millis(waited));
        Ok(serde_json::json!({ "waitedMs": waited, "capped": capped }))
    } else if command.method == ComputerMethod::Click && command.params.get("mark").is_some() {
        click_by_mark(&command, asking)
    } else if command.method == ComputerMethod::Find {
        // A desktop find reads the way a desktop read does.
        let reading = zerocode_core::computer_use::ComputerCommand {
            params: computer_use::eye::reading(&command.params),
            ..command.clone()
        };
        call_with_the_persons_last_step(&reading, asking, &mut computer_use::call)
    } else {
        call_with_the_persons_last_step(&command, asking, &mut computer_use::call)
    };
    match result {
        Ok(mut result) => {
            let png = computer_use::export_screenshot(&mut result);
            if command.method == ComputerMethod::Screenshot
                && let Some(png) = png
            {
                computer_use::observe::remember(
                    &command.params.as_object().cloned().unwrap_or_default(),
                    &result,
                    png,
                );
            }
            said_envelope(&command, result)
        }
        Err(error) => {
            use zerocode_core::computer_use_protocol::{error_code, validate};
            if error.code == error_code::PERMISSION_DENIED
                && let Some(window) = permission_window
            {
                let _ = window.emit("computer:permission-denied", serde_json::json!({}));
            }
            // A stop the helper made on its own (the person's chord): the
            // window takes it in, so its band shows it and the resume.
            if error.code == error_code::STOPPED
                && computer_use::guard::stopped_reason().is_none()
                && computer_use::session_stands()
                && let Ok(helper) = computer_use::call("status", serde_json::json!({}))
            {
                adopt_helper_stop(&helper);
            }
            // A password field refused the keys, or a line break would have
            // pressed: the helper's terse words as the model reads them. A
            // stop is said in the window's one sentence for its reason —
            // whether `resume` may lift it.
            let message = match computer_use::guard::stopped_reason() {
                Some(reason) if error.code == error_code::STOPPED => {
                    computer_use::guard::refusal(&reason).message
                }
                _ => {
                    validate::refusal_sentence(&error.code, &error.message).unwrap_or(error.message)
                }
            };
            computer_cli_error(command.json, &error.code, &message)
        }
    }
}

/// `click --mark N --look L` (§7.1): the mark's own element click on the
/// look's window, pinned by what the look saw, down the same road as every
/// press — the person's last step, the stop, the evidence — and answered
/// with what it pressed.
fn click_by_mark(
    command: &zerocode_core::computer_use::ComputerCommand,
    asking: computer_use::confirm::Asking,
) -> Result<serde_json::Value, computer_use::ComputerUseError> {
    let mark = computer_use::marks::pinned_click(&command.params)?;
    let resolved = zerocode_core::computer_use::ComputerCommand {
        method: command.method,
        params: serde_json::Value::Object(mark.params.clone()),
        json: command.json,
    };
    call_with_the_persons_last_step(&resolved, asking, &mut computer_use::call)
        .map(|answer| computer_use::marks::click_answer(answer, &mark))
        .map_err(|error| computer_use::marks::click_refusal(error, &mark))
}

/// Call the helper for a command, holding a press that lands on a payment,
/// transfer or delete control for the person (§1.5): the policy's words ride
/// with a pressing request, the helper answers `confirmation_required` with
/// the kind and the label, the window asks, and only an allowed press is
/// sent again with the receipt. A caller that already knows where it is
/// says `--confirming <kind>` and is asked before the helper is called.
pub(super) fn call_with_the_persons_last_step(
    command: &zerocode_core::computer_use::ComputerCommand,
    asking: computer_use::confirm::Asking,
    call: &mut dyn FnMut(
        &str,
        serde_json::Value,
    ) -> Result<serde_json::Value, computer_use::ComputerUseError>,
) -> Result<serde_json::Value, computer_use::ComputerUseError> {
    use computer_use::confirm::{self, Asking, Decision};
    use zerocode_core::computer_use::{ConfirmKind, parse_confirmation_required};
    use zerocode_core::computer_use_protocol::error_code;

    let method = command.method.provider_name().expect("provider method");
    let verb = command.method;
    let mut params = command.params.clone();
    let policy = confirm::policy();
    let refusal = |decision: Decision, kind: ConfirmKind, label: &str| {
        let (code, why) = match decision {
            Decision::TimedOut => (error_code::CONFIRMATION_TIMEOUT, "nobody answered in time"),
            Decision::Refused | Decision::Allowed => {
                (error_code::CONFIRMATION_REFUSED, "the person said no")
            }
        };
        computer_use::ComputerUseError::new(
            code,
            format!(
                "the {} step '{label}' was not confirmed: {why}",
                kind.as_str()
            ),
        )
    };
    if verb.presses() {
        if let Some(declared) = params
            .get("confirming")
            .and_then(serde_json::Value::as_str)
            .and_then(ConfirmKind::parse)
            .filter(|kind| policy.asks(*kind))
        {
            let label = params
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(declared)")
                .to_string();
            if asking == Asking::HandBack {
                // Handed back unpressed, in the helper's own words, so the
                // caller reads it as the press it found (parse_confirmation_required).
                return Err(computer_use::ComputerUseError::new(
                    error_code::CONFIRMATION_REQUIRED,
                    format!("{}: {label}", declared.as_str()),
                ));
            }
            match confirm::ask(declared, &label, verb.verb_name()) {
                Decision::Allowed => {}
                decision => return Err(refusal(decision, declared, &label)),
            }
            params["confirmed"] = serde_json::Value::Bool(true);
        } else if let Some(words) = policy.guard_words() {
            params["confirmGuard"] = words;
        }
    }
    if verb.types_keys() {
        params[zerocode_core::computer_use::KEYBOARD_GUARD_PARAM] =
            zerocode_core::computer_use::keyboard_guard();
    }
    match call(method, params.clone()) {
        Err(error)
            if error.code == error_code::CONFIRMATION_REQUIRED && asking == Asking::Person =>
        {
            let (kind, label) = parse_confirmation_required(&error.message).ok_or(error)?;
            match confirm::ask(kind, &label, verb.verb_name()) {
                Decision::Allowed => {
                    params["confirmed"] = serde_json::Value::Bool(true);
                    if let Some(object) = params.as_object_mut() {
                        object.remove("confirmGuard");
                    }
                    call(method, params)
                }
                decision => Err(refusal(decision, kind, &label)),
            }
        }
        other => other,
    }
}

/// Recipes (§7.2): saved from this session's steps (or a folder named), kept
/// under the window's data root, and registered in the artifacts catalogue
/// so a person finds and edits them where every other document is.
pub(super) fn desktop_recipe(
    command: &zerocode_core::computer_use::ComputerCommand,
) -> Result<serde_json::Value, computer_use::ComputerUseError> {
    use zerocode_core::computer_use::ComputerMethod;
    use zerocode_core::computer_use_protocol::error_code;
    let Some(root) = computer_use::evidence::root() else {
        return Err(computer_use::ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            RECIPES_ROOT_UNKNOWN,
        ));
    };
    let name = command
        .params
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match command.method {
        ComputerMethod::RecipeSave => {
            let from = command
                .params
                .get("from")
                .and_then(serde_json::Value::as_str)
                .map(std::path::PathBuf::from)
                .or_else(|| computer_use::evidence::session_dir(now_epoch_ms()))
                .ok_or_else(|| {
                    computer_use::ComputerUseError::new(
                        error_code::INVALID_ARGUMENT,
                        "no session evidence yet and no --from folder",
                    )
                })?;
            let last = command
                .params
                .get("last")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| usize::try_from(n).ok());
            let note = command
                .params
                .get("note")
                .and_then(serde_json::Value::as_str);
            let file = computer_use::recipes::save(&root, name, note, &from, last, now_epoch_ms())?;
            // Into the catalogue, so the gallery shows it beside the reports.
            let artifact = crate::artifact_runtime::store().and_then(|store| {
                store
                    .register_copy(
                        &file,
                        zerocode_core::artifact::Source::Manual,
                        zerocode_core::artifact::Origin {
                            agent: Some("computer-use".into()),
                            ..zerocode_core::artifact::Origin::default()
                        },
                        now_epoch_ms(),
                    )
                    .ok()
            });
            Ok(serde_json::json!({
                "name": name,
                "file": file.display().to_string(),
                "from": from.display().to_string(),
                "artifactId": artifact.map(|one| one.id),
            }))
        }
        ComputerMethod::RecipeList => {
            Ok(serde_json::json!({ "recipes": computer_use::recipes::list(&root) }))
        }
        ComputerMethod::RecipeShow => {
            let (file, text) = computer_use::recipes::show(&root, name)?;
            // What a walk will ask for, read the way recipe-run reads it; a
            // document a person broke still shows, with why it will not walk.
            let (steps, params, unreadable) =
                match zerocode_core::computer_recipe::recipe_lines(&text) {
                    Ok(lines) => (
                        lines.len(),
                        zerocode_core::computer_recipe::params_of(&lines),
                        None,
                    ),
                    Err(why) => (0, Vec::new(), Some(why)),
                };
            Ok(serde_json::json!({
                "name": name,
                "file": file.display().to_string(),
                "steps": steps,
                "params": params,
                "unreadable": unreadable,
                "text": text,
            }))
        }
        _ => unreachable!("only the recipe verbs come here"),
    }
}

/// The window's way of asking (§1.5): the page shows the question, the
/// person presses, the command answers by id; the request waits here. The
/// person's turn (§7.4) goes the same road with its own card.
pub(super) fn install_confirm_asker(app: AppHandle) {
    use computer_use::confirm;
    let handoff_app = app.clone();
    confirm::install_handoff_asker(Box::new(move |handoff| {
        let receiver = confirm::open(&handoff.id);
        let _ = handoff_app.emit("computer:handoff", handoff);
        let decision = confirm::wait(
            &receiver,
            std::time::Duration::from_millis(handoff.timeout_ms),
        );
        confirm::close(&handoff.id);
        let _ = handoff_app.emit(
            "computer:handoff-closed",
            serde_json::json!({ "id": handoff.id, "decision": decision }),
        );
        decision
    }));
    confirm::install_asker(Box::new(move |ask| {
        let receiver = confirm::open(&ask.id);
        let _ = app.emit("computer:confirm", ask);
        let decision = confirm::wait(&receiver, std::time::Duration::from_millis(ask.timeout_ms));
        confirm::close(&ask.id);
        let _ = app.emit(
            "computer:confirm-closed",
            serde_json::json!({ "id": ask.id, "decision": decision }),
        );
        decision
    }));
}

/// Watch for an element, a piece of screen text or a window title until it is
/// there — or, with `absent`, gone — looking every `COMPUTER_WAIT_FOR_POLL_MS`
/// through the helper, for at most the table's budget. An app or window that
/// is not there yet is a reason to look again; any other refusal is the answer.
pub(super) fn desktop_wait_for(
    params: &serde_json::Value,
) -> Result<serde_json::Value, computer_use::ComputerUseError> {
    use zerocode_core::computer_use::{
        COMPUTER_WAIT_FOR_POLL_MS, desktop_wait_for_ms, wait_for_retries, wait_for_settled,
        window_title_matches,
    };
    let absent = params.get("absent").and_then(serde_json::Value::as_bool) == Some(true);
    let wanted_title = params
        .get("window")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let (budget_ms, capped) =
        desktop_wait_for_ms(params.get("timeoutMs").and_then(serde_json::Value::as_u64));
    let mut probe = params.clone();
    if let Some(object) = probe.as_object_mut() {
        for key in ["timeoutMs", "absent", "window"] {
            object.remove(key);
        }
    }
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(budget_ms);
    let poll = std::time::Duration::from_millis(COMPUTER_WAIT_FOR_POLL_MS);
    let mut looks = 0_u32;
    // An OCR wait reads the screen again only after the eye saw it change
    // where it reads; without the eye it reads every time.
    let mut helper = computer_use::call;
    let reads_pixels = wanted_title.is_none()
        && probe.get("ocr").and_then(serde_json::Value::as_bool) == Some(true)
        && probe.get("app").is_none();
    let mut gate = reads_pixels
        .then(|| computer_use::eye::Gate::new(&computer_use::eye::MEMORY, &probe, &mut helper))
        .flatten();
    let mut last_read = serde_json::Value::Array(Vec::new());
    // What the last desktop reading matched in ZeroCode's own windows and
    // left out: the agent's own words are not the app's answer.
    let mut excluded_regions = false;
    loop {
        looks += 1;
        let unchanged = gate.as_mut().is_some_and(|gate| !gate.read(&mut helper));
        let seen = if unchanged {
            Ok(last_read.clone())
        } else if let Some(title) = wanted_title.as_deref() {
            computer_use::call("listAllWindows", serde_json::json!({})).map(|answer| {
                let windows = answer
                    .get("windows")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|window| {
                        // ZeroCode's own window is never the one waited for:
                        // "Code" is in its title.
                        window.get("own").and_then(serde_json::Value::as_bool) != Some(true)
                            && window
                                .get("title")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|shown| window_title_matches(shown, title))
                    })
                    .collect::<Vec<_>>();
                serde_json::Value::Array(windows)
            })
        } else {
            computer_use::call("findElements", computer_use::eye::reading(&probe)).map(|answer| {
                excluded_regions = answer
                    .get(zerocode_core::computer_use_protocol::eye::EXCLUDED_REGIONS_KEY)
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|regions| !regions.is_empty());
                answer
                    .get("matches")
                    .cloned()
                    .unwrap_or_else(|| serde_json::Value::Array(Vec::new()))
            })
        };
        let matches = match seen {
            Ok(matches) => matches,
            Err(error) if wait_for_retries(&error.code) => serde_json::Value::Array(Vec::new()),
            Err(error) => return Err(error),
        };
        last_read.clone_from(&matches);
        let count = matches.as_array().map_or(0, Vec::len);
        if wait_for_settled(count, absent) {
            return Ok(serde_json::json!({
                "satisfied": true,
                "absent": absent,
                "looks": looks,
                "reads": gate.as_ref().map_or(looks, |gate| gate.reads),
                "elapsedMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "budgetMs": budget_ms,
                "budgetCapped": capped,
                "matches": matches,
            }));
        }
        if started.elapsed() + poll >= budget {
            return Err(computer_use::ComputerUseError {
                code: zerocode_core::computer_use_protocol::error_code::TIMEOUT.into(),
                message: format!(
                    "still {} after {} ms ({looks} looks, {count} matches{}{})",
                    if absent { "present" } else { "absent" },
                    started.elapsed().as_millis(),
                    gate.as_ref()
                        .map(|gate| format!(", {} reads", gate.reads))
                        .unwrap_or_default(),
                    if excluded_regions {
                        "; ZeroCode's own window regions were excluded — the app may be behind them"
                            .to_string()
                    } else {
                        String::new()
                    },
                ),
            });
        }
        std::thread::sleep(poll);
    }
}

/// QA's comparison (§4): the baseline file against another file or, by
/// default, the screen as it is now (the helper's full-resolution frame);
/// the diff picture is exported beside the screenshots.
pub(super) fn desktop_compare(
    params: &serde_json::Value,
) -> Result<serde_json::Value, computer_use::ComputerUseError> {
    use zerocode_core::computer_use_protocol::error_code;
    let read = |key: &str| -> Result<Option<Vec<u8>>, computer_use::ComputerUseError> {
        params
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(|path| {
                std::fs::read(path).map_err(|error| {
                    computer_use::ComputerUseError::new(
                        error_code::INVALID_ARGUMENT,
                        format!("could not read {path}: {error}"),
                    )
                })
            })
            .transpose()
    };
    let baseline = read("baseline")?.ok_or_else(|| {
        computer_use::ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            "missing required --baseline",
        )
    })?;
    let against = match read("against")? {
        Some(bytes) => bytes,
        None => {
            // The screen is taken the way the baseline was: a bounded
            // `screenshot` baseline against the bounded picture, a
            // full-resolution one against full resolution — the same pixels
            // for the same screen, never one resampled against the other.
            let mut ask = serde_json::Map::new();
            for key in ["display", "region"] {
                if let Some(value) = params.get(key) {
                    ask.insert(key.to_string(), value.clone());
                }
            }
            let shoot = |full_res: bool| {
                let mut ask = ask.clone();
                if full_res {
                    ask.insert("fullRes".into(), serde_json::Value::Bool(true));
                }
                computer_use::call("screenshotDesktop", serde_json::Value::Object(ask)).and_then(
                    |frame| {
                        computer_use::screenshot_png(&frame).ok_or_else(|| {
                            computer_use::ComputerUseError::new(
                                error_code::SCREENSHOT_FAILED,
                                "the helper answered without a frame",
                            )
                        })
                    },
                )
            };
            let bounded = shoot(false)?;
            let size = computer_use::compare::png_size;
            if size(&bounded).is_some() && size(&bounded) == size(&baseline) {
                bounded
            } else {
                shoot(true)?
            }
        }
    };
    let (mut answer, picture) = computer_use::compare::compare(
        &baseline,
        &against,
        params.get("maxDiff").and_then(serde_json::Value::as_f64),
    )?;
    // The diff picture takes the screenshot road out: a private file, named.
    use base64::Engine as _;
    let mut exported = serde_json::json!({
        "screenshot": { "data": base64::engine::general_purpose::STANDARD.encode(picture) }
    });
    computer_use::export_screenshot(&mut exported);
    answer["diffPath"] = exported
        .pointer("/screenshot/path")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(answer)
}

/// A QA scenario's verdict, into the folder the request belongs to.
pub(super) fn answer_verdict(
    command: &zerocode_core::computer_use::ComputerCommand,
    argv: &[String],
    dir: Option<&Path>,
) -> zerocode_hookd::TeamAnswer {
    use zerocode_core::computer_use_protocol::error_code;
    let Some(dir) = dir else {
        return computer_cli_error(
            command.json,
            error_code::INVALID_ARGUMENT,
            "a verdict needs an evidence folder — this pane belongs to no run and no session stands",
        );
    };
    let pass = command
        .params
        .get("pass")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let reason = command
        .params
        .get("reason")
        .and_then(serde_json::Value::as_str);
    match computer_use::evidence::write_verdict(dir, argv, pass, reason, now_epoch_ms()) {
        Ok(file) => {
            // The folder's report, rendered from its files now that the
            // verdict is among them (§3.5) — none at `off`; a report that
            // could not be written is said, the verdict stands.
            let (report, report_error) = match computer_use::report::write(dir) {
                Ok(report) => (report.map(|path| path.display().to_string()), None),
                Err(why) => (None, Some(why)),
            };
            let result = serde_json::json!({
                "pass": pass, "reason": reason, "dir": dir.display().to_string(), "file": file.display().to_string(),
                "report": report, "reportError": report_error,
            });
            let envelope = serde_json::json!({ "ok": true, "result": result });
            if command.json {
                computer_said(format!("{envelope}\n"))
            } else {
                computer_said(format!(
                    "verdict: {} — {}{}\n",
                    if pass { "pass" } else { "fail" },
                    reason.unwrap_or(dir.to_str().unwrap_or_default()),
                    result["report"]
                        .as_str()
                        .map_or(String::new(), |report| format!(" · report: {report}"))
                ))
            }
        }
        Err(why) => computer_cli_error(command.json, error_code::ACCESSIBILITY_ERROR, &why),
    }
}

/// The line a Flow's verdict is logged as: the `verdict` verb's own words,
/// so the log reads back the way a hand-written verdict does.
fn verdict_words(pass: bool, reason: Option<&str>) -> Vec<String> {
    let mut words = vec![
        zerocode_core::computer_use::ComputerMethod::Verdict
            .verb_name()
            .to_string(),
        if pass { "--pass" } else { "--fail" }.to_string(),
    ];
    if let Some(reason) = reason {
        words.extend(["--reason".to_string(), reason.to_string()]);
    }
    words
}

/// What `evidence` answers for a folder: its last steps — and, asked
/// `--verify`, whether the folder's report and verdict reproduce from its
/// files alone (`computer_use::report::verify`).
pub(super) fn evidence_report(
    command: &zerocode_core::computer_use::ComputerCommand,
    dir: Option<&Path>,
) -> serde_json::Value {
    let last = command
        .params
        .get("last")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(computer_use::evidence::REPORT_STEPS);
    let mut report = computer_use::evidence::report(dir, last);
    if command
        .params
        .get("verify")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        report["verify"] = dir.map_or(serde_json::Value::Null, |dir| {
            serde_json::to_value(computer_use::report::verify(dir))
                .unwrap_or(serde_json::Value::Null)
        });
    }
    report
}

/// An answered command's envelope as the CLI prints it: JSON when asked,
/// else the verb's own pretty form.
pub(super) fn said_envelope(
    command: &zerocode_core::computer_use::ComputerCommand,
    result: serde_json::Value,
) -> zerocode_hookd::TeamAnswer {
    let envelope = serde_json::json!({ "ok": true, "result": result });
    if command.json {
        computer_said(format!("{envelope}\n"))
    } else {
        computer_said(format!(
            "{}\n",
            computer_pretty(command.method, &envelope["result"])
        ))
    }
}

/// Run a program the way a person would from a terminal, bounded by the
/// tables (`COMPUTER_RUN_MAX_MS`, `COMPUTER_RUN_MAX_BYTES`): spawn it, read
/// both streams to the end on their own threads, and kill it at the budget.
/// The answer says what it printed, how it ended, and whether anything was
/// cut — never a hang, never an unbounded blob.
pub(super) fn desktop_run(params: &serde_json::Value) -> serde_json::Value {
    use std::io::Read as _;
    use zerocode_core::computer_use::{desktop_run_ms, desktop_run_output};
    let program = params
        .get("program")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let args: Vec<&str> = params
        .get("args")
        .and_then(serde_json::Value::as_str)
        .map(|words| words.split_whitespace().collect())
        .unwrap_or_default();
    let (budget_ms, capped) =
        desktop_run_ms(params.get("timeoutMs").and_then(serde_json::Value::as_u64));
    let mut command = crate::proc::quiet_command(program);
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(cwd) = params.get("cwd").and_then(serde_json::Value::as_str) {
        command.current_dir(cwd);
    }
    let started = std::time::Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return serde_json::json!({
                "program": program, "spawned": false, "error": error.to_string(),
            });
        }
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    });
    let budget = std::time::Duration::from_millis(budget_ms);
    let poll = std::time::Duration::from_millis(20);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= budget => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(poll),
            Err(_) => {
                let _ = child.kill();
                break child.wait().ok();
            }
        }
    };
    let (stdout_text, stdout_cut) = desktop_run_output(&out_reader.join().unwrap_or_default());
    let (stderr_text, stderr_cut) = desktop_run_output(&err_reader.join().unwrap_or_default());
    serde_json::json!({
        "program": program,
        "spawned": true,
        "exitCode": status.and_then(|status| status.code()),
        "timedOut": timed_out,
        "budgetMs": budget_ms,
        "budgetCapped": capped,
        "elapsedMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "stdout": stdout_text,
        "stderr": stderr_text,
        "truncated": stdout_cut || stderr_cut,
    })
}

pub(super) fn computer_said(stdout: impl Into<String>) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_code: 0,
    }
}

pub(super) fn computer_refused(stderr: impl Into<String>) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: String::new(),
        stderr: format!("{}\n", stderr.into().trim_end()),
        exit_code: 1,
    }
}

pub(super) fn computer_cli_error(
    json: bool,
    code: &str,
    message: &str,
) -> zerocode_hookd::TeamAnswer {
    if json {
        computer_refused(
            serde_json::json!({ "ok": false, "error": { "code": code, "message": message } })
                .to_string(),
        )
    } else {
        computer_refused(format!(
            "{}: {message}",
            zerocode_core::computer_use::COMPUTER_CLI
        ))
    }
}

pub(super) fn computer_pretty(
    method: zerocode_core::computer_use::ComputerMethod,
    value: &serde_json::Value,
) -> String {
    use zerocode_core::computer_use::ComputerMethod;
    match method {
        ComputerMethod::ListApps => value
            .get("apps")
            .and_then(serde_json::Value::as_array)
            .map(|apps| {
                apps.iter()
                    .map(|app| {
                        format!(
                            "{}\tpid:{}\t{}",
                            app.get("name")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("app"),
                            app.get("pid")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default(),
                            app.get("bundleId")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| "No apps found.".into()),
        ComputerMethod::ListWindows => value
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .map(|windows| {
                windows
                    .iter()
                    .map(|window| {
                        format!(
                            "[{}] id:{} \"{}\" ({}x{})",
                            window
                                .get("index")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default(),
                            window
                                .get("id")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default(),
                            window
                                .get("title")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default(),
                            window
                                .get("width")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default(),
                            window
                                .get("height")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|| "No windows found.".into()),
        ComputerMethod::GetAppState => {
            let tree = value
                .pointer("/snapshot/treeText")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let screenshot = value
                .pointer("/screenshot/path")
                .and_then(serde_json::Value::as_str)
                .map(|path| format!("\nScreenshot: {path}"))
                .unwrap_or_default();
            format!("{tree}{screenshot}")
        }
        _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    }
}

pub(super) fn browser_said(stdout: impl Into<String>) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_code: 0,
    }
}

/// An answer whose words the PAGE wrote — a title, visible text, a console
/// line, a request URL, an evaluated value — handed to the agent inside the
/// one fence every agent road uses (`zerocode_core::untrusted`). These tabs
/// carry the person's signed-in sessions, so a page's text is the likeliest
/// place for an order aimed at the agent to hide. Host-made answers (a label,
/// a click report, a wait's yes) stay [`browser_said`]. The door already
/// bounds each answer, so the fence adds its markers and cuts nothing.
pub(super) fn page_said(source: &str, words: impl Into<String>) -> zerocode_hookd::TeamAnswer {
    browser_said(zerocode_core::untrusted::fence(
        source,
        &words.into(),
        usize::MAX,
    ))
}

/// The source a fence names when the words came from every open tab at
/// once (`list`, `tabs`) rather than one page.
const EVERY_TAB: &str = "browser tabs";

pub(super) fn browser_refused(stderr: impl Into<String>) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: String::new(),
        stderr: stderr.into(),
        exit_code: 1,
    }
}

/// A read's words — the page's title and address, then its text, then a
/// selector read's reduced DOM. The one formatter both read roads print
/// through, inside the read arm's one fence: the plain read's and the read
/// seat's, so a page the seat hands back whole is the plain read's bytes.
fn read_words(report: cmd::browser::BrowserReadReport) -> String {
    let detail = report
        .dom
        .map(|dom| format!("\nDOM:\n{dom}\n"))
        .unwrap_or_default();
    format!(
        "제목: {}\n주소: {}\n\n{}\n{}",
        report.title, report.url, report.text, detail
    )
}

/// A typing's answer, on either road — and the label it leaves on the
/// pane's last judged read.
fn typed_answer(
    label: &str,
    typed: Result<cmd::browser::BrowserInputReport, String>,
) -> zerocode_hookd::TeamAnswer {
    if let Ok(report) = &typed {
        crate::browser_read::label_press(label, "type", report);
    }
    match typed {
        Ok(report) => browser_said(format!(
            "{}\n",
            cmd::browser::input_said(cmd::browser::TYPED_SAID, &report)
        )),
        Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
    }
}

pub(super) async fn answer_browser_command(
    app: &AppHandle,
    argv: &[String],
    pane: Option<&str>,
) -> zerocode_hookd::TeamAnswer {
    let state = app.state::<AppState>();
    let verb = argv.first().map(String::as_str).unwrap_or_default();
    match (verb, argv.len()) {
        ("list", 1) => {
            let mut labels: Vec<String> = state.browser_panes().iter().cloned().collect();
            labels.sort();
            let mut lines = String::new();
            for label in labels {
                if browser_pane_of(app, &state, &label).is_err() {
                    continue;
                }
                // The pane's recorded address, never the webview's answer: wry
                // unwraps a nil URL (a failed load) on the main thread and the
                // window dies with it (2026-09-07, three times).
                let url = state
                    .browser_urls()
                    .get(&label)
                    .map(|url| cmd::browser::scrub_url_credentials(url))
                    .unwrap_or_default();
                lines.push_str(&format!("{label}\t{url}\n"));
            }
            if lines.is_empty() {
                // The host's own hint, not a page's words: no fence.
                return browser_said(
                    "(열린 브라우저 판이 없습니다 — `zerocode-browser open <url>`)\n",
                );
            }
            page_said(EVERY_TAB, lines)
        }
        ("open", 2) => {
            // Validated HERE as well as in the window: the emit crosses a
            // trust boundary, and the window's own guard catching it later
            // would still have let an agent see a laxer door.
            let url = match browsable(&argv[1]) {
                Ok(parsed) => parsed.to_string(),
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            // The label is reserved BEFORE the window opens anything, so the
            // answer is the label itself (§2.2): the window claims the
            // reservation in `open_browser_pane`, and this side waits inside
            // the door's budget for the pane to appear in the mint. Lock
            // order born→reserved, the same as the claim.
            let label = reserve_label(&mut state.browser_born(), &mut state.browser_reserved());
            // The WINDOW opens it — the tab, the toolbar and the pane's seat
            // are the frontend's, and this is the same road a popup walks.
            // An emit that failed is a request that never left, and saying
            // success about it would strand the agent's next `tabs`.
            let asked = app.emit_to(
                "main",
                "browser:agent-open",
                BrowserPopup {
                    label: label.clone(),
                    url,
                    // Seated where it was asked for, not where the person is
                    // looking (live report 2026-09-03).
                    term: pane.and_then(hooks::term_of_pane_key),
                },
            );
            if let Err(error) = asked {
                return browser_refused(format!("zerocode-browser: {error}\n"));
            }
            let opened = cmd::browser::wait_until(
                cmd::browser::BROWSER_DOOR_BUDGET,
                cmd::browser::BROWSER_WAIT_POLL,
                || state.browser_panes().contains(&label),
            )
            .await;
            browser_said(cmd::browser::open_answer(&label, opened))
        }
        ("close", 2) => {
            let label = argv[1].clone();
            if !state.browser_panes().contains(&label) {
                return browser_refused("zerocode-browser: 이 창이 만든 브라우저 판이 아닙니다\n");
            }
            // The WINDOW closes it, through the one `closeTab` every close
            // walks (hide→close→burn); this side watches the mint for the
            // label to leave, inside the budget.
            let asked = app.emit_to(
                "main",
                "browser:agent-close",
                BrowserAgentClose {
                    label: label.clone(),
                },
            );
            if let Err(error) = asked {
                return browser_refused(format!("zerocode-browser: {error}\n"));
            }
            let closed = cmd::browser::wait_until(
                cmd::browser::BROWSER_DOOR_BUDGET,
                cmd::browser::BROWSER_WAIT_POLL,
                || !state.browser_panes().contains(&label),
            )
            .await;
            browser_said(cmd::browser::close_answer(&label, closed))
        }
        ("viewport", 3) => {
            let viewport = match zerocode_core::agent_browser::parse_viewport(&argv[2]) {
                Ok(viewport) => viewport,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            if !state.browser_panes().contains(&argv[1]) {
                return browser_refused("zerocode-browser: 이 창이 만든 브라우저 판이 아닙니다\n");
            }
            // A size is the window's to apply — the pane is placed by the
            // window's own preset road (`viewportBox`), so the box the agent
            // asked for is the box the person would have picked.
            let asked = app.emit_to(
                "main",
                "browser:agent-viewport",
                BrowserAgentViewport {
                    label: argv[1].clone(),
                    viewport: viewport.window_word(),
                },
            );
            match asked {
                Ok(()) => browser_said(format!("뷰포트 {} {}\n", argv[1], viewport.said())),
                Err(error) => browser_refused(format!("zerocode-browser: {error}\n")),
            }
        }
        ("scroll", 3) => {
            let target = match zerocode_core::agent_browser::parse_scroll(&argv[2]) {
                Ok(target) => target,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            match cmd::browser::automate_scroll(app, &state, &argv[1], &target).await {
                Ok((x, y)) => browser_said(format!("스크롤 x={x} y={y}\n")),
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("diagnose", _) => {
            let command = match zerocode_core::agent_browser::parse_diagnose(argv) {
                Ok(command) => command,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            match cmd::browser::automate_diagnose(app, &state, &command.label).await {
                Ok(facts) => {
                    let verdict = diagnosis(&facts);
                    // `--json` stays parseable and carries the fence's JSON
                    // key instead; the paragraph rides inside the fence.
                    if command.json {
                        browser_said(format!("{}\n", diagnose_json(&facts, &verdict)))
                    } else {
                        page_said(&command.label, diagnose_words(&facts, &verdict))
                    }
                }
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("find", 3) => {
            match cmd::browser::automate_find(app, &state, &argv[1], &argv[2], true, false).await {
                Ok(found) => browser_said(format!(
                    "{}\n",
                    found.unwrap_or_else(|| json!({ "count": 0, "index": 0 }))
                )),
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("goto", 3) => {
            let url = match browsable(&argv[2]) {
                Ok(parsed) => parsed,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            match browser_pane_of(app, &state, &argv[1]) {
                Ok(pane) => match pane.navigate(url) {
                    Ok(()) => browser_said("가는 중\n"),
                    Err(_) => browser_refused("zerocode-browser: 이동 요청을 보낼 수 없습니다\n"),
                },
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("eval", 3) => match cmd::browser::automate_eval(app, &state, &argv[1], &argv[2]).await {
            Ok(value) => page_said(
                &argv[1],
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".to_string())
                ),
            ),
            Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
        },
        // `read <label> [css]` reads the page or a selector; `read <label>
        // --full` reads the page whole, whatever the read seat would fold.
        // The judged road and the plain road print through ONE formatter
        // (`read_answer`), so a read the seat hands back whole is the plain
        // read's bytes.
        ("read", 2) | ("read", 3) => {
            let read = match zerocode_core::agent_browser::parse_read(argv) {
                Ok(read) => read,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            let mode = zerocode_core::jev::BROWSER_READ
                .mode_in(&crate::systemone::Wire::of_this_machine().settings_root());
            let answered = if read.selector.is_none() && !read.full && mode.asks() {
                // The words' workspace is the checkout the asking pane runs
                // in — what the door asks the person's consent for.
                let workspace = pane
                    .and_then(hooks::term_of_pane_key)
                    .and_then(|term| state.pane_cwds().get(&term).cloned())
                    .map(std::path::PathBuf::from);
                crate::browser_read::read_judged(app, &state, &read.label, workspace, mode).await
            } else {
                cmd::browser::automate_read(app, &state, &read.label, read.selector.as_deref())
                    .await
            };
            match answered {
                Ok(report) => {
                    // A whole read after a fold is the fold's own label
                    // (t-6155 F6): written after the page really was read.
                    if read.full && read.selector.is_none() && mode.asks() {
                        crate::browser_read::label_full_read(&read.label, &report.url);
                    }
                    page_said(&read.label, read_words(report))
                }
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        // `click <label> <css>` presses the first element a selector names;
        // `click <label> --mark <n>` presses the control numbered n on the
        // pane's last `marks`, pinned so a moved or changed control is refused;
        // `--settle-later` answers at once with the page the press changed, its
        // settle finished by the pane's next `marks` (t-9712) — a press that
        // left the legend as it was settles first, as a plain one does (t-9876).
        ("click", 3) | ("click", 4) | ("click", 5) => {
            use zerocode_core::agent_browser::ClickTarget;
            let pressed = match zerocode_core::agent_browser::parse_click(argv) {
                Ok(ClickTarget::Css(css)) => {
                    cmd::browser::automate_click(app, &state, &argv[1], &css).await
                }
                Ok(ClickTarget::Mark(mark)) => {
                    cmd::browser::automate_click_mark(app, &state, &argv[1], mark).await
                }
                Ok(ClickTarget::MarkSettleLater(mark)) => {
                    let later =
                        cmd::browser::automate_click_mark_later(app, &state, &argv[1], mark);
                    return match later.await {
                        Ok((report, look)) => {
                            crate::browser_read::label_press(&argv[1], "click", &report);
                            let answer = cmd::browser::settle_later_json(&report, look.as_ref());
                            browser_said(format!("{answer}\n"))
                        }
                        Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
                    };
                }
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            match pressed {
                Ok(report) => {
                    crate::browser_read::label_press(&argv[1], "click", &report);
                    browser_said(format!(
                        "{}\n",
                        cmd::browser::input_said(cmd::browser::CLICK_SAID, &report)
                    ))
                }
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        // `type <label> <css> <text>` types with its keys; `type <label> <css>
        // --value <text>` — the shape the shim sends for `--value-stdin` —
        // writes with the setter alone (review 8).
        ("type", 4) => {
            let (label, css, text) = (&argv[1], &argv[2], &argv[3]);
            let road = cmd::browser::TypeRoad::Keys;
            typed_answer(
                label,
                cmd::browser::automate_type(app, &state, label, css, text, road).await,
            )
        }
        ("type", 5) if argv[3] == zerocode_core::agent_browser::TYPE_VALUE_FLAG => {
            let (label, css, text) = (&argv[1], &argv[2], &argv[4]);
            let road = cmd::browser::TypeRoad::Setter;
            typed_answer(
                label,
                cmd::browser::automate_type(app, &state, label, css, text, road).await,
            )
        }
        ("wait", 3) | ("wait", 4) => {
            let timeout_ms = match argv.get(3) {
                Some(raw) => match raw.parse::<u64>() {
                    Ok(timeout) => Some(timeout),
                    Err(_) => {
                        return browser_refused(
                            "zerocode-browser: 대기 시간은 밀리초 정수여야 합니다\n",
                        );
                    }
                },
                None => None,
            };
            match cmd::browser::automate_wait(app, &state, &argv[1], &argv[2], timeout_ms).await {
                Ok(()) => browser_said("조건이 충족됐습니다\n"),
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("tabs", 1) => {
            // Every column is a record a hook wrote — the webview is never
            // asked (the URL crash of 2026-09-07). The state is the clock's
            // reading of the record, so a load past NAV_DEAD_AFTER_SECS says
            // `dead` here even before the timer that emits it has fired.
            let now = std::time::Instant::now();
            let mut labels: Vec<String> = state.browser_panes().iter().cloned().collect();
            labels.sort_by_key(|label| birth_number(label));
            let rows: Vec<TabRow> = labels
                .into_iter()
                .filter(|label| browser_pane_of(app, &state, label).is_ok())
                .map(|label| {
                    let record = state.browser_records().get(&label).cloned();
                    let url = state
                        .browser_urls()
                        .get(&label)
                        .map(|url| cmd::browser::scrub_url_credentials(url))
                        .unwrap_or_default();
                    TabRow {
                        label,
                        state: record
                            .as_ref()
                            .map_or(NavState::Loading, |record| record.nav.observed(now)),
                        title: record
                            .as_ref()
                            .map(|record| record.title.clone())
                            .unwrap_or_default(),
                        url,
                        profile: record.as_ref().and_then(|record| record.profile.clone()),
                        reader: record.and_then(|record| record.reader),
                    }
                })
                .collect();
            if rows.is_empty() {
                // The host's own hint, not a page's words: no fence.
                return browser_said(tabs_lines(&rows));
            }
            page_said(EVERY_TAB, tabs_lines(&rows))
        }
        ("console", _) => {
            let command = match zerocode_core::agent_browser::parse_console(argv) {
                Ok(command) => command,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            let (label, since, level) = (&command.label, command.since, command.level.as_str());
            match cmd::browser::automate_console(app, &state, label, since, level).await {
                Ok(take) => page_said(
                    label,
                    cmd::browser::console_lines(&take, local_offset_secs()),
                ),
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("network", _) => {
            let command = match zerocode_core::agent_browser::parse_network(argv) {
                Ok(command) => command,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            let (label, since, failed) = (&command.label, command.since, command.failed_only);
            match cmd::browser::automate_network(app, &state, label, since, failed).await {
                Ok(take) => page_said(label, cmd::browser::network_lines(&take)),
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("marks", _) => {
            let command = match zerocode_core::agent_browser::parse_marks(argv) {
                Ok(command) => command,
                Err(why) => return browser_refused(format!("zerocode-browser: {why}\n")),
            };
            match cmd::browser::automate_marks(app, &state, &command.label).await {
                // The labels are the page's own words. The legend is fenced
                // like `read`; the `--json` answer carries the fence's flag
                // instead, because a program reads that one and marker lines
                // make it unparseable (`marks_json`).
                Ok(marks) if command.json => {
                    browser_said(format!("{}\n", cmd::browser::marks_json(&marks)))
                }
                Ok(marks) => page_said(&command.label, cmd::browser::marks_lines(&marks)),
                Err(why) => browser_refused(format!("zerocode-browser: {why}\n")),
            }
        }
        ("screenshot", _) => {
            let wants_json = argv.iter().any(|word| word == "--json");
            let command = match zerocode_core::agent_browser::parse_screenshot(argv) {
                Ok(command) => command,
                Err(why) => return browser_screenshot_refused(wants_json, &why),
            };
            match cmd::fs::browser_snapshot_png(app, &state, &command.label).await {
                Ok(bytes) => {
                    // `--marks` lays the pane's last marks onto the picture
                    // before it is written; without a marks answer to draw,
                    // the flag is a clear refusal, not a bare screenshot.
                    let bytes = if command.marks {
                        match cmd::browser::overlay_last_marks(&command.label, &bytes) {
                            Ok(marked) => marked,
                            Err(why) => return browser_screenshot_refused(command.json, &why),
                        }
                    } else {
                        bytes
                    };
                    let byte_count = bytes.len();
                    match write_agent_screenshot("browser", &bytes, command.out.as_deref(), None) {
                        Ok(path) if command.json => browser_said(format!(
                            "{}\n",
                            json!({
                                "ok": true,
                                "result": { "path": path, "bytes": byte_count },
                            })
                        )),
                        Ok(path) => browser_said(format!("Screenshot: {path}\n")),
                        Err(why) => browser_screenshot_refused(command.json, &why),
                    }
                }
                Err(why) => browser_screenshot_refused(command.json, &why),
            }
        }
        _ => browser_refused(format!("{}\n", zerocode_core::agent_browser::usage())),
    }
}

pub(super) fn browser_screenshot_refused(json: bool, message: &str) -> zerocode_hookd::TeamAnswer {
    if json {
        browser_refused(format!(
            "{}\n",
            json!({ "ok": false, "error": { "code": "screenshot_error", "message": message } })
        ))
    } else {
        browser_refused(format!("zerocode-browser: {message}\n"))
    }
}

/// Persist a bounded PNG and return an absolute path. The default destination
/// is private scratch space; an explicit destination uses normal CLI overwrite
/// semantics but is only touched after the capture itself has succeeded.
/// `cwd` is where the shell that asked stands, when its door said so
/// (`EMULATOR_CWD_VERBS`): a relative `requested` is taken from there, as a
/// person's would be. A door that does not say leaves a relative path to this
/// process's own folder, as it always did.
pub(super) fn write_agent_screenshot(
    surface: &str,
    bytes: &[u8],
    requested: Option<&str>,
    cwd: Option<&Path>,
) -> Result<String, String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err("screenshot backend did not return a PNG".to_string());
    }
    if bytes.len() > AGENT_SCREENSHOT_MAX_BYTES {
        return Err(format!(
            "screenshot exceeds the {AGENT_SCREENSHOT_MAX_BYTES} byte limit"
        ));
    }
    let path = match requested {
        Some(path) if !path.is_empty() => match cwd {
            Some(cwd) if Path::new(path).is_relative() => cwd.join(path),
            _ => PathBuf::from(path),
        },
        Some(_) => return Err("--out needs a path".to_string()),
        None => {
            let directory = std::env::temp_dir().join("zerocode-screenshots");
            std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                    .map_err(|error| error.to_string())?;
            }
            directory.join(format!("{surface}-{}.png", uuid::Uuid::new_v4()))
        }
    };
    std::fs::write(&path, bytes)
        .map_err(|error| format!("could not write screenshot to {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    Ok(path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned())
}
