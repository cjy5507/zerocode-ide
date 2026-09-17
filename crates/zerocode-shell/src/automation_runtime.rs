use super::*;

/// One checkout's review, keyed the way the board joins it.
#[derive(Serialize, Clone)]
pub(super) struct ReviewState {
    /// The checkout this was asked in. The join key, and the reason the answer
    /// is a list rather than a bare mark.
    pub(super) worktree: String,
    /// The branch the answer was measured on. `gh pr view` answers about
    /// whatever HEAD is on, so a mark without its branch is an answer to a
    /// question the asker cannot check — the original scopes its own review
    /// snapshot by repo, worktree AND branch for exactly this reason
    /// ("scoped so a response for a previous target is ignored",
    /// use-hosted-review-state.ts:13-14).
    pub(super) branch: String,
    pub(super) number: u64,
    /// `open`, `draft`, `merged` or `closed` — [`gh::review_word`]'s four.
    pub(super) state: &'static str,
}

/// How long a checkout's review is believed without asking `gh` again.
///
/// The board repaints on every hook event, and every repaint asks for this
/// map. A minute is the distance between "the pill follows a merge within the
/// time it takes to notice" and "watching five agents work spawns a `gh` per
/// frame".
pub(super) const REVIEW_STATE_FRESH: Duration = Duration::from_secs(60);

/// How many checkouts are asked about at once.
///
/// Four. Each ask is a `gh` process and usually a network round trip, so one
/// at a time makes a board of eight workspaces wait eight round trips; and the
/// ceiling exists because this runs behind a repaint nobody asked for.
pub(super) const REVIEW_STATE_LANES: usize = 4;

/// What `gh` last said about each checkout's branch, and when.
///
/// A checkout with no review is remembered as `None` rather than left out —
/// otherwise the one answer that costs a process is the one never cached, and
/// a board of branches without pull requests would spawn `gh` on every paint.
///
/// The BRANCH is half the key. `gh pr view` answers for whatever HEAD is on,
/// so a checkout that moved to another branch would otherwise wear the
/// previous branch's pull request for the rest of the freshness window — and
/// with the review pill and the create door both reading this, that is a door
/// that stays hidden on a branch which has no review at all.
pub(super) type ReviewMemory = HashMap<(PathBuf, String), (Instant, Option<gh::ReviewMark>)>;

pub(super) fn review_states_cache() -> std::sync::MutexGuard<'static, ReviewMemory> {
    static STATES: std::sync::OnceLock<Mutex<ReviewMemory>> = std::sync::OnceLock::new();
    STATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The reviews these checkouts are on, asking `gh` only for what has gone
/// stale.
pub(super) fn review_states(worktrees: &[String]) -> Vec<ReviewState> {
    // The window sends one entry per card, and five cards can share a
    // workspace. Asking `gh` five times about one branch is five processes for
    // one answer.
    let mut paths: Vec<&str> = worktrees
        .iter()
        .map(String::as_str)
        .filter(|one| !one.is_empty())
        .collect();
    paths.sort_unstable();
    paths.dedup();

    // A checkout with no branch has nothing `gh pr view` can answer about, and
    // asking anyway spends a process to be told so — the original's polling
    // refuses the same two states before it fetches (an empty name and
    // `HEAD`, use-hosted-review-polling.ts:63-73). The branch is read from
    // the checkout's own HEAD rather than asked of git: this runs once per
    // card, behind a repaint nobody requested.
    let wanted: Vec<(&str, String)> = paths
        .into_iter()
        .filter_map(|one| current_branch(Path::new(one)).map(|branch| (one, branch)))
        .collect();

    let mut held: HashMap<&str, Option<gh::ReviewMark>> = HashMap::new();
    let mut asking: Vec<(&str, &str)> = Vec::new();
    {
        let mut cache = review_states_cache();
        // Stale rows are dropped rather than overwritten, so the map holds
        // exactly what is fresh and a checkout that has left the board stops
        // being remembered without a second bookkeeping pass.
        let now = Instant::now();
        cache.retain(|_, (at, _)| now.duration_since(*at) < REVIEW_STATE_FRESH);
        for (one, branch) in &wanted {
            match cache.get(&(PathBuf::from(*one), branch.clone())) {
                Some((_, mark)) => {
                    held.insert(one, *mark);
                }
                None => asking.push((one, branch)),
            }
        }
    }

    // Four at a time, waiting for each batch before the next — so five
    // checkouts never put five `gh` processes on the machine at once.
    for chunk in asking.chunks(REVIEW_STATE_LANES) {
        let asked = std::thread::scope(|scope| {
            let running: Vec<_> = chunk
                .iter()
                .map(|(one, _)| {
                    let at = *one;
                    scope.spawn(move || gh::fetch_review_state(Path::new(at)))
                })
                .collect();
            running
                .into_iter()
                .map(|one| one.join().unwrap_or(None))
                .collect::<Vec<_>>()
        });
        let mut cache = review_states_cache();
        let at = Instant::now();
        for ((one, branch), mark) in chunk.iter().zip(asked) {
            cache.insert((PathBuf::from(*one), (*branch).to_string()), (at, mark));
            held.insert(one, mark);
        }
    }

    wanted
        .into_iter()
        .filter_map(|(one, branch)| {
            let mark = held.get(one).copied().flatten()?;
            Some(ReviewState {
                worktree: one.to_string(),
                branch,
                number: mark.number,
                state: mark.state,
            })
        })
        .collect()
}

