//! Repo policy commands.

use crate::*;

/// Does the active repository's own code have permission to run here?
///
/// Asked at ACTION TIME — creating a workspace, opening a repository's declared
/// tabs — and never when a project is merely opened. A person browsing a
/// repository has not asked it to do anything, and a dialog there is one they
/// answer to make it go away.
#[tauri::command(async)]
pub(crate) fn repo_trust_standing(
    state: State<'_, AppState>,
    project: Option<String>,
) -> RepoTrustReport {
    let (repo_root, checkout) = repo_trust_target(&state, project);
    let Some(content) = repo_trust_content(&checkout) else {
        // Unreadable: there is no parsed script and no parsed tab to run, so
        // there is nothing to put in front of anybody either. The scripts panel
        // is where an unreadable project file is reported.
        return RepoTrustReport {
            standing: "nothing",
            changed: false,
            content: String::new(),
        };
    };
    match repo_trust_standing_of(state.config_root(), &repo_root, &content) {
        zerocode_core::TrustStanding::Trusted => RepoTrustReport {
            standing: "trusted",
            changed: false,
            content,
        },
        zerocode_core::TrustStanding::NothingToRun => RepoTrustReport {
            standing: "nothing",
            changed: false,
            content: String::new(),
        },
        zerocode_core::TrustStanding::Ask { changed } => RepoTrustReport {
            standing: "ask",
            changed,
            content,
        },
    }
}

/// Remember that somebody approved this repository's scripts.
///
/// The window sends the DECISION and nothing else. The hash is recomputed here
/// from the content on disk right now, because a webview that could name what
/// it was approving could name something other than what it showed — and then
/// the approval would be for a script nobody read. The only thing the window is
/// trusted to report is which button the person pressed.
#[tauri::command(async)]
pub(crate) fn record_repo_trust(
    state: State<'_, AppState>,
    project: Option<String>,
    always: bool,
) -> Result<(), String> {
    let (repo_root, checkout) = repo_trust_target(&state, project);
    let Some(content) = repo_trust_content(&checkout) else {
        // Nothing legible to approve. Storing a hash of the empty string here
        // would hand out an approval that the next readable version of the file
        // could not possibly match — or worse, one that matched a file somebody
        // emptied.
        return Ok(());
    };
    let now = now_epoch_ms();
    let mut held = stored_repo_trust(state.config_root());
    let entry = held.entry(repo_trust_key(&repo_root)).or_default();
    entry.approved_at = Some(now);
    if always {
        entry.all = Some(now);
    } else {
        entry.setup_hash = Some(zerocode_core::content_hash(&content));
    }
    write_repo_trust(state.config_root(), &held)
}

/// Where the repository stands on its ARCHIVE script, for the removal dialog.
///
/// The removal is the action this consent is about, so the ask happens there
/// — Orca's `ensureHooksConfirmed(…, 'archive')` sits in its remove flow the
/// same way. The content shown is the repo's half alone: that is exactly what
/// the approval is over, and a person's own `local_archive` beside it is not
/// up for a vote.
#[tauri::command(async)]
pub(crate) fn archive_trust_standing(
    state: State<'_, AppState>,
    path: String,
) -> Result<RepoTrustReport, String> {
    let (orchestrator, chosen) = known_worktree_context(state.config_root(), &path)?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    let Some(content) = repo_archive_half(state.settings(), &repo_root, &chosen.path)? else {
        return Ok(RepoTrustReport {
            standing: "nothing",
            changed: false,
            content: String::new(),
        });
    };
    Ok(
        match archive_trust_standing_of(state.config_root(), &repo_root, &content) {
            zerocode_core::TrustStanding::Trusted => RepoTrustReport {
                standing: "trusted",
                changed: false,
                content,
            },
            zerocode_core::TrustStanding::NothingToRun => RepoTrustReport {
                standing: "nothing",
                changed: false,
                content: String::new(),
            },
            zerocode_core::TrustStanding::Ask { changed } => RepoTrustReport {
                standing: "ask",
                changed,
                content,
            },
        },
    )
}

/// Remember that somebody approved this repository's archive script.
///
/// The window sends the decision alone; the hash is recomputed here from the
/// file on disk, for `record_repo_trust`'s reason — an approval must be over
/// what will run, not over what a webview said it showed.
#[tauri::command(async)]
pub(crate) fn record_archive_trust(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let (orchestrator, chosen) = known_worktree_context(state.config_root(), &path)?;
    let repo_root = orchestrator.repo_root().to_path_buf();
    let Some(content) = repo_archive_half(state.settings(), &repo_root, &chosen.path)? else {
        return Ok(());
    };
    if content.trim().is_empty() {
        return Ok(());
    }
    let mut held = stored_repo_trust(state.config_root());
    let entry = held.entry(repo_trust_key(&repo_root)).or_default();
    entry.approved_at = Some(now_epoch_ms());
    entry.archive_hash = Some(zerocode_core::content_hash(&content));
    write_repo_trust(state.config_root(), &held)
}

