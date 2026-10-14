use crate::browser_workspace::{Mode, Snapshot};
use crate::*;

#[tauri::command(async)]
pub(crate) fn get_browser_workspaces(
    webview: tauri::Webview,
    state: State<'_, AppState>,
) -> Result<Vec<Snapshot>, String> {
    from_the_main_webview(&webview)?;
    let live = state.browser_panes().clone();
    Ok(state
        .shell_runtime()
        .browser_workspaces
        .snapshots()?
        .into_iter()
        .filter(|snapshot| live.contains(&snapshot.label))
        .collect())
}

#[tauri::command]
pub(crate) async fn set_browser_workspace_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    mode: Mode,
    expected_revision: u64,
) -> Result<Snapshot, String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    let transition =
        state
            .shell_runtime()
            .browser_workspaces
            .transition(&label, mode, expected_revision)?;
    let _ = app.emit("browser:workspace", transition.snapshot()?);
    super::browser::hide_browser_pane(&pane, &webview)?;
    tokio::time::timeout(Duration::from_secs(8), transition.wait())
        .await
        .map_err(|_| {
            "진행 중인 브라우저 요청을 기다리고 있습니다 — 직접 조작은 아직 잠겨 있습니다"
                .to_string()
        })??;
    super::browser::forget_workspace_observation(&label);
    let snapshot = transition.finish()?;
    let _ = app.emit("browser:workspace", &snapshot);
    Ok(snapshot)
}

pub(crate) fn require_person_control(state: &AppState, label: &str) -> Result<(), String> {
    if state
        .shell_runtime()
        .browser_workspaces
        .allows_person(label)?
    {
        Ok(())
    } else {
        Err("에이전트의 작업 브라우저입니다 — 먼저 ‘내가 조작’을 눌러 주세요".to_string())
    }
}