/* ---- review notes ----------------------------------------------------------
 *
 * Notes written on diff lines, held until they are handed to an agent. Orca
 * keeps them on the worktree in its persisted store; here they live in one
 * file beside the automations, each row naming its workspace — same reader
 * posture too: a file that will not parse answers empty rather than keeping
 * the window from opening over a malformed note. */

pub(super) fn diff_notes_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::DIFF_NOTES)
}

pub(super) fn stored_diff_notes(config_root: &Path) -> Vec<DiffNote> {
    std::fs::read_to_string(diff_notes_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<DiffNote>>(&text).ok())
        .unwrap_or_default()
}

pub(super) fn write_diff_notes(config_root: &Path, rows: &[DiffNote]) -> Result<(), String> {
    let file = diff_notes_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(rows).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// Write a note, or rewrite the one that already carries this id.
pub(super) fn validate_diff_note(note: &DiffNote) -> Result<(), String> {
    if note.body.trim().is_empty() {
        return Err("내용 없는 노트는 저장하지 않습니다".to_string());
    }
    Ok(())
}

/// The local clock's offset from UTC, in seconds — derived from the same
/// `localtime_r` road as [`local_minute_now`], so there is still one local
/// clock in this program. Offsets are whole minutes everywhere on earth.
pub(super) fn local_offset_secs() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    local_minute_now().epoch_minutes * 60 - secs.div_euclid(60) * 60
}

/// The minute it is now, locally.
///
/// The offset comes from the platform rather than from a timezone crate: the
/// only question asked is "which local minute is this", and `localtime_r` is
/// the operating system's own answer to it — including whatever daylight-saving
/// rule is in force today, which a bundled table would have to be kept current
/// to match.
#[cfg(unix)]
pub(super) fn local_minute_now() -> LocalMinute {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    // `localtime_r` fills a struct the platform owns the layout of. The call is
    // unsafe because it writes through a raw pointer; it is sound here because
    // the buffer is exactly the struct being asked for and is read only after
    // the call returns non-null.
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    let filled = unsafe { libc::localtime_r(&raw const secs, tm.as_mut_ptr()) };
    if filled.is_null() {
        return LocalMinute {
            epoch_minutes: secs.div_euclid(60),
        };
    }
    let tm = unsafe { tm.assume_init() };
    LocalMinute {
        epoch_minutes: (secs + tm.tm_gmtoff).div_euclid(60),
    }
}

#[cfg(windows)]
pub(super) fn local_minute_now() -> LocalMinute {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
    }
    unsafe extern "C" {
        fn _localtime64_s(result: *mut Tm, time: *const i64) -> i32;
    }
    let mut tm = std::mem::MaybeUninit::<Tm>::zeroed();
    let res = unsafe { _localtime64_s(tm.as_mut_ptr(), &raw const secs) };
    if res == 0 {
        let tm = unsafe { tm.assume_init() };
        // Howard Hinnant's days-from-civil to calculate exact local epoch minutes
        let y = (tm.tm_year as i64) + 1900;
        let m = (tm.tm_mon as i64) + 1;
        let d = tm.tm_mday as i64;
        let y = if m <= 2 { y - 1 } else { y };
        let era = (if y >= 0 { y } else { y - 399 }) / 400;
        let yoe = (y - era * 400) as u32;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u32;
        let days = era * 146_097 + (doe as i64) - 719_468;
        let local_minutes = days * 1440 + (tm.tm_hour as i64) * 60 + (tm.tm_min as i64);
        return LocalMinute {
            epoch_minutes: local_minutes,
        };
    }
    LocalMinute {
        epoch_minutes: secs.div_euclid(60),
    }
}

#[cfg(all(not(unix), not(windows)))]
pub(super) fn local_minute_now() -> LocalMinute {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    LocalMinute {
        epoch_minutes: secs.div_euclid(60),
    }
}

/// Take a job and the history that belongs to it off disk. Worktrees made by
/// earlier runs deliberately remain: they are repositories, not history rows.
pub(super) fn delete_automation_and_run_history(
    config_root: &Path,
    local_data_root: &Path,
    id: &str,
) -> Result<(), String> {
    let mut domain = automation_store_domain();
    let mut rows = stored_automations(config_root);
    let existed = rows.iter().any(|held| held.id == id);
    rows.retain(|held| held.id != id);
    write_automations(config_root, &rows)?;
    if existed {
        domain.replace_incarnation(id);
    }

    let mut history = stored_automation_runs(local_data_root);
    history.retain(|run| run.automation_id != id);
    write_automation_runs(local_data_root, &history)?;
    // Its evidence goes with its history: those folders are the rows' files,
    // and a job nobody can look up any more has nothing to show them in.
    let _ = std::fs::remove_dir_all(local_data_root.join("automations").join(id));
    Ok(())
}

/// Remove a worktree that this firing created but never managed to launch in.
///
/// `ConfirmedIfClean` is the safety boundary: the path was minted moments ago,
/// but if anything has managed to write into it we preserve it and carry the
/// refusal into the failure row. A failed spawn must not become authority to
/// discard work.
pub(super) fn cleanup_failed_automation_worktree(
    local_data_root: &Path,
    orchestrator: &Orchestrator,
    path: &Path,
) -> (Option<String>, Option<String>) {
    match remove_automatic_worktree(orchestrator, path) {
        Ok(()) => {
            note_worktree_removal(
                local_data_root,
                "failed-automation",
                path,
                "the firing that cut it never opened a terminal",
            );
            (None, None)
        }
        Err(error) => {
            note_window_event(
                local_data_root,
                &format!(
                    "automatic worktree cleanup skipped for {}: {error}",
                    path.display()
                ),
            );
            (Some(path.display().to_string()), Some(error))
        }
    }
}