/// Give a repository a colour mark, or take its mark away.
///
/// `color: None` is the erase — the field goes out of the file entirely rather
/// than being stored as a null, which is what keeps "unmarked" one state
/// instead of two. Anything else is read by [`zerocode_core::normalize_repo_mark`]
/// and REFUSED if it is not a colour: the picker only ever sends its own eight
/// strings, so a value that fails here came from somewhere else, and storing it
/// raw would put a string CSS cannot paint into the sidebar. Silently dropping
/// it would be worse still — the person would see the mark they asked for
/// disappear with no word about why.
#[tauri::command(async)]
pub(crate) fn set_repo_mark(
    state: State<'_, AppState>,
    path: String,
    color: Option<String>,
) -> Result<(), String> {
    let mark = match color.as_deref() {
        Some(said) => match zerocode_core::normalize_repo_mark(Some(said)) {
            Some(colour) => Some(colour),
            None => {
                return Err(format!(
                    "{said}은(는) 색이 아닙니다 — #rrggbb로 적어야 합니다"
                ));
            }
        },
        None => None,
    };
    // Keyed by the root as the catalog spells it, so the mark the sidebar reads
    // back is the mark that was saved. The erase that leaves no trace, and the
    // four other facts this entry holds surviving a colour being taken off, are
    // both `update_project_settings`.
    update_project_settings(state.settings(), &project_settings_key(&path), |entry| {
        entry.mark_color = mark;
    })
}

/// Put this project's external worktrees away, or let them in.
///
/// Saved immediately — the dialog has one button and no cancel, so pressing it
/// IS the answer. Freezing which side of the rollout this project is on, and
/// lifting a permanent suppression when the answer is `show`, are both part of
/// the one move ([`zerocode_core::set_visibility`]).
#[tauri::command(async)]
pub(crate) fn set_external_worktree_visibility(
    state: State<'_, AppState>,
    path: String,
    show: bool,
) -> Result<(), String> {
    let key = project_settings_key(&path);
    let added_at = stored_external_visibility(state.settings())?
        .get(&key)
        .and_then(|(_, at)| *at);
    let said = if show {
        zerocode_core::Visibility::Show
    } else {
        zerocode_core::Visibility::Hide
    };
    update_project_settings(state.settings(), &key, |entry| {
        zerocode_core::set_visibility(&mut entry.external_worktrees, added_at, said);
    })
}

/// Stop asking about this project's hidden worktrees — and write down which ones
/// were hidden when that was said.
///
/// The baseline is the feature. Without it "keep hidden" would answer for every
/// external worktree this repository will ever have, and a checkout somebody cut
/// by hand tomorrow would never be mentioned. With it, the answer covers exactly
/// what was on screen when it was given.
///
/// One door for both cards — the one that asks the first time and the inbox that
/// asks about what appeared since — because both say the same thing: *these* are
/// answered, keep them quiet. Two commands would be two chances to write the
/// baseline one way and read it another. The timestamp is only ever set once
/// (`get_or_insert`): it records when this project was first asked, and the
/// inbox's own "keep hidden" is not a first asking.
#[tauri::command(async)]
pub(crate) fn dismiss_external_worktree_prompt(
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let key = project_settings_key(&path);
    let hidden = hidden_external_paths(state.settings(), &key, state.active_root().as_path())?;
    let now = now_epoch_ms();
    update_project_settings(state.settings(), &key, |entry| {
        entry
            .external_worktrees
            .prompt_dismissed_at
            .get_or_insert(now);
        let borrowed: Vec<&str> = hidden.iter().map(String::as_str).collect();
        zerocode_core::merge_baseline(&mut entry.external_worktrees, &borrowed);
    })
}

/// Take these worktrees onto the list by name, whatever the switch says.
///
/// Also lifts a permanent suppression, because importing one is a person asking
/// to see this kind of thing again. A hide with no way back is not a hide.
#[tauri::command(async)]
pub(crate) fn import_external_worktrees(
    state: State<'_, AppState>,
    path: String,
    paths: Vec<String>,
) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let key = project_settings_key(&path);
    // Imported paths are compared against the catalog's spelling, so they are
    // stored in it — a path handed in as typed would never match a row again.
    let named: Vec<String> = paths.iter().map(project_settings_key).collect();
    update_project_settings(state.settings(), &key, |entry| {
        let borrowed: Vec<&str> = named.iter().map(String::as_str).collect();
        zerocode_core::import_clears_suppression(&mut entry.external_worktrees, &borrowed);
    })
}

/// Never mention this project's external worktrees again — including ones made
/// later.
///
/// The one irreversible-looking answer here, and it is not actually
/// irreversible: importing a worktree, or letting them in from the project menu,
/// clears this. The window says so in the dialog that asks for it.
#[tauri::command(async)]
pub(crate) fn suppress_external_worktree_inbox(
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let now = now_epoch_ms();
    update_project_settings(state.settings(), &project_settings_key(&path), |entry| {
        entry.external_worktrees.discovery_suppressed_at = Some(now);
    })
}

