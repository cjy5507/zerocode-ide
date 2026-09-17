//! Skills IPC: gather context once, do disk work off the UI thread.
use crate::skills_runtime::{self, Context};
use crate::*;

#[tauri::command]
pub(crate) async fn skills_list(
    state: State<'_, AppState>,
) -> Result<skills_runtime::Report, String> {
    let context = Context::from_state(&state)?;
    tauri::async_runtime::spawn_blocking(move || skills_runtime::list(context, false))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) async fn skills_rescan(
    state: State<'_, AppState>,
    term: Option<TermId>,
) -> Result<skills_runtime::Report, String> {
    let context = Context::from_state(&state)?;
    let output = term.and_then(|term| skills_runtime::terminal_output(&state, term, &context));
    tauri::async_runtime::spawn_blocking(move || skills_runtime::rescan(context, output))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) async fn skill_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<skills_runtime::Detail, String> {
    let context = Context::from_state(&state)?;
    tauri::async_runtime::spawn_blocking(move || skills_runtime::detail(context, id))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn skill_install_plan(
    state: State<'_, AppState>,
    repository: Option<String>,
    names: Vec<String>,
    agents: Vec<String>,
    action: String,
) -> Result<skills_runtime::Plan, String> {
    let context = Context::from_state(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        skills_runtime::plan(context, repository, names, agents, action)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn skill_bundle_parse(
    state: State<'_, AppState>,
    output: String,
) -> Result<zerocode_core::skill::BundleListing, String> {
    let _crumb = crate::crumbs::Command::enter("skill_bundle_parse");
    Ok(skills_runtime::parse_bundle(
        Context::from_state(&state)?,
        &output,
    ))
}
#[tauri::command(async)]
pub(crate) fn skill_reveal(state: State<'_, AppState>, path: String) -> Result<(), String> {
    skills_runtime::reveal_skill(Context::from_state(&state)?, path)
}
// Compatibility entry points: existing integration panes share this runtime.
#[tauri::command]
pub(crate) async fn list_skills(
    state: State<'_, AppState>,
) -> Result<zerocode_core::skill::SkillReport, String> {
    let context = Context::from_state(&state)?;
    tauri::async_runtime::spawn_blocking(move || skills_runtime::legacy_list(context))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) async fn orchestration_report(
    state: State<'_, AppState>,
) -> Result<zerocode_core::skill::OrchestrationReport, String> {
    let context = Context::from_state(&state)?;
    tauri::async_runtime::spawn_blocking(move || skills_runtime::legacy_orchestration(context))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) async fn computer_use_skill_report(
    state: State<'_, AppState>,
) -> Result<zerocode_core::skill::ComputerUseSkillReport, String> {
    let context = Context::from_state(&state)?;
    tauri::async_runtime::spawn_blocking(move || skills_runtime::legacy_computer_use(context))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) async fn install_bundled_skill(
    state: State<'_, AppState>,
    name: String,
) -> Result<Vec<zerocode_core::skill_install::SkillInstallOutcome>, String> {
    let context = Context::from_state(&state)?;
    tauri::async_runtime::spawn_blocking(move || skills_runtime::install_bundled(context, name))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command(async)]
pub(crate) fn reveal_skill(state: State<'_, AppState>, path: String) -> Result<(), String> {
    skills_runtime::reveal_skill(Context::from_state(&state)?, path)
}