/// Consume an occurrence that could not open a usable terminal and keep the
/// reason in the same ledger as successful and precheck-stopped runs.
pub(super) struct AutomationFailureInput {
    pub(super) stage: AutomationFailureStage,
    pub(super) error: String,
    pub(super) root: PathBuf,
    pub(super) made_worktree: Option<String>,
    pub(super) worktree_cleanup_error: Option<String>,
}

impl AutomationFailureInput {
    pub(super) fn new(
        stage: AutomationFailureStage,
        error: String,
        root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            stage,
            error,
            root: root.into(),
            made_worktree: None,
            worktree_cleanup_error: None,
        }
    }

    pub(super) fn with_cleanup(
        mut self,
        made_worktree: Option<String>,
        worktree_cleanup_error: Option<String>,
    ) -> Self {
        self.made_worktree = made_worktree;
        self.worktree_cleanup_error = worktree_cleanup_error;
        self
    }
}

pub(super) fn remember_automation_failure(
    state: &AppState,
    automation: &Automation,
    claim: &AutomationRunClaim,
    at: LocalMinute,
    failure: AutomationFailureInput,
) -> String {
    let AutomationFailureInput {
        stage,
        error,
        root,
        made_worktree,
        worktree_cleanup_error,
    } = failure;
    let at_epoch_ms = now_epoch_ms();
    let run = AutomationRun {
        id: format!("{at_epoch_ms}-failed"),
        automation_id: automation.id.clone(),
        scheduled_for_epoch_minutes: Some(at.epoch_minutes),
        at_epoch_ms,
        root: root.display().to_string(),
        term: FLOAT_TERM,
        made_worktree,
        skipped: None,
        failure: Some(AutomationFailureRecord {
            stage,
            scheduled_for_epoch_minutes: at.epoch_minutes,
            error: error.clone(),
            worktree_cleanup_error,
        }),
        ended_at_epoch_ms: Some(at_epoch_ms),
        exit_code: None,
        completed_at_epoch_ms: None,
        result: None,
        evidence_dir: None,
        launch: None,
        unwitnessed: false,
    };
    match remember_run(
        state.config_root(),
        state.local_data_root(),
        automation,
        claim,
        at,
        run,
    ) {
        Ok(()) => error,
        Err(store_error) => format!("{error} (실패 기록을 저장하지 못했습니다: {store_error})"),
    }
}