#[tauri::command(async)]
pub(crate) fn project_scripts(state: State<'_, AppState>) -> Result<ProjectScriptsReport, String> {
    let context = state.active_project_context();
    project_scripts_for(state.settings(), state.config_root(), &context)
}

#[tauri::command(async)]
pub(crate) fn default_tabs(
    state: State<'_, AppState>,
    worktree: Option<String>,
) -> Result<DefaultTabsReport, String> {
    let context = state.active_project_context();
    let settings = stored_project_script_settings(
        state.settings(),
        &context.repo_root,
        Some(context.checkout.as_path()),
    )?;
    let file = script::read_project_file(&context.checkout).unwrap_or_default();
    // A repo whose commands may never run still gets its tabs — the titles and
    // the layout are the useful half for somebody who wanted the shape without
    // the automation.
    // Machine-only scripts means the person opted OUT of the repository's
    // declarations running here — its commands are shown, never run.
    let repo_commands_barred = matches!(settings.source, zerocode_core::ScriptSource::LocalOnly);
    Ok(DefaultTabsReport {
        applied: worktree.as_deref().is_some_and(|path| {
            stored_applied_default_tabs(state.config_root())
                .iter()
                .any(|held| held == path)
        }),
        runs_commands: if repo_commands_barred {
            Some(false)
        } else {
            zerocode_core::runs_setup_on_create(settings.setup_run_policy)
        },
        tabs: file.default_tabs,
    })
}

/// Remember that this workspace has had its default tabs, so a second visit
/// does not stack a second set.
#[tauri::command(async)]
pub(crate) fn mark_default_tabs_applied(
    state: State<'_, AppState>,
    worktree: String,
) -> Result<(), String> {
    let mut held = stored_applied_default_tabs(state.config_root());
    if held.contains(&worktree) {
        return Ok(());
    }
    held.push(worktree);
    // Bounded: this is a list of paths that only grows, and a person who works
    // through a thousand workspaces should not carry a thousand-line file for a
    // fact that only matters the first time. Oldest out.
    const KEEP: usize = 500;
    if held.len() > KEEP {
        let over = held.len() - KEEP;
        held.drain(..over);
    }
    let file = applied_default_tabs_file(state.config_root());
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string(&held).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// Save only the policy controls the settings pane actually exposes.
#[tauri::command(async)]
pub(crate) fn set_project_script_policy(
    state: State<'_, AppState>,
    source: zerocode_core::ScriptSource,
    setup_run_policy: zerocode_core::SetupRunPolicy,
) -> Result<(), String> {
    let context = state.active_project_context();
    update_project_script_policy(
        state.settings(),
        &context.repo_root,
        &context.checkout,
        source,
        setup_run_policy,
    )
}

/// Patch one project setting. A stale settings window cannot erase sibling
/// scripts because it never sends them.
#[tauri::command(async)]
pub(crate) fn set_project_script_setting(
    state: State<'_, AppState>,
    field: ProjectScriptField,
    value: Option<String>,
) -> Result<ProjectScriptsReport, String> {
    let context = state.active_project_context();
    let root_key = project_settings_key(&context.repo_root);
    mutate_project_settings(state.settings(), |held| {
        // Promote the active checkout's legacy fields before applying the
        // field patch. The canonical root wins when both exist.
        let current =
            project_script_settings_from(held, &context.repo_root, Some(&context.checkout));
        let root_had_scripts = held
            .get(&root_key)
            .is_some_and(ProjectSettings::speaks_about_scripts);
        let entry = held.entry(root_key.clone()).or_insert(current);
        match field {
            ProjectScriptField::Source => {
                entry.source = serde_json::from_value(serde_json::Value::String(
                    value.ok_or("스크립트 소스를 골라야 합니다")?,
                ))
                .map_err(|_| "알 수 없는 스크립트 소스입니다".to_string())?;
            }
            ProjectScriptField::SetupRunPolicy => {
                entry.setup_run_policy = serde_json::from_value(serde_json::Value::String(
                    value.ok_or("실행 정책을 골라야 합니다")?,
                ))
                .map_err(|_| "알 수 없는 실행 정책입니다".to_string())?;
            }
            ProjectScriptField::LocalSetup => {
                entry.local_setup = value
                    .map(|one| one.trim().to_string())
                    .filter(|one| !one.is_empty());
            }
            ProjectScriptField::LocalArchive => {
                entry.local_archive = value
                    .map(|one| one.trim().to_string())
                    .filter(|one| !one.is_empty());
            }
        }

        if !root_had_scripts {
            for candidate in [
                context.checkout.to_string_lossy().into_owned(),
                project_settings_key(&context.checkout),
            ] {
                if candidate == root_key {
                    continue;
                }
                if let Some(legacy) = held.get_mut(&candidate) {
                    clear_project_script_fields(legacy);
                }
                if held
                    .get(&candidate)
                    .is_some_and(ProjectSettings::says_nothing)
                {
                    held.remove(&candidate);
                }
            }
        }
        Ok(())
    })?;
    project_scripts_for(state.settings(), state.config_root(), &context)
}
