//! Review commands.

use crate::*;

/// Open one already-cataloged local workspace in its configured application,
/// or reveal it in the platform file manager when no application id is sent.
///
/// The renderer sends only an id. The current settings document supplies the
/// command, and the project/worktree catalog supplies the directory, so stale
/// menu data cannot become either executable text or an arbitrary disk path.
#[tauri::command(async)]
pub(crate) fn open_workspace_in_application(
    state: State<'_, AppState>,
    path: String,
    application_id: Option<String>,
) -> Result<(), String> {
    let workspace = match known_workspace_context(state.config_root(), &path)? {
        KnownWorkspace::Git(_, worktree) => worktree.path,
        KnownWorkspace::Folder(folder) => folder,
    }
    .canonicalize()
    .map_err(|_| "워크스페이스 디렉터리를 찾을 수 없습니다".to_string())?;

    let Some(application_id) = application_id else {
        return open_workspace_in_file_manager(&workspace);
    };
    spawn_open_in_application(state.settings(), &application_id, &workspace)
}

/// Open one FILE in a configured Open-in application — the SCM row menu's
/// Open-in entries (Orca's `entry-context-menu.tsx:116-154`, which hands the
/// row's absolute path to the same `openWorktreePath` machinery its
/// workspace menu uses).
///
/// A different door from the workspace one because the fences differ: that
/// one demands a CATALOGED WORKSPACE ROOT — a file path shown to it is
/// refused as unregistered — while this one demands a path inside the active
/// root, which is `fenced_path`'s contract and exactly what the row's
/// relative path is. There is no id-less file-manager arm here: revealing a
/// file is `fs_reveal`'s job and the menu already has that row.
#[tauri::command(async)]
pub(crate) fn open_path_in_application(
    state: State<'_, AppState>,
    path: String,
    application_id: String,
) -> Result<(), String> {
    let seat = fenced_path(&state, &path)?;
    if !seat.exists() {
        return Err("그 경로가 없습니다".into());
    }
    spawn_open_in_application(state.settings(), &application_id, &seat)
}

#[tauri::command(async)]
pub(crate) fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("웹 주소만 열 수 있습니다".into());
    }
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    let mut opener = crate::proc::quiet_command(launcher);
    opener.arg(&url);
    // Through the reaping door: the opener exits in milliseconds, and a
    // dropped handle would leave that exit standing in the process table for
    // the rest of the window's life (zerocode_core::reap).
    zerocode_core::reap::spawn_forgotten(opener)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The checks on the active checkout's pull request.
///
/// Two `gh` invocations deep at minimum (`pr view`, then the commit's checks),
/// so it rides the blocking pool rather than the window's thread — a panel
/// that opens should not wait on a network round trip to paint its head.
///
/// Two things ride along since t-2733. The stack the review is on, read off
/// the open reviews (one cached page) and ordered by the pure function. And
/// the observation: what CI says is remembered per checkout, and when it
/// DIFFERS from the last look the agents seated in that checkout are mailed
/// one `status` line through the ledger — only then, because a panel
/// repainting every four seconds must not become four-second mail.
#[tauri::command]
pub(crate) async fn pr_checks(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ChecksReport, String> {
    let root = state.active().root;
    let limits = checks_runtime::limits(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let review = match gh::fetch_pull_request(&root) {
            Ok(Some(review)) => review,
            // No PR is not an error: most branches do not have one, and the
            // panel says that in words.
            Ok(None) => {
                return ChecksReport {
                    review: None,
                    checks: Vec::new(),
                    error: None,
                    detail: None,
                    at_page_limit: false,
                    stack: None,
                };
            }
            Err(error) => return ChecksReport::refused(error),
        };
        let stack = checks_runtime::stack_for(&root, &review, &limits);
        // A PR whose head commit is unknown has nothing to ask about; the head
        // is what checks hang from, not the PR number.
        if review.head_sha.is_empty() {
            return ChecksReport {
                review: Some(review),
                checks: Vec::new(),
                error: None,
                detail: None,
                at_page_limit: false,
                stack,
            };
        }
        let mut client = gh::observer::Client::default();
        let mut budget = gh::observer::Budget::new(&limits);
        match gh::fetch_checks(&root, &review.owner_repo, &review.head_sha) {
            Ok(checks) => {
                let mut ci = zerocode_core::scm_observer::Ci {
                    head: review.head_sha.clone(),
                    checks,
                    detail: String::new(),
                };
                if client
                    .detail(&root, &review, &mut ci, false, &mut budget)
                    .is_ok()
                {
                    let _ = checks_runtime::note_checks(&app, &root, &review, &ci, &limits);
                }
                let checks = ci.checks;
                ChecksReport {
                    at_page_limit: checks.len() >= gh::CHECKS_PER_PAGE,
                    review: Some(review),
                    checks,
                    error: None,
                    detail: None,
                    stack,
                }
            }
            Err(error) => ChecksReport {
                review: Some(review),
                stack,
                ..ChecksReport::refused(error)
            },
        }
    })
    .await
    .map_err(|error| error.to_string())
}