/// Start one job now: open a shell in its workspace and post its prompt.
///
/// Returns the shell it opened, so the window can bring that terminal forward
/// — a job that fires with nothing on screen is one nobody can supervise, and
/// supervising is the point of running agents in a window rather than in cron.
///
/// `Ok(None)` is the third answer, and it is not an error: the precheck said
/// no. The firing happened, the schedule was spent, and the history has a row
/// for it — nothing was started, and nobody needs an alarm about a guard doing
/// its job.
///
/// The run is recorded before the prompt is delivered, not after. A prompt
/// that times out still consumed the run: the agent was started, and firing
/// again a second later because the delivery failed would stack shells.
pub(super) fn start_automation(
    app: &AppHandle,
    automation: &Automation,
    at: LocalMinute,
    trigger: RunTrigger,
) -> Result<Option<StartedRun>, String> {
    let state = app.state::<AppState>();
    let (current_automation, claim) = claim_automation_run(state.config_root(), &automation.id)?;
    let automation = &current_automation;
    let settings_document = load_settings_for_boot(state.settings()).document;
    // The gate, BEFORE anything is made. A `NewPerRun` job whose precheck
    // fails must not leave behind a worktree nothing ever ran in — and the
    // command is asked in the checkout the record names, which for that mode
    // is the repository the cut would have come from.
    //
    // Which firings are gated is `should_run_precheck`'s answer and nobody
    // else's: scheduled only, and only when a command was written.
    if should_run_precheck(trigger, &automation.precheck) {
        let ran = script::run_precheck(
            &automation.precheck,
            Path::new(&automation.workspace),
            Duration::from_secs(u64::from(automation.precheck_timeout_seconds)),
        );
        if !ran.outcome.passed() {
            remember_run(
                state.config_root(),
                state.local_data_root(),
                automation,
                &claim,
                at,
                skipped_run(automation, &ran, at),
            )?;
            return Ok(None);
        }
    }

    // Resolve WHAT will be spawned before a per-run checkout is made. A
    // custom command is argv, parsed by the same quote-aware grammar as agent
    // launch settings. When the command is empty but an agent is named, the
    // agent catalogue — including launch subcommands — is the executable
    // source, and its saved launch args/env are part of the plan. Only a job
    // naming neither falls back to the configured terminal command/user shell.
    let mut launch_env: Vec<(String, String)> = Vec::new();
    let argv = if !automation.command.trim().is_empty() {
        match zerocode_core::split_command_line(&automation.command) {
            Ok(words) if !words.is_empty() => words,
            Ok(_) => {
                let error = "실행할 명령이 없습니다".to_string();
                return Err(remember_automation_failure(
                    &state,
                    automation,
                    &claim,
                    at,
                    AutomationFailureInput::new(
                        AutomationFailureStage::Command,
                        error,
                        automation.workspace.as_str(),
                    ),
                ));
            }
            Err(error) => {
                let error = format!("실행할 명령을 읽을 수 없습니다: {error}");
                return Err(remember_automation_failure(
                    &state,
                    automation,
                    &claim,
                    at,
                    AutomationFailureInput::new(
                        AutomationFailureStage::Command,
                        error,
                        automation.workspace.as_str(),
                    ),
                ));
            }
        }
    } else if let Some(agent) = automation.agent.as_deref() {
        let Some(spec) = agent_spec(agent) else {
            let error = format!("{agent}은(는) 이 창이 모르는 에이전트입니다");
            return Err(remember_automation_failure(
                &state,
                automation,
                &claim,
                at,
                AutomationFailureInput::new(
                    AutomationFailureStage::LaunchPlan,
                    error,
                    automation.workspace.as_str(),
                ),
            ));
        };
        let mut words = match zerocode_core::split_command_line(spec.launch) {
            Ok(words) if !words.is_empty() => words,
            Ok(_) => {
                let error = format!("{}의 실행 명령이 비어 있습니다", spec.name);
                return Err(remember_automation_failure(
                    &state,
                    automation,
                    &claim,
                    at,
                    AutomationFailureInput::new(
                        AutomationFailureStage::LaunchPlan,
                        error,
                        automation.workspace.as_str(),
                    ),
                ));
            }
            Err(error) => {
                let error = format!("{}의 실행 명령을 읽을 수 없습니다: {error}", spec.name);
                return Err(remember_automation_failure(
                    &state,
                    automation,
                    &claim,
                    at,
                    AutomationFailureInput::new(
                        AutomationFailureStage::LaunchPlan,
                        error,
                        automation.workspace.as_str(),
                    ),
                ));
            }
        };
        let launch_override = match stored_launch_override(state.settings(), spec.id) {
            Ok(held) => held,
            Err(error) => {
                let error = format!("{} 실행 설정을 읽을 수 없습니다: {error}", spec.name);
                return Err(remember_automation_failure(
                    &state,
                    automation,
                    &claim,
                    at,
                    AutomationFailureInput::new(
                        AutomationFailureStage::LaunchPlan,
                        error,
                        automation.workspace.as_str(),
                    ),
                ));
            }
        };
        let plan = zerocode_core::launch_plan(spec.id, launch_override.as_ref());
        words.extend(plan.args);
        launch_env = plan.env;
        words
    } else {
        let configured = split_command(&settings_document.terminal_command);
        if configured.is_empty() {
            let (program, args) = user_shell();
            std::iter::once(program).chain(args).collect()
        } else {
            configured
        }
    };
    let (program, args) = argv
        .split_first()
        .map(|(program, args)| (program.clone(), args.to_vec()))
        .expect("every automation launch branch returns a program");

    // In the checkout it names, not the one being looked at. An automation is
    // a standing instruction about a workspace; which tab happens to be open
    // when it fires is not part of it.
    //
    // `NewPerRun` cuts a fresh worktree first — the record's workspace then
    // names the REPOSITORY the checkout is cut from, and the run happens in
    // the checkout. The orchestrator is opened per run rather than kept: a
    // 9am job must see the repository as it is at 9am, not as it was when
    // the job was written.
    let (root, made, made_by) = match automation.workspace_mode {
        zerocode_core::WorkspaceMode::Existing => {
            let root = PathBuf::from(&automation.workspace);
            if !root.is_dir() {
                let error = format!("워크스페이스를 찾을 수 없습니다: {}", root.display());
                return Err(remember_automation_failure(
                    &state,
                    automation,
                    &claim,
                    at,
                    AutomationFailureInput::new(
                        AutomationFailureStage::Workspace,
                        error,
                        root.clone(),
                    ),
                ));
            }
            (root, None, None)
        }
        zerocode_core::WorkspaceMode::NewPerRun => {
            let opened = match Orchestrator::open(&automation.workspace) {
                Ok(opened) => opened,
                Err(error) => {
                    let error = error.to_string();
                    return Err(remember_automation_failure(
                        &state,
                        automation,
                        &claim,
                        at,
                        AutomationFailureInput::new(
                            AutomationFailureStage::Workspace,
                            error,
                            automation.workspace.as_str(),
                        ),
                    ));
                }
            };
            let orchestrator = match apply_workspace_creation_prefs(
                opened,
                &settings_document.workspace_creation_prefs,
            ) {
                Ok(orchestrator) => orchestrator,
                Err(error) => {
                    return Err(remember_automation_failure(
                        &state,
                        automation,
                        &claim,
                        at,
                        AutomationFailureInput::new(
                            AutomationFailureStage::Workspace,
                            error,
                            automation.workspace.as_str(),
                        ),
                    ));
                }
            };
            let task = WorktreeTask::from_spec(&automation.name, None, None);
            let cut = match orchestrator
                // The base is handed to git verbatim and git validates it —
                // a branch that stopped existing since the job was written
                // refuses here, tonight, out loud, instead of running the
                // job somewhere it did not mean.
                .create_from(&task, automation.base_branch.as_deref())
            {
                Ok(cut) => cut,
                Err(error) => {
                    let error = error.to_string();
                    return Err(remember_automation_failure(
                        &state,
                        automation,
                        &claim,
                        at,
                        AutomationFailureInput::new(
                            AutomationFailureStage::Workspace,
                            error,
                            automation.workspace.as_str(),
                        ),
                    ));
                }
            };
            record_cut_base(
                &Host::for_workspace(Path::new(&automation.workspace)),
                Path::new(&automation.workspace),
                &cut,
                automation.base_branch.as_deref(),
            );
            let path = cut.path.clone();
            (path.clone(), Some(path), Some(orchestrator))
        }
    };
    // A scheduled run is the same agent as a typed one, so it runs as the same
    // account. This shell was opening with an empty environment, which meant a
    // job firing at 9am ran under whatever login the machine had rather than
    // the one the picker names — invisibly, because nobody is watching at 9am.
    // Which agent, in the record's own order: a known selected agent, then
    // what the resolved executable turns out to be. A plain fallback is not
    // called Claude merely because Claude has an account picker.
    let driving = automation
        .agent
        .as_deref()
        .and_then(agent_spec)
        .map(|spec| spec.id)
        .or_else(|| agent_for_program(&program));
    // Or NO shell at all, when the job asked to continue the one already
    // listening in that checkout. Normalised first — a `NewPerRun` checkout
    // was cut seconds ago and has never held an agent, so `reuse_normalized`
    // is what stops the record's flag from meaning "start one and call it
    // reuse". The daily job this exists for otherwise leaves thirty claude
    // tabs in one workspace by the end of the month.
    //
    // Nothing to continue is not a failure: the fall-through is the ordinary
    // spawn below, which is also the first run of every reusing job.
    if reuse_normalized(automation.workspace_mode, automation.reuse_session)
        && let Some(driving) = driving
        && let Some(alive) = live_session_of(&state, automation, &root, driving)
    {
        return remember_reused_run(&state, automation, &claim, at, &root, alive, driving)
            .map(Some);
    }
    // And HOW IT REPORTS BACK — assembled exactly as `launch_agent_tab` and
    // `resume_session` assemble it, because a run that cannot report is a run
    // nobody can see. This shell was opening with the account env alone: no
    // pane key, no nonce, no bridge coordinates, so the agent inside it never
    // reached `hook_loop` — which meant no board card ever left `idle`, and
    // the bell that exists to fetch a person to a waiting agent could not ring
    // for the one class of run nobody is sitting in front of. The terminal id
    // and the nonce are taken BEFORE the spawn for the reason they are there:
    // a child cannot be handed a key minted after it exists.
    let term = state.take_term_id();
    let launch_token = new_launch_token(term);
    let mut env = launch_env;
    let mut _auth_launch_lock = None;
    let requested_env = match driving {
        Some(driving) => account_env_for(state.config_root(), driving),
        None => shell_account_env(state.config_root()),
    };
    let account_env = match requested_env {
        Ok(env) => env,
        Err(error) => {
            let (surviving_worktree, cleanup_error) = match (&made_by, &made) {
                (Some(orchestrator), Some(path)) => {
                    cleanup_failed_automation_worktree(state.local_data_root(), orchestrator, path)
                }
                _ => (None, None),
            };
            return Err(remember_automation_failure(
                &state,
                automation,
                &claim,
                at,
                AutomationFailureInput::new(AutomationFailureStage::Spawn, error, root.clone())
                    .with_cleanup(surviving_worktree, cleanup_error),
            ));
        }
    };
    env.extend(account_env);
    if let Some(driving) = driving {
        let (agent_env, auth_launch_lock) =
            hooks::agent_launch_env_with_lock(state.local_data_root(), driving);
        env.extend(agent_env);
        _auth_launch_lock = auth_launch_lock;
    }
    // The run's evidence folder, made before the shell exists so the agent
    // can be told where it is — in its environment always, and in its prompt
    // when an agent is reading; a custom command's shell is not told twice.
    let at_epoch_ms = now_epoch_ms();
    let evidence = leaves_evidence(automation)
        .then(|| make_run_evidence(state.local_data_root(), &automation.id, at_epoch_ms))
        .flatten();
    if let Some(dir) = &evidence {
        env.push((RUN_EVIDENCE_DIR_ENV.to_string(), dir.clone()));
        // The folder is an artifact source from the moment it exists; the
        // catalog walks it when its stamp moves.
        artifact_runtime::adopt_evidence(dir, &automation.id, root.to_str());
    }
    let inherited_path = pty_path_in(&env).map(str::to_string);
    env.extend(hooks::pty_env(
        &hooks::pane_key_of(term),
        Some(&launch_token),
        &root,
        inherited_path.as_deref(),
    ));
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
            note_window_event(
                state.local_data_root(),
                &format!("term {term} refused {program}: {error}"),
            );
            let (surviving_worktree, cleanup_error) = match (&made_by, &made) {
                (Some(orchestrator), Some(path)) => {
                    cleanup_failed_automation_worktree(state.local_data_root(), orchestrator, path)
                }
                _ => (None, None),
            };
            return Err(remember_automation_failure(
                &state,
                automation,
                &claim,
                at,
                AutomationFailureInput::new(
                    AutomationFailureStage::Spawn,
                    error.to_string(),
                    root.clone(),
                )
                .with_cleanup(surviving_worktree, cleanup_error),
            ));
        }
    };
    note_window_event(
        state.local_data_root(),
        &format!("term {term} spawned {program} {args:?}"),
    );
    state.hold_terminal(term, pty);
    let launch = crate::cmd::terminal::launch_fingerprint(&launch_token);
    state.launch_tokens().insert(term, launch_token);
    // The record says which agent this run drives; the terminal it opened is
    // therefore a send target like any other agent terminal.
    if let Some(driving) = driving {
        state.agent_terms().insert(term, driving);
    }
    // Its own delivery, waiting on the agent the same way a person's prompt
    // does. Nothing about being scheduled makes an agent listen any sooner.
    state.deliveries().insert(
        term,
        PromptDelivery::with_deadlines(
            prompt_with_evidence(
                &automation.prompt,
                evidence.as_deref().filter(|_| driving.is_some()),
            ),
            true,
            ready_signal_for(driving),
            Instant::now(),
            ready_quiet_for(driving),
            ready_timeout_for(driving),
        )
        .clearing(false)
        .guarded(zerocode_pty::ready::Guard::for_its_own_line(Some(launch))),
    );
    state.cadence().wake();
    // And the run itself, written down. `last_run_at` above says only WHEN the
    // most recent one was; a job that has fired forty times and one that fired
    // once an hour ago are the same record without this. The path it cut is
    // recorded here too, and that is the only place provenance lives — a
    // second store beside the worktree record would be a second thing to get
    // wrong about the same fact.
    //
    // The ledger is written with `let _`: the run already happened, the shell
    // is already open, and failing the start because a history file could not
    // be written would throw away the thing that worked to report on the thing
    // that did not.
    let _ = remember_run(
        state.config_root(),
        state.local_data_root(),
        automation,
        &claim,
        at,
        AutomationRun {
            id: format!("{at_epoch_ms}-{term}"),
            automation_id: automation.id.clone(),
            scheduled_for_epoch_minutes: Some(at.epoch_minutes),
            at_epoch_ms,
            root: root.display().to_string(),
            term,
            made_worktree: made.as_ref().map(|path| path.display().to_string()),
            skipped: None,
            failure: None,
            ended_at_epoch_ms: None,
            exit_code: None,
            completed_at_epoch_ms: None,
            result: None,
            evidence_dir: evidence,
            launch: Some(launch),
            unwitnessed: false,
        },
    );
    Ok(Some(StartedRun {
        term,
        name: automation.name.clone(),
        root: root.display().to_string(),
        worktree: made.map(|path| path.display().to_string()),
    }))
}

