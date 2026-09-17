//! GitHub pull request depth (t-2733): reviewers written back, one file's
//! diff read at a time.
//!
//! Beside `worktree.rs`'s item commands rather than inside them because these
//! two are the PR's own — an issue has neither — and the module boundary is
//! what keeps the dialog's read path (`github_work_item_detail`) free of the
//! write it now also offers.

use crate::*;

/// Ask for, or take back, reviewers on a pull request.
///
/// The login array travels to `gh` unmodified (`gh::set_reviewers`) — one
/// flag per login, in the order the chips stand. `remove` picks the verb;
/// nothing else is decided here. The refusal is `gh`'s first line.
#[tauri::command]
pub(crate) async fn github_set_reviewers(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
    logins: Option<Vec<String>>,
    remove: Option<bool>,
) -> Result<(), GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GhFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let logins = logins.unwrap_or_default();
    let remove = remove.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        gh::set_reviewers(&root, item, &logins, remove).map_err(GhFailure::from)
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}

/// One file of one pull request, in the rows the diff viewer already draws.
///
/// The whole PR diff is read once and kept per PR (`checks_runtime::pr_diff`,
/// byte-capped LRU); every file after the first is a cut of that text, so
/// switching files in the dialog spends no second process. The same ceiling
/// `commit_file_diff` keeps applies: a file past it hands back the reason and
/// no rows.
#[tauri::command]
pub(crate) async fn github_pr_file_diff(
    state: State<'_, AppState>,
    project: Option<String>,
    number: Option<u64>,
    path: Option<String>,
) -> Result<DiffView, GhFailure> {
    let orchestrator =
        known_project_orchestrator(state.config_root(), &project.unwrap_or_default())
            .map_err(GhFailure::ours)?;
    let root = orchestrator.repo_root().to_path_buf();
    let item = number.ok_or_else(|| GhFailure::ours("작업 항목 번호가 없습니다".to_string()))?;
    let path = path
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| GhFailure::ours("파일 경로가 없습니다".to_string()))?;
    let limits = checks_runtime::limits(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let whole = checks_runtime::pr_diff(&root, item, &limits).map_err(GhFailure::from)?;
        let section = checks_runtime::file_section(&whole, &path).unwrap_or_default();
        if let Some(limit) = diff_render_limit(section) {
            return Ok(DiffView {
                limit: Some(limit),
                lines: Vec::new(),
                texts: None,
            });
        }
        Ok(DiffView {
            limit: None,
            lines: parse_unified_diff(section),
            texts: None,
        })
    })
    .await
    .map_err(|join| GhFailure::ours(join.to_string()))?
}