/// One row's own page, fetched when it is opened.
///
/// The ids come from the row the window already has rather than being looked
/// up again: a check run knows its own id, and re-resolving the PR to find it
/// would spend a request to learn something already on screen.
#[tauri::command]
pub(crate) async fn pr_check_details(
    state: State<'_, AppState>,
    owner_repo: String,
    name: String,
    check_run_id: Option<u64>,
    workflow_run_id: Option<u64>,
) -> Result<CheckDetails, String> {
    let root = state.active().root;
    tauri::async_runtime::spawn_blocking(move || {
        gh::fetch_check_details(&root, &owner_repo, &name, check_run_id, workflow_run_id).map_err(
            |error| match error.detail() {
                Some(detail) => format!("{}: {detail}", error.reason()),
                None => error.reason().to_string(),
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The reviews the board's checkouts are on.
///
/// A checkout with no pull request is simply absent from the answer, and so is
/// a machine without `gh`: this is a dress on a card, and a board is not the
/// surface to explain a git remote.
#[tauri::command]
pub(crate) async fn github_review_states(worktrees: Vec<String>) -> Vec<ReviewState> {
    // Every row is a process spawn at worst, so this never runs on the thread
    // the window is waiting to paint on.
    tauri::async_runtime::spawn_blocking(move || review_states(&worktrees))
        .await
        .unwrap_or_default()
}

/// The notes of one checkout, in the order they were written.
#[tauri::command(async)]
pub(crate) fn list_diff_notes(state: State<'_, AppState>, workspace: String) -> Vec<DiffNote> {
    stored_diff_notes(state.config_root())
        .into_iter()
        .filter(|note| note.workspace == workspace)
        .collect()
}

#[tauri::command(async)]
pub(crate) fn save_diff_note(state: State<'_, AppState>, note: DiffNote) -> Result<(), String> {
    validate_diff_note(&note)?;
    let mut rows = stored_diff_notes(state.config_root());
    match rows.iter_mut().find(|held| held.id == note.id) {
        Some(held) => *held = note,
        None => rows.push(note),
    }
    write_diff_notes(state.config_root(), &rows)
}

/// Remove a note because the person said so.
#[tauri::command(async)]
pub(crate) fn delete_diff_note(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut rows = stored_diff_notes(state.config_root());
    rows.retain(|held| held.id != id);
    write_diff_notes(state.config_root(), &rows)
}

/// Sweep a checkout's notes — all of them, or one file's.
///
/// The shelf's two clear verbs (`PendingDiffCommentsClear`: `{kind: 'all'}`
/// and `{kind: 'file'}`) are one door because they are one question with a
/// narrower answer. Scoped to the WORKSPACE always: notes are the checkout's,
/// and "clear all" from one workspace must never reach another's.
#[tauri::command(async)]
pub(crate) fn clear_diff_notes(
    state: State<'_, AppState>,
    workspace: String,
    file_path: Option<String>,
) -> Result<(), String> {
    let mut rows = stored_diff_notes(state.config_root());
    rows.retain(|held| {
        held.workspace != workspace
            || file_path
                .as_deref()
                .is_some_and(|path| held.file_path != path)
    });
    write_diff_notes(state.config_root(), &rows)
}

/// Remove notes because they were delivered — but only the ones still saying
/// what was sent.
///
/// Orca's `clearDeliveredDiffComments` matches each delivered note against a
/// snapshot taken at send time (`deliverySnapshotMatches`) before removing
/// it: a note edited while the send was in flight is a NEW instruction the
/// agent has not seen, and deleting it would silently drop the edit.
#[tauri::command(async)]
pub(crate) fn clear_delivered_diff_notes(
    state: State<'_, AppState>,
    delivered: Vec<DiffNote>,
) -> Result<(), String> {
    if delivered.is_empty() {
        return Ok(());
    }
    let mut rows = stored_diff_notes(state.config_root());
    rows.retain(|held| {
        !delivered
            .iter()
            .any(|sent| sent.id == held.id && sent.body == held.body)
    });
    write_diff_notes(state.config_root(), &rows)
}

/// Every scheduled job, with the next time each is due.
#[tauri::command(async)]
pub(crate) fn list_automations(state: State<'_, AppState>) -> Vec<AutomationRow> {
    let now = local_minute_now();
    // Read once for the whole list rather than per row: the ledger holds every
    // job's runs in one file, so asking per automation would reread and
    // reparse it once per row for an answer that cannot change between them.
    let history = stored_automation_runs(state.local_data_root());
    stored_automations(state.config_root())
        .into_iter()
        .map(|automation| {
            let next = automation
                .next_unconsumed_run(now)
                .map(|minute| minute.epoch_minutes);
            // A gap row stands for nights it did not run.
            let runs = history
                .iter()
                .filter(|run| run.automation_id == automation.id && run.missed.is_none())
                .count();
            // The newest run that started, judged by what its folder holds.
            let last_verdict = history
                .iter()
                .rev()
                .find(|run| {
                    run.automation_id == automation.id
                        && run.missed.is_none()
                        && run.skipped.is_none()
                })
                .and_then(|run| run.evidence_dir.as_deref())
                .and_then(|dir| computer_use::evidence::read_verdict(Path::new(dir)))
                .map(|(pass, _)| pass);
            // A job an agent asked for waits once its Flow reads differently
            // from the one the person allowed. One reading answers that and
            // what 「다시 승인」 will ask about: the row carries the hash of
            // the very text it judged, whether that text may spend, and which
            // document it is.
            let standing = automation
                .asked
                .as_ref()
                .and(automation.flow.as_deref())
                .and_then(|slug| flow_standing(state.local_data_root(), slug));
            let flow_changed = automation
                .flow_changed_since_approval(standing.as_ref().map(|one| one.sha256.as_str()));
            let waits_on = standing.filter(|_| flow_changed);
            AutomationRow {
                cron: automation.schedule.to_cron(),
                leaves_evidence: zerocode_core::leaves_evidence(&automation),
                next_run_at: next,
                runs,
                last_verdict,
                flow_changed,
                flow_money: waits_on.as_ref().is_some_and(|one| one.money),
                flow_about: waits_on.as_ref().map(|one| one.about.clone()),
                flow_stands: waits_on.map(|one| one.sha256),
                automation,
            }
        })
        .collect()
}

/// Add a job or replace one, by id — through [`save_automation_in`], the
/// door every writer of a job uses.
///
/// # Errors
///
/// When the schedule does not parse. Refused at the door rather than stored:
/// an automation whose cron is nonsense is one that will never fire, and the
/// moment to say so is while the person is still looking at the editor.
#[tauri::command(async)]
pub(crate) fn save_automation(
    state: State<'_, AppState>,
    automation: Automation,
) -> Result<Vec<AutomationRow>, String> {
    // What the page says of an approval is never read: the door decides what
    // of the stored job's approval this save keeps (`Approval::FromForm`).
    let root = state.local_data_root();
    let standing = |slug: &str| flow_standing(root, slug);
    save_automation_in(
        state.config_root(),
        automation,
        Approval::FromForm(&standing),
    )?;
    Ok(list_automations(state))
}

/// What a save does with the approval a job carries (`Automation::asked`).
pub(crate) enum Approval<'a> {
    /// The approval is what the writer wrote: the window's card writing the
    /// job the person allowed, or a job the window keeps for itself.
    AsWritten,
    /// A save from the form. What the page says of an approval is never read:
    /// [`approval_a_form_save_keeps`] answers from the stored job, reading
    /// its Flow's document through this (`flow_standing`).
    FromForm(&'a dyn Fn(&str) -> Option<computer_use::recipes::schedule::Standing>),
}

/// What a waiting job's save is refused with when it would stop being a job
/// that walks a Flow ([`approval_a_form_save_keeps`]).
pub(crate) const WAITING_JOB_STAYS_A_FLOW: &str = "다시 승인을 기다리는 예약은 Flow를 걷는 예약으로만 저장할 수 있습니다 — 먼저 「다시 승인」을 누르거나, 지우고 새로 만들어 주세요";

/// What of the stored job's approval a save from the form keeps.
///
/// An approval binds a job to the document the person allowed: once that
/// document reads differently the job waits, and 「다시 승인」 asks. A save
/// that dropped the approval took the binding off without a word — a job
/// that waited walked its changed Flow at the next minute, and one that did
/// not wait walked whatever the document was rewritten to afterwards
/// (t-105044 알아 둘 것 3, t-105495 막지 않는 1). So:
///
/// - a job that WAITS keeps its approval through a save — its name, its
///   schedule, its switch and the Flow it names may change, and it still
///   waits; 「다시 승인」 alone releases it. Saved as another kind of job — an
///   agent, a command, no Flow — there would be nothing left to allow, and it
///   would wait for ever: that save is refused;
/// - a job that does not wait keeps its approval while it stays the job that
///   was allowed — the window walking that same Flow. Pointed at another
///   Flow, or made another kind of job, it is the person's own from then on:
///   they chose that in the form;
/// - a job the person wrote, and one the store does not hold, carry none.
///
/// `held` is the stored job and `saved` the job as the form sent it, its Flow
/// already in the store's spelling (`bind_flow`); what `saved` says of an
/// approval is never read. `standing` reads a Flow's document. Called under
/// the store's lock ([`save_automation_in`]), as 「다시 승인」 judges: between
/// a look outside it and the write, the document could change.
///
/// # Errors
///
/// When a waiting job would be saved as another kind of job.
pub(crate) fn approval_a_form_save_keeps(
    held: Option<&Automation>,
    saved: &Automation,
    standing: &dyn Fn(&str) -> Option<computer_use::recipes::schedule::Standing>,
) -> Result<Option<zerocode_core::automation::Asked>, String> {
    let Some((held, asked)) = held.and_then(|held| Some((held, held.asked.clone()?))) else {
        return Ok(None);
    };
    let stands = held.flow.as_deref().and_then(standing);
    let waits = held.flow_changed_since_approval(stands.as_ref().map(|one| one.sha256.as_str()));
    let walks_a_flow = saved.runs_flow_directly();
    if waits {
        return if walks_a_flow {
            Ok(Some(asked))
        } else {
            Err(WAITING_JOB_STAYS_A_FLOW.to_string())
        };
    }
    Ok((walks_a_flow && saved.flow == held.flow).then_some(asked))
}

/// 「다시 승인」: allow the Flow of a job an agent asked for, as the document
/// the person was asked about. The person's alone (`PERSON_ONLY_COMMANDS`): it
/// is the yes the card took, said again for a document that has changed since.
///
/// `seen` is the hash the list read for the row (`flow_stands`) and the
/// question was drawn from. A document that changed again while the question
/// stood is refused — as the card's own road refuses one that changed under
/// the card (`schedule::settle`) — and the page reads the list again.
///
/// # Errors
///
/// When the job is gone or was written by the person, when its Flow's document
/// cannot be read or is no Flow, or when it is not the document that was seen.
#[tauri::command(async)]
pub(crate) fn approve_automation_flow(
    state: State<'_, AppState>,
    id: String,
    seen: String,
) -> Result<Vec<AutomationRow>, String> {
    let root = state.local_data_root();
    approve_automation_flow_in(
        state.config_root(),
        &id,
        &seen,
        u64::try_from(now_epoch_ms()).unwrap_or_default(),
        |slug| flow_standing(root, slug),
    )?;
    Ok(list_automations(state))
}

/// A Flow's document as it stands under the window's data root — its hash and
/// what a question about it says — or `None` when it cannot be read or is no
/// Flow: what an approval is held against (`recipes::schedule`). The root is
/// the window's own (`AppState`), not the one the Computer Use door remembers
/// from its first request: a window just opened has answered none, and every
/// asked job read as changed until it did.
pub(crate) fn flow_standing(
    local_data_root: &Path,
    slug: &str,
) -> Option<computer_use::recipes::schedule::Standing> {
    computer_use::recipes::schedule::standing(local_data_root, slug)
}

/// The save behind [`save_automation`], for every writer of a job: the second
/// brain's weekly review is written through it too (t-77377), so it is
/// checked, armed and locked like one the person typed. `approval` says what
/// becomes of the approval the job carries ([`Approval`]).
pub(crate) fn save_automation_in(
    config_root: &Path,
    mut automation: Automation,
    approval: Approval<'_>,
) -> Result<(), String> {
    let cron = automation.schedule.to_cron();
    if Cron::parse(&cron).is_none() {
        return Err(format!("일정을 읽을 수 없습니다: {cron}"));
    }
    // A Flow job's prompt is the command that walks its recipe, written here so
    // the record never says one thing in its link and another in its prompt.
    automation.bind_flow()?;
    automation.validate_for_save()?;
    // Reuse is normalised on the way IN, not only on the way out — Orca forces
    // the same pair at both of its doors. A record that says a per-run
    // checkout will continue an existing session is a record that will read as
    // a bug forever afterwards: the form shows a switch that is on, and the
    // firing ignores it. Storing the truth means the two agree.
    automation.reuse_session =
        reuse_normalized(automation.workspace_mode, automation.reuse_session);
    // A save arms the job: what the schedule said before it is not something
    // this schedule can have missed.
    automation.armed_at = Some(local_minute_now().epoch_minutes);
    let automation_id = automation.id.clone();
    let mut domain = automation_store_domain();
    let mut rows = stored_automations(config_root);
    // What a form's save keeps of the stored job's approval is judged here,
    // under the store's lock, against the job as it is stored.
    if let Approval::FromForm(standing) = approval {
        let stored = rows.iter().find(|row| row.id == automation.id);
        automation.asked = approval_a_form_save_keeps(stored, &automation, standing)?;
    }
    // And which agent, on the same terms the editor offers: a first save may
    // only name one this build knows, while an edit keeps the unrecognised id
    // it was already carrying. The rule and its reasoning live in
    // `zerocode_core::automation::agent_may_be_saved`.
    let held = rows.iter().find(|row| row.id == automation.id);
    if !agent_may_be_saved(automation.agent.as_deref(), held) {
        return Err(format!(
            "이 창이 모르는 에이전트입니다: {}",
            automation.agent.as_deref().unwrap_or_default()
        ));
    }
    let replacing = rows.iter_mut().find(|held| held.id == automation.id);
    let is_new = replacing.is_none();
    match replacing {
        // The last run belongs to the schedule's history, not to the form that
        // was just submitted — editing a prompt must not make a job that ran
        // an hour ago eligible to run again immediately.
        Some(held) => {
            let ran = held.last_run_at;
            *held = automation;
            held.last_run_at = ran;
        }
        None => rows.push(automation),
    }
    write_automations(config_root, &rows)?;
    if is_new {
        domain.replace_incarnation(&automation_id);
    } else {
        domain.current_or_mint(&automation_id);
    }
    drop(domain);
    Ok(())
}

/// Take a job off the schedule for good, together with its run history.
#[tauri::command(async)]
pub(crate) fn delete_automation(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<AutomationRow>, String> {
    delete_automation_and_run_history(state.config_root(), state.local_data_root(), &id)?;
    Ok(list_automations(state))
}

/// Run one now, by hand, whatever its schedule says.
///
/// By hand is [`RunTrigger::Manual`], which is the whole of the precheck's
/// asymmetry: somebody pressing this has the screen in front of them and has
/// already decided, so the guard written for 3am does not get to argue.
#[tauri::command(async)]
pub(crate) fn run_automation(app: AppHandle, id: String) -> Result<StartedRun, String> {
    let automation = stored_automations(app.state::<AppState>().config_root())
        .into_iter()
        .find(|held| held.id == id)
        .ok_or_else(|| format!("자동화 {id}을(를) 찾지 못했습니다"))?;
    start_automation(&app, &automation, local_minute_now(), RunTrigger::Manual)?
        .ok_or_else(|| format!("자동화 {id}이(가) 시작되지 않았습니다"))
}

#[tauri::command(async)]
pub(crate) fn enable_automation(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<Vec<AutomationRow>, String> {
    set_automation_enabled_in(state.config_root(), &id, enabled)?;
    Ok(list_automations(state))
}

/// What one job has already done, most recent first.
///
/// Newest first because the list under an open form is read from the top and
/// the question it answers is "did the last one work" — a history that has to
/// be scrolled to its end to answer that is a history nobody reads. The ledger
/// itself stays oldest-first: it is an append-only log, and its order is what
/// makes the cap forget the right end.
#[tauri::command(async)]
pub(crate) fn list_automation_runs(
    state: State<'_, AppState>,
    automation_id: String,
) -> Vec<AutomationRunRow> {
    let mut mine: Vec<AutomationRun> = stored_automation_runs(state.local_data_root())
        .into_iter()
        .filter(|run| run.automation_id == automation_id)
        .collect();
    mine.reverse();
    mine.into_iter()
        .map(|run| {
            let dir = run.evidence_dir.as_deref().map(Path::new);
            let evidence_files = dir.map_or(0, evidence_file_count);
            let (verdict, report) = dir.map(folder_verdict).unwrap_or_default();
            AutomationRunRow {
                run,
                evidence_files,
                verdict,
                report,
            }
        })
        .collect()
}

/// One run as the runs tab reads it: the row, how many files its evidence
/// folder holds — the number that tells "완료" from "proved" — and, when the
/// folder holds them, the verdict of what ran and the report rendered from it.
#[derive(Serialize, Clone)]
pub(crate) struct AutomationRunRow {
    #[serde(flatten)]
    run: AutomationRun,
    evidence_files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict: Option<RunVerdict>,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<String>,
}

/// How a run went, in its folder's own words: whether it passed, and the
/// reason it gave when it did not.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunVerdict {
    pass: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// What a run's evidence folder says of how it went: the verdict a Flow's walk
/// — or a QA scenario — left in it, and the report rendered from the folder.
/// A folder that holds neither answers nothing, and the row then says only
/// that the run ended.
fn folder_verdict(dir: &Path) -> (Option<RunVerdict>, Option<String>) {
    let verdict =
        computer_use::evidence::read_verdict(dir).map(|(pass, reason)| RunVerdict { pass, reason });
    let report = dir.join(computer_use::report::REPORT_FILE);
    (
        verdict,
        report.is_file().then(|| report.display().to_string()),
    )
}

/// The files in an evidence folder that are evidence — the step log is the
/// index of them, not one of them.
fn evidence_file_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_file())
                    && entry.file_name() != crate::run_evidence::STEPS_FILE
            })
            .count()
    })
}

/// One file a run left in its evidence folder.
#[derive(Serialize, Clone)]
pub(crate) struct RunEvidenceFile {
    name: String,
    bytes: u64,
    /// `image` | `text` | `other`, by extension — what the row can preview.
    kind: &'static str,
    /// A `data:` URL for a small image, so the row can show it inline.
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
    /// A small text file's contents — the run's `report.md`, read in place.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

/// What a run's evidence folder holds: the files, by name, with inline
/// previews for small images. Capped so a folder of a thousand frames does
/// not become a thousand-row list; `more` says how many were left out.
#[derive(Serialize, Clone)]
pub(crate) struct RunEvidence {
    dir: String,
    files: Vec<RunEvidenceFile>,
    more: usize,
    /// The step log: what was done, in order, and which frame shows it.
    steps: Vec<crate::run_evidence::Step>,
}

/// How much of an evidence folder is carried inline — here in the run
/// evidence view, and in the folder's own `report.html`
/// (`computer_use::report`): how many files, how large a picture, how large
/// a text.
pub(crate) const EVIDENCE_FILES_SHOWN: usize = 24;
pub(crate) const EVIDENCE_PREVIEW_MAX_BYTES: u64 = 512 * 1024;
pub(crate) const EVIDENCE_TEXT_MAX_BYTES: u64 = 8 * 1024;

/// A picture as a page can show it without the file: a data URL.
pub(crate) fn image_data_url(name: &str, data: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{};base64,{}",
        image_mime(name),
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

fn evidence_kind(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        "image"
    } else if [".txt", ".md", ".log", ".json", ".csv"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        "text"
    } else {
        "other"
    }
}

fn image_mime(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

/// The evidence folder of one run, fenced to the automations tree under the
/// app's own data root — a row that named somewhere else lists nothing.
fn run_evidence_dir_of(
    state: &AppState,
    automation_id: &str,
    run_id: &str,
) -> Result<PathBuf, String> {
    let run = stored_automation_runs(state.local_data_root())
        .into_iter()
        .find(|run| run.automation_id == automation_id && run.id == run_id)
        .ok_or("그 실행이 없습니다")?;
    let dir = PathBuf::from(run.evidence_dir.ok_or("이 실행에는 증거 폴더가 없습니다")?);
    let fence = state.local_data_root().join("automations");
    if !dir.starts_with(&fence) {
        return Err("증거 폴더가 앱 데이터 밖을 가리킵니다".into());
    }
    Ok(dir)
}

/// List what a run left behind.
#[tauri::command(async)]
pub(crate) fn list_run_evidence(
    state: State<'_, AppState>,
    automation_id: String,
    run_id: String,
) -> Result<RunEvidence, String> {
    let dir = run_evidence_dir_of(&state, &automation_id, &run_id)?;
    let mut names: Vec<(String, u64)> = std::fs::read_dir(&dir)
        .map_err(|error| format!("증거 폴더를 읽을 수 없습니다: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            (meta.is_file() && name != crate::run_evidence::STEPS_FILE)
                .then_some((name, meta.len()))
        })
        .collect();
    names.sort();
    let more = names.len().saturating_sub(EVIDENCE_FILES_SHOWN);
    let files = names
        .into_iter()
        .take(EVIDENCE_FILES_SHOWN)
        .map(|(name, bytes)| {
            let kind = evidence_kind(&name);
            let preview = (kind == "image" && bytes <= EVIDENCE_PREVIEW_MAX_BYTES)
                .then(|| std::fs::read(dir.join(&name)).ok())
                .flatten()
                .map(|data| image_data_url(&name, &data));
            let text = (kind == "text" && bytes <= EVIDENCE_TEXT_MAX_BYTES)
                .then(|| std::fs::read_to_string(dir.join(&name)).ok())
                .flatten();
            RunEvidenceFile {
                name,
                bytes,
                kind,
                preview,
                text,
            }
        })
        .collect();
    Ok(RunEvidence {
        dir: dir.display().to_string(),
        files,
        more,
        steps: crate::run_evidence::steps_in(&dir),
    })
}

/// Show a run's evidence folder in the file manager.
#[tauri::command(async)]
pub(crate) fn reveal_run_evidence(
    state: State<'_, AppState>,
    automation_id: String,
    run_id: String,
) -> Result<(), String> {
    let dir = run_evidence_dir_of(&state, &automation_id, &run_id)?;
    #[cfg(target_os = "macos")]
    let status = crate::proc::quiet_command("open").arg(&dir).status();
    #[cfg(target_os = "linux")]
    let status = crate::proc::quiet_command("xdg-open").arg(&dir).status();
    #[cfg(target_os = "windows")]
    let status = crate::proc::quiet_command("explorer").arg(&dir).status();
    status
        .map_err(|error| format!("폴더를 열 수 없습니다: {error}"))
        .and_then(|status| {
            status
                .success()
                .then_some(())
                .ok_or_else(|| "폴더를 열 수 없습니다".to_string())
        })
}

/// Every checkout an automation cut, each named once.
///
/// The sidebar's hide filter asks this. There is no provenance store to
/// consult: a run that cut a worktree wrote the path down, so the ledger is
/// the answer — Orca keeps the same fact on the worktree record
/// (`automationProvenance.kind === "created-by-automation"`), which is a
/// second copy of something already known.
#[tauri::command(async)]
pub(crate) fn automation_born_worktrees(state: State<'_, AppState>) -> Vec<String> {
    born_worktrees(&stored_automation_runs(state.local_data_root()))
}

// ---- Flow cards (t-4260) ---------------------------------------------------
//
// The card is a thin surface over the recipe documents (core `computer_flow`,
// shell `computer_use::recipes`): a Flow is a recipe with `## Flow` and
// `## Checks`. These two doors read that folder and rewrite those two
// sections — they add no store and no scheduler of their own. Running a Flow
// stays the existing `recipe-run` road (the composer), and scheduling it stays
// the automations above; the roster only reads how a slug is triggered today.

/// One automation that runs a recipe: which job, the cron it does so on, and
/// whether it is switched on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecipeTrigger {
    id: String,
    name: String,
    cron: String,
    enabled: bool,
}

/// The recipe slugs a prompt walks by name (`recipe-run --name <name>`), each
/// once however often the prompt says it — how a job written before the
/// recorded link (`Automation::flow`) is still read. The words are split the way
/// a command line is, so a quoted name of several words is one name; a prompt
/// that does not read as a command line (an apostrophe in a sentence) falls back
/// to its plain words.
fn prompt_recipe_slugs(prompt: &str) -> Vec<String> {
    if !prompt.contains("recipe-run") {
        return Vec::new();
    }
    let words = zerocode_core::split_command_line(prompt)
        .unwrap_or_else(|_| prompt.split_whitespace().map(str::to_string).collect());
    let mut slugs: Vec<String> = Vec::new();
    for (at, word) in words.iter().enumerate() {
        let named = word.strip_prefix("--name=").or_else(|| {
            (word == "--name")
                .then(|| words.get(at + 1).map(String::as_str))
                .flatten()
        });
        let Some(named) = named else {
            continue;
        };
        let slug = zerocode_core::computer_use::recipe_slug(named.trim_matches(['"', '\'']));
        if !slug.is_empty() && !slugs.contains(&slug) {
            slugs.push(slug);
        }
    }
    slugs
}

/// Which recipe slugs the automations run, and by which jobs — read from the
/// automation surface, never a scheduler of the Flow page's own. A job's link is
/// the one its record keeps (`Automation::flow`); a job without one is read from
/// its prompt. Every job counts, switched on or off: the page lists a paused
/// schedule as paused, and deleting the Flow names every job and takes them
/// with it, so none is left pointing at a recipe that is gone.
fn automation_recipe_triggers(
    automations: &[Automation],
) -> std::collections::BTreeMap<String, Vec<RecipeTrigger>> {
    let mut triggers: std::collections::BTreeMap<String, Vec<RecipeTrigger>> =
        std::collections::BTreeMap::new();
    for automation in automations {
        let slugs = match automation.flow.as_deref() {
            Some(flow) => vec![zerocode_core::computer_use::recipe_slug(flow)],
            None => prompt_recipe_slugs(&automation.prompt),
        };
        let cron = automation.schedule.to_cron();
        for slug in slugs.into_iter().filter(|slug| !slug.is_empty()) {
            triggers.entry(slug).or_default().push(RecipeTrigger {
                id: automation.id.clone(),
                name: automation.name.clone(),
                cron: cron.clone(),
                enabled: automation.enabled,
            });
        }
    }
    triggers
}

/// How a slug is re-run today, as the Flow page draws it: the cron of the first
/// job that is switched on, and every job that runs it.
fn flow_trigger_of(
    triggers: &std::collections::BTreeMap<String, Vec<RecipeTrigger>>,
    slug: &str,
) -> Option<computer_use::recipes::FlowTrigger> {
    let jobs = triggers.get(slug)?;
    Some(computer_use::recipes::FlowTrigger {
        cron: jobs
            .iter()
            .find(|job| job.enabled)
            .map(|job| job.cron.clone()),
        automations: jobs.iter().map(|job| job.name.clone()).collect(),
        jobs: jobs
            .iter()
            .map(|job| computer_use::recipes::FlowJob {
                id: job.id.clone(),
                name: job.name.clone(),
                enabled: job.enabled,
            })
            .collect(),
    })
}

/// Build the roster once — the recipes that are Flows, their last verdicts, and
/// the closed sets the card's controls draw from (the core enums).
fn flow_listing(state: &AppState) -> computer_use::recipes::FlowListing {
    let triggers = automation_recipe_triggers(&stored_automations(state.config_root()));
    computer_use::recipes::flow_list(state.local_data_root(), |slug| {
        flow_trigger_of(&triggers, slug)
    })
}

/// The Flow roster: the recipes that are Flows, each with its last run's
/// verdict, and the policy and evidence sets the card's controls are drawn
/// from — the core enums, so the card never fixes a value of its own.
#[tauri::command(async)]
pub(crate) fn flow_list(state: State<'_, AppState>) -> computer_use::recipes::FlowListing {
    flow_listing(&state)
}

/// Change a Flow's policy and/or evidence level: the two Flow sections of the
/// recipe document are rewritten (its steps left alone) with one atomic write,
/// and the fresh roster is answered — the pattern `save_automation` uses. The
/// person's alone (`PERSON_ONLY_COMMANDS`): the policy decides whether a walk
/// rehearses its money step or presses it.
#[tauri::command(async)]
pub(crate) fn flow_set(
    state: State<'_, AppState>,
    slug: String,
    policy: Option<String>,
    evidence: Option<String>,
) -> Result<computer_use::recipes::FlowListing, String> {
    computer_use::recipes::flow_set(
        state.local_data_root(),
        &slug,
        policy.as_deref(),
        evidence.as_deref(),
    )
    .map_err(|error| error.message)?;
    Ok(flow_listing(&state))
}

/// What deleting a recipe from the card did: the roster after it, what the
/// recipe store took, and the automations that walked the recipe, named — they
/// went with it.
#[derive(serde::Serialize)]
pub(crate) struct FlowDeleteAnswer {
    listing: computer_use::recipes::FlowListing,
    deleted: computer_use::recipes::FlowDeleted,
    automations: Vec<String>,
}

/// The delete behind [`flow_delete`], on explicit roots so a test can stand on
/// folders it made: the recipe store takes the document and the walk records
/// that make up the last verdict, then each automation the roster showed as the
/// recipe's trigger goes with its run history. Answers what the store took and
/// the names of the automations that went.
fn delete_recipe_and_triggers(
    config_root: &Path,
    local_data_root: &Path,
    slug: &str,
) -> Result<(computer_use::recipes::FlowDeleted, Vec<String>), String> {
    let triggers = automation_recipe_triggers(&stored_automations(config_root));
    let jobs = triggers.get(slug).cloned().unwrap_or_default();
    let deleted =
        computer_use::recipes::flow_delete(local_data_root, slug).map_err(|error| error.message)?;
    let mut automations = Vec::new();
    for job in jobs {
        delete_automation_and_run_history(config_root, local_data_root, &job.id).map_err(
            |why| {
                format!(
                    "{slug} 문서는 지웠지만 자동화 {}은(는) 지우지 못했습니다: {why}",
                    job.name
                )
            },
        )?;
        automations.push(job.name);
    }
    Ok((deleted, automations))
}

/// Take a recipe away — a Flow, or one of the card's second list: its document,
/// the walk records that make up its last verdict, and the automations that
/// re-run it (the ones its row names, so the question the person answered named
/// them). The person's alone (`PERSON_ONLY_COMMANDS`): nothing here can be had
/// back. The slug is the recipe store's to check; nothing else names a file.
#[tauri::command(async)]
pub(crate) fn flow_delete(
    state: State<'_, AppState>,
    slug: String,
) -> Result<FlowDeleteAnswer, String> {
    let (deleted, automations) =
        delete_recipe_and_triggers(state.config_root(), state.local_data_root(), &slug)?;
    Ok(FlowDeleteAnswer {
        listing: flow_listing(&state),
        deleted,
        automations,
    })
}

/// What the window can see now, which names the identity a recipe's words stand
/// for: its browser panes, and the apps the helper lists (a helper that does not
/// answer leaves the words as they were typed).
fn seen_by_the_window(state: &AppState) -> computer_use::recipes::Seen {
    let pages = state
        .browser_urls()
        .iter()
        .map(|(label, url)| (label.clone(), url.clone()))
        .collect();
    let apps = computer_use::call("listApps", serde_json::json!({})).ok();
    computer_use::recipes::Seen::of(pages, apps.as_ref())
}

/// The roster after a recipe was registered, and what registering made of it.
#[derive(serde::Serialize)]
pub(crate) struct FlowRegisterAnswer {
    listing: computer_use::recipes::FlowListing,
    registered: computer_use::recipes::Registered,
}

/// Stand a plain recipe up as a Flow: its Flow and Checks sections are appended
/// to the document (its steps untouched) and the fresh roster is answered. The
/// person's alone (`PERSON_ONLY_COMMANDS`) when it is the card that asks; an
/// agent's road to the same thing is `recipe-save --flow-check`, made in the
/// session that walked the recipe.
#[tauri::command(async)]
pub(crate) fn flow_register(
    state: State<'_, AppState>,
    slug: String,
) -> Result<FlowRegisterAnswer, String> {
    let seen = seen_by_the_window(&state);
    let registered =
        computer_use::recipes::flow_register(state.local_data_root(), &slug, None, &seen)
            .map_err(|error| error.message)?;
    Ok(FlowRegisterAnswer {
        listing: flow_listing(&state),
        registered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, name: &str, prompt: &str, enabled: bool) -> Automation {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": name,
            "enabled": enabled,
            "workspace": "/tmp/work",
            "prompt": prompt,
        }))
        .expect("an automation reads with its defaults")
    }

    /// A run's row reads how it went from its own folder: nothing from a folder
    /// that holds no verdict, the verdict with its reason from one that does,
    /// and the report only when the file is there to open.
    #[test]
    fn a_runs_row_says_the_verdict_and_the_report_its_folder_holds() {
        let folder = tempfile::tempdir().expect("an evidence folder");
        assert_eq!(folder_verdict(folder.path()), (None, None));

        computer_use::evidence::write_verdict(
            folder.path(),
            &["verdict".to_string(), "--fail".to_string()],
            false,
            Some("the balance was not shown"),
            1_000,
        )
        .expect("a verdict is written");
        let (verdict, report) = folder_verdict(folder.path());
        assert_eq!(
            verdict,
            Some(RunVerdict {
                pass: false,
                reason: Some("the balance was not shown".to_string()),
            })
        );
        assert_eq!(report, None, "no report was rendered");

        let file = folder.path().join(computer_use::report::REPORT_FILE);
        std::fs::write(&file, "<!doctype html>").expect("a report");
        assert_eq!(
            folder_verdict(folder.path()).1,
            Some(file.display().to_string())
        );
    }

    /// The Flow page lists every automation that runs a slug — by the link its
    /// record keeps or, for an older job, by the name its prompt walks (once each
    /// however often the prompt says it, a quoted name of several words too) —
    /// switched on or off. Deleting the Flow takes exactly those, with their run
    /// history, and no automation that walks another recipe or walks nothing.
    #[test]
    fn deleting_a_flow_takes_the_automations_that_walk_it_and_no_other() {
        let config = tempfile::tempdir().expect("config root");
        let local = tempfile::tempdir().expect("data root");
        let recipes = local.path().join(computer_use::recipes::RECIPES_DIR);
        std::fs::create_dir_all(&recipes).expect("recipes folder");
        for slug in ["wallet-qa", "other"] {
            std::fs::write(recipes.join(format!("{slug}.md")), format!("# {slug}\n"))
                .expect("a recipe");
        }
        write_automations(
            config.path(),
            &[
                job(
                    "a",
                    "Morning wallet QA",
                    "Run `zerocode-computer recipe-run --name wallet-qa --json` and report.",
                    true,
                ),
                job(
                    "b",
                    "Twice over",
                    "recipe-run --name wallet-qa, then recipe-run --name=wallet-qa",
                    true,
                ),
                job("c", "Other recipe", "recipe-run --name other", true),
                job("d", "Switched off", "recipe-run --name wallet-qa", false),
                job("e", "Walks nothing", "summarise the week", true),
                Automation {
                    flow: Some("wallet-qa".to_string()),
                    ..job("f", "Recorded link", "says nothing of the recipe", true)
                },
                job("g", "Quoted name", r#"recipe-run --name "Wallet QA""#, true),
            ],
        )
        .expect("automation fixtures");

        let shown = flow_trigger_of(
            &automation_recipe_triggers(&stored_automations(config.path())),
            "wallet-qa",
        )
        .expect("the roster shows a trigger");
        let all = [
            "Morning wallet QA",
            "Twice over",
            "Switched off",
            "Recorded link",
            "Quoted name",
        ];
        assert_eq!(shown.automations, all);
        assert!(
            shown.cron.as_deref().is_some_and(|cron| !cron.is_empty()),
            "the chip is the cron of the first job that is switched on"
        );
        assert_eq!(
            shown
                .jobs
                .iter()
                .map(|job| (job.id.as_str(), job.enabled))
                .collect::<Vec<_>>(),
            [
                ("a", true),
                ("b", true),
                ("d", false),
                ("f", true),
                ("g", true)
            ],
            "each job is listed by its id, with whether it is on"
        );

        let (deleted, gone) =
            delete_recipe_and_triggers(config.path(), local.path(), "wallet-qa").expect("deleted");
        assert!(deleted.file.ends_with("wallet-qa.md"), "{deleted:?}");
        assert_eq!(gone, all, "the jobs the question named");
        assert!(!recipes.join("wallet-qa.md").exists());
        assert!(recipes.join("other.md").exists(), "another recipe stays");
        assert_eq!(
            stored_automations(config.path())
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            ["c", "e"],
            "only the jobs the page listed went"
        );

        // The next recipe has its own job and goes with it; a slug that is no slug is
        // refused before any automation is touched.
        let (_, other) =
            delete_recipe_and_triggers(config.path(), local.path(), "other").expect("deleted");
        assert_eq!(other, ["Other recipe"]);
        let refused = delete_recipe_and_triggers(config.path(), local.path(), "../other")
            .expect_err("a path is no slug");
        assert!(refused.contains("not a recipe slug"), "{refused}");
        assert_eq!(
            stored_automations(config.path())
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            ["e"],
            "a refused delete takes no automation"
        );
    }
}