/// The shell an earlier run of this job left listening in that checkout, if
/// one is still there and free to be typed at.
///
/// **The ledger answers where each shell is.** This process holds no map from
/// a terminal to its checkout — the window does, in its tabs — but the run
/// history already records the root of every firing beside the term it opened
/// (1-da), and reusing that is cheaper and truer than a second store of the
/// same fact.
///
/// Four questions, and a `None` from any of them means spawn instead:
///
///   - is the shell still alive (`terminals`),
///   - is it running the agent this job drives (`agent_terms`) — a checkout
///     whose claude was replaced by a plain shell is not a session,
///   - has that agent told us its conversation id (`pane_sessions`), which is
///     the difference between a session to continue and a process that
///     happens to be running,
///   - and is nothing already on its way in (`deliveries`). Two bracketed
///     envelopes interleaved in one pty read as a single corrupt paste, so
///     this falls back to a shell of its own rather than reordering
///     somebody's instructions — the same answer `send_prompt` gives.
///
/// The four are read one at a time, never held together: see the note in the
/// body for the deadlock that holding them would make reachable.
pub(super) fn live_session_of(
    state: &AppState,
    automation: &Automation,
    root: &Path,
    driving: &str,
) -> Option<TermId> {
    let here = root.display().to_string();
    // Read one at a time rather than held together. This window has no
    // lock-order rule between these four, and it already takes two of them
    // both ways round: the pump holds `terminals` and then reaches for the
    // agent map, while the `agent_terms` command holds the agent map and then
    // reaches for the terminals. Four small copies cost nothing beside the
    // deadlock between a sweep and an open send menu that holding them all at
    // once would make reachable.
    let alive: Vec<TermId> = state.terminals().terms();
    let agents = state.agent_terms().clone();
    let sessions: Vec<TermId> = state.pane_sessions().keys().copied().collect();
    let busy: Vec<TermId> = state.deliveries().keys().copied().collect();
    // Newest first: a job that has been reusing for a month has many rows
    // naming the same checkout, and the one still on screen is the last.
    stored_automation_runs(state.local_data_root())
        .iter()
        .rev()
        .filter(|run| {
            run.automation_id == automation.id && run.skipped.is_none() && run.failure.is_none()
        })
        .filter(|run| run.root == here)
        .map(|run| run.term)
        .find(|term| {
            alive.contains(term)
                && agents.get(term) == Some(&driving)
                && sessions.contains(term)
                && !busy.contains(term)
        })
}

