use crate::browser_workspace::{Mode, Operation, Snapshot};
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

fn owner_actor(app: &AppHandle, owner_term: &str) -> Result<String, String> {
    let term: u32 = owner_term
        .parse()
        .map_err(|_| "workspace_owner_invalid: 에이전트 판 번호가 아닙니다")?;
    if term.to_string() != owner_term {
        return Err("workspace_owner_invalid: 에이전트 판 번호가 아닙니다".to_string());
    }
    crate::artifact_runtime::held_pane(app, &hooks::pane_key_of(term))
        .map(|held| held.key)
        .ok_or_else(|| {
            "workspace_owner_missing: 선택한 에이전트 판이 현재 창에 없습니다".to_string()
        })
}

#[tauri::command]
pub(crate) async fn set_browser_workspace_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
    mode: Mode,
    expected_revision: u64,
    owner_term: Option<String>,
) -> Result<Snapshot, String> {
    from_the_main_webview(&webview)?;
    let pane = browser_pane_of(&app, &state, &label)?;
    let existing = state
        .shell_runtime()
        .browser_workspaces
        .snapshot(&label)?
        .ok_or("workspace_missing: 이 탭은 전용 작업 브라우저가 아닙니다")?;
    let wanted = owner_term.as_deref().or(existing.owner_term.as_deref());
    let owner = if mode == Mode::Agent || owner_term.is_some() {
        Some(owner_actor(
            &app,
            wanted.ok_or("workspace_owner_required: 맡길 에이전트 판을 선택하세요")?,
        )?)
    } else {
        None
    };
    let notify_app = app.clone();
    let mut transition = state
        .shell_runtime()
        .browser_workspaces
        .transition_owned(&label, mode, expected_revision, owner.as_deref())?
        .notifying(move |snapshot| {
            let _ = notify_app.emit_to("main", "browser:workspace", snapshot);
        });
    let _ = app.emit_to("main", "browser:workspace", transition.snapshot()?);
    super::browser::hide_browser_pane(&pane, &webview)?;
    tokio::time::timeout(
        super::browser::BROWSER_DOOR_BUDGET + super::browser::BROWSER_CALLBACK_DEADLINE,
        transition.wait(),
    )
    .await
    .map_err(|_| {
        "workspace_paused: 요청 대기 시간이 지났습니다 — 일시정지 상태에서 다시 제어권을 선택하세요"
            .to_string()
    })??;
    if mode != Mode::Paused {
        let labels = transition.fence_labels()?;
        super::browser::settle_known_navigation(&app, &state, &labels).await?;
        tokio::time::timeout(super::browser::BROWSER_CALLBACK_DEADLINE, async {
            for fenced in labels {
                if state.browser_panes().contains(&fenced) {
                    let pane = browser_pane_of(&app, &state, &fenced)?;
                    super::browser::workspace_page_fence(&pane).await?;
                } else if fenced == label {
                    return Err("workspace_missing: 작업 브라우저가 닫혔습니다".to_string());
                }
            }
            Ok::<(), String>(())
        }).await.map_err(|_| "workspace_paused: 페이지 상태를 확인하지 못했습니다 — 일시정지 상태에서 다시 제어권을 선택하세요".to_string())??;
        transition.confirm_fence()?;
    }
    super::browser::forget_workspace_observation(&label);
    transition.finish()
}

pub(crate) fn begin_person_control(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    expected_revision: Option<u64>,
) -> Result<Option<Operation>, String> {
    let notify_app = app.clone();
    Ok(state
        .shell_runtime()
        .browser_workspaces
        .begin_person(label, expected_revision)?
        .map(|operation| {
            operation.notifying(move |snapshot| {
                let _ = notify_app.emit_to("main", "browser:workspace", snapshot);
            })
        }))
}

pub(crate) fn finish_person_control<T>(
    operation: Option<Operation>,
    result: Result<T, String>,
) -> Result<T, String> {
    if let Some(operation) = operation {
        operation.finish(result.is_ok());
    }
    result
}