/// Hand this firing's prompt to a shell that is already open, and write the
/// run down as the run it is.
///
/// No terminal is taken, no pty is spawned, no launch nonce is minted: the
/// agent in there was launched once and reported itself once, and doing any of
/// that again would hand a running child a key it has never seen. What this
/// adds is the delivery — the same delivery a person's prompt gets, waiting on
/// the same signal, because nothing about being scheduled makes an agent
/// listen any sooner.
pub(super) fn remember_reused_run(
    state: &AppState,
    automation: &Automation,
    claim: &AutomationRunClaim,
    at: LocalMinute,
    root: &Path,
    term: TermId,
    driving: &str,
) -> Result<StartedRun, String> {
    // A continued session cannot be handed an environment variable, so the
    // prompt line is the only way it learns where this run's evidence goes.
    let at_epoch_ms = now_epoch_ms();
    let evidence = leaves_evidence(automation)
        .then(|| make_run_evidence(state.local_data_root(), &automation.id, at_epoch_ms))
        .flatten();
    if let Some(dir) = &evidence {
        artifact_runtime::adopt_evidence(dir, &automation.id, root.to_str());
    }
    let launch = crate::cmd::terminal::launch_of(state, term);
    state.deliveries().insert(
        term,
        reused_run_delivery(
            prompt_with_evidence(&automation.prompt, evidence.as_deref()),
            driving,
            launch,
            Instant::now(),
        ),
    );
    state.cadence().wake();
    remember_run(
        state.config_root(),
        state.local_data_root(),
        automation,
        claim,
        at,
        AutomationRun {
            id: format!("{at_epoch_ms}-{term}"),
            automation_id: automation.id.clone(),
            scheduled_for_epoch_minutes: Some(at.epoch_minutes),
            at_epoch_ms,
            root: root.display().to_string(),
            term,
            // It cut nothing — that is what continuing a session means.
            made_worktree: None,
            skipped: None,
            failure: None,
            // The shell it joined is still open, and the pump will stamp this
            // row when that shell exits, the same as any other run's.
            ended_at_epoch_ms: None,
            exit_code: None,
            completed_at_epoch_ms: None,
            result: None,
            evidence_dir: evidence,
            launch,
            unwitnessed: false,
        },
    )?;
    Ok(StartedRun {
        term,
        name: automation.name.clone(),
        root: root.display().to_string(),
        worktree: None,
    })
}

/// The delivery a reused run hands its prompt to the running agent with: a
/// send to a composer that is already up, so the door a person's send to that
/// pane takes — rest, its clear keys, its guard, its clocks.
///
/// It waited on the agent's LAUNCH announcement until t-4530, and an idle
/// composer never announces itself again: codex's `›` is not redrawn, nor is
/// Claude Code's `❯` (41.6 s of recorded idle, 2026-09-17), so a glyph agent's
/// reused run starved to its deadline, and agy's startup quiet — four seconds
/// for a sign-in gate at launch — held back a send to an agy long past it.
pub(super) fn reused_run_delivery(
    text: String,
    driving: &str,
    launch: Option<u64>,
    now: Instant,
) -> PromptDelivery {
    crate::cmd::terminal::prompt_delivery_for(
        text,
        true,
        Some(driving),
        crate::cmd::terminal::PromptReadiness::Resting,
        launch,
        now,
    )
}

/// The history row for a firing the precheck stopped.
///
/// `term` is `FLOAT_TERM` because no shell was opened and there is no honest
/// id to give; the row is told apart by `skipped` carrying the reason, and
/// every reader asks that first.
pub(super) fn skipped_run(
    automation: &Automation,
    ran: &script::PrecheckRun,
    at: LocalMinute,
) -> AutomationRun {
    let at_epoch_ms = now_epoch_ms();
    AutomationRun {
        id: format!("{at_epoch_ms}-skipped"),
        automation_id: automation.id.clone(),
        scheduled_for_epoch_minutes: Some(at.epoch_minutes),
        at_epoch_ms,
        root: automation.workspace.clone(),
        term: FLOAT_TERM,
        made_worktree: None,
        skipped: Some(zerocode_core::PrecheckRecord {
            command: automation.precheck.clone(),
            exit_code: ran.outcome.exit_code,
            timed_out: ran.outcome.timed_out,
            duration_ms: ran.duration_ms,
            output: ran.output.clone(),
        }),
        failure: None,
        // No shell was opened, so none of it ended. The row is already final
        // — `skipped` is what says so — and a second way to say the same
        // thing is a second thing to disagree with it.
        ended_at_epoch_ms: None,
        exit_code: None,
        completed_at_epoch_ms: None,
        result: None,
        evidence_dir: None,
        launch: None,
        unwitnessed: false,
    }
}

/// Spend the schedule on this firing and put it in the history.
///
/// The two roads that open no shell of their own come through here. The
/// started road keeps its own copy inline, where two gates pin it whole — the
/// one road that must not be able to lose the ledger by refactoring.
///
/// **The stamp happens even when nothing ran.** A precheck that says no has
/// still consumed the 9am firing: leaving `last_run_at` alone means the sweep
/// finds the job due again next minute, and a job whose precheck stays red
/// spawns that command once a minute all day.
pub(super) fn remember_run(
    config_root: &Path,
    local_data_root: &Path,
    automation: &Automation,
    claim: &AutomationRunClaim,
    at: LocalMinute,
    run: AutomationRun,
) -> Result<(), String> {
    let domain = automation_store_domain();
    if claim.automation_id != automation.id {
        return Err("자동화 실행 claim이 다른 정의를 가리킵니다".to_string());
    }
    if domain.incarnations.get(&automation.id).copied() != Some(claim.incarnation) {
        // The definition was deleted (and may have been recreated with the
        // same id) while precheck/git/spawn ran. Deletion owns its history, so
        // the stale completion must not attach itself to the new incarnation.
        return Ok(());
    }

    let mut rows = stored_automations(config_root);
    let Some(held) = rows.iter_mut().find(|held| held.id == automation.id) else {
        // An external writer is outside the in-process lock domain, but a
        // missing definition is still a tombstone we must respect.
        return Ok(());
    };
    held.last_run_at = Some(at.epoch_minutes);

    // Ledger first: it carries the occurrence identity, so it can fence a
    // retry even if the smaller schedule document cannot be stamped. Both
    // writes are attempted either way; one durable fence is better than none.
    let mut history = stored_automation_runs(local_data_root);
    let forgotten = record_run(&mut history, run, RUNS_KEPT_PER_AUTOMATION);
    let history_result = write_automation_runs(local_data_root, &history);
    if history_result.is_ok() {
        forget_run_evidence(local_data_root, &forgotten);
        // And the catalog forgets the rows those folders held.
        artifact_runtime::forget_evidence(
            &forgotten
                .iter()
                .filter_map(|run| run.evidence_dir.as_deref().map(PathBuf::from))
                .collect::<Vec<_>>(),
        );
    }
    let schedule_result = write_automations(config_root, &rows);

    match (history_result, schedule_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(history), Ok(())) => Err(format!("실행 이력을 저장할 수 없습니다: {history}")),
        (Ok(()), Err(schedule)) => Err(format!("일정 상태를 저장할 수 없습니다: {schedule}")),
        (Err(history), Err(schedule)) => Err(format!(
            "실행 이력과 일정 상태를 저장할 수 없습니다: {history}; {schedule}"
        )),
    }
}

/// Every evidence folder the run ledger still names, registered with the
/// artifact catalog at boot — one registration call per row, nothing read.
pub(super) fn adopt_stored_evidence(local_data_root: &Path) {
    for run in stored_automation_runs(local_data_root) {
        if let Some(dir) = run.evidence_dir.as_deref() {
            artifact_runtime::adopt_evidence(
                dir,
                &run.automation_id,
                run.made_worktree.as_deref().or(Some(run.root.as_str())),
            );
        }
    }
}

/// Fire whatever is due, and make up one missed run apiece.
///
/// Called from the pump, which is already the window's one clock. It costs a
/// file read per turn, so it turns once a minute rather than at the display
/// rate — the schedule has minute resolution and nothing here can change in
/// less than that.
/// The first tick of this window closes every run the LAST window left open.
///
/// A restart ends every pane's child while nobody is listening, so the exit
/// that stamps a row never arrives and the row reads "running" for good. Rows
/// whose terminal this window holds a launch token for are its own — a job
/// fired by hand before the first tick is a live run — and are left alone.
/// Once per process: after the first tick, endings are witnessed the ordinary
/// way, and a second sweep could only close a run whose shell is still there.
fn close_runs_nobody_saw_end(state: &AppState) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SWEPT: AtomicBool = AtomicBool::new(false);
    if SWEPT.swap(true, Ordering::AcqRel) {
        return;
    }
    let local_data_root = state.local_data_root().to_path_buf();
    let _store = automation_store_domain();
    let mut history = stored_automation_runs(&local_data_root);
    let held = state.launch_tokens();
    let closed = close_unwitnessed(&mut history, now_epoch_ms(), |term| {
        held.contains_key(&term)
    });
    drop(held);
    if closed > 0 {
        note_window_event(
            &local_data_root,
            &format!("automation sweep closed {closed} run(s) nobody saw end"),
        );
        let forgotten = prune_final_runs(&mut history, RUNS_KEPT_PER_AUTOMATION);
        if write_automation_runs(&local_data_root, &history).is_ok() {
            forget_run_evidence(&local_data_root, &forgotten);
        }
    }
}

pub(super) fn tick_automations(app: &AppHandle, now: LocalMinute) {
    let state = app.state::<AppState>();
    close_runs_nobody_saw_end(&state);
    let history = stored_automation_runs(state.local_data_root());
    for automation in stored_automations(state.config_root()) {
        let due = if automation.is_due(now) {
            Some(now)
        } else {
            automation.missed_run(now)
        };
        let Some(at) = due else {
            continue;
        };
        if occurrence_recorded(&history, &automation.id, at) {
            continue;
        }
        match start_automation(app, &automation, at, RunTrigger::Scheduled) {
            Ok(Some(started)) => {
                let _ = app.emit(
                    "automation:started",
                    AutomationStarted {
                        id: automation.id.clone(),
                        run: started,
                    },
                );
            }
            // The precheck said no. This is not an alarm, but it is an event:
            // the list and open history need a refresh to show the durable
            // skip row that consumed this occurrence.
            Ok(None) => {
                let _ = app.emit(
                    "automation:skipped",
                    AutomationSkipped {
                        id: automation.id.clone(),
                    },
                );
            }
            // Said out loud rather than swallowed: a job that cannot start —
            // its checkout is gone, its command does not exist — is one the
            // person needs told about, and it will otherwise fail silently
            // every night forever.
            Err(error) => {
                eprintln!("자동화 {}: {error}", automation.name);
                let _ = app.emit(
                    "automation:failed",
                    AutomationFailed {
                        id: automation.id.clone(),
                        error,
                    },
                );
            }
        }
    }
}

/// Switch one on or off without touching anything else about it.
pub(super) fn set_automation_enabled_in(
    config_root: &Path,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let mut domain = automation_store_domain();
    let mut rows = stored_automations(config_root);
    let Some(held) = rows.iter_mut().find(|held| held.id == id) else {
        return Err(format!("자동화 {id}을(를) 찾지 못했습니다"));
    };
    held.enabled = enabled;
    write_automations(config_root, &rows)?;
    domain.current_or_mint(id);
    Ok(())
}
