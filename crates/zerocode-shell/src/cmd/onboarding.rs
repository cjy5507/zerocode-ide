//! Onboarding commands.

use crate::*;
use zerocode_core::pick::{PickKind, PickRequest};

/// The native folder panel, in the helper's process and guarded — see
/// [`pick_paths`] for the clock it keeps and the trace it leaves. The
/// in-window browser's 「시스템 대화상자로 찾기」 comes here in folder mode;
/// `start` is the folder the browser was showing.
#[tauri::command]
pub(crate) async fn choose_project(
    app: AppHandle,
    start: Option<String>,
) -> Result<Option<String>, String> {
    let paths = pick_paths(&app, native_pick_request(PickKind::Folder, start)).await?;
    Ok(paths
        .into_iter()
        .next()
        .map(|path| path.to_string_lossy().into_owned()))
}

/// The native panel for any kind — the browser's attach mode asks for
/// `files`. Same seat, same clock.
#[tauri::command]
pub(crate) async fn choose_paths(
    app: AppHandle,
    kind: PickKind,
    start: Option<String>,
) -> Result<Vec<String>, String> {
    let paths = pick_paths(&app, native_pick_request(kind, start)).await?;
    Ok(paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect())
}

/// A start folder the window typed or browsed to; an empty one is none.
fn native_pick_request(kind: PickKind, start: Option<String>) -> PickRequest {
    let request = PickRequest::new(kind);
    match start
        .as_deref()
        .map(str::trim)
        .filter(|start| !start.is_empty())
    {
        Some(start) => {
            request.starting_at(expand_home(start, workspace_home_directory().as_deref()))
        }
        None => request,
    }
}

/// The person cannot see the panel — bring the helper that holds it (or
/// the window) forward. The toast's retry, the browser's strip and a second
/// press of the sidebar button all come here; none opens a second panel.
#[tauri::command(async)]
pub(crate) fn recall_folder_panel(app: AppHandle) -> bool {
    recall_folder_panel_window(&app)
}

/// The person gives up on the panel — kill the helper.
#[tauri::command(async)]
pub(crate) fn cancel_folder_panel(app: AppHandle) -> bool {
    cancel_folder_panel_window(&app)
}

/// 이 URL을 복제하면 생기는 디렉터리 이름 — 창이 미리 보여 주는 그 이름.
///
/// 규칙은 [`zerocode_core::clone::clone_dir_name`] 하나다. 창에 한 벌 더 두지
/// 않는 이유는 [`work_item_seed`]와 같다: 미리 보기와 실제로 만들어지는 이름이
/// 두 규칙에서 나오면 언젠가 갈라지고, 갈라진 날의 증상은 "미리 보기와 다른
/// 폴더가 생겼다"이다.
#[tauri::command]
pub(crate) fn clone_target_name(url: String) -> Option<String> {
    let _crumb = crate::crumbs::Command::enter("clone_target_name");
    zerocode_core::clone::clone_dir_name(&url)
}

/// 새 체크아웃이 앉을 기본 부모 폴더.
///
/// Orca's `getDefaultCloneParent`: a workspace directory literally ending in
/// `workspaces` contributes its parent; every other configured directory is
/// already the intended clone parent. Relative values are resolved against the
/// active repository before they cross the native clone boundary.
#[tauri::command(async)]
pub(crate) fn default_project_parent(state: State<'_, AppState>) -> Result<String, String> {
    let root = state.active_root();
    let prefs = load_settings_for_boot(state.settings())
        .document
        .workspace_creation_prefs;
    let workspace_directory = workspace_directory_for_repo(&root, &prefs)?;
    let parent = if workspace_directory
        .file_name()
        .is_some_and(|name| name == "workspaces")
    {
        workspace_directory
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or(workspace_directory)
    } else {
        workspace_directory
    };
    Ok(parent.to_string_lossy().into_owned())
}

/// 원격 저장소 하나를 `parent/name`으로 받아 온다.
///
/// 되돌아오는 것은 만들어진 경로다. 등록도 활성화도 창이 그 경로로
/// `open_project`를 불러서 한다 — 폴더를 고른 사람이 지나는 길과 한 글자도
/// 다르지 않아야 한다.
///
/// 실패하면 **git이 만든 것만 치운다.** 시작 전에 대상이 없다는 것을 확인했으므로
/// 지금 거기 있는 것은 전부 이 클론이 만든 것이고, 그래서 지워도 되는 것이다.
/// 이미 있던 폴더를 지우는 경로는 이 순서 덕분에 존재하지 않는다.
#[tauri::command]
pub(crate) async fn clone_repository(
    app: AppHandle,
    url: String,
    parent: String,
    name: Option<String>,
) -> Result<String, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("복제할 주소를 적어 주세요".to_string());
    }
    // 사람이 고쳐 적은 이름이 있으면 그것, 없으면 URL이 정하는 이름.
    let named = name
        .as_deref()
        .map(str::trim)
        .filter(|one| !one.is_empty())
        .map(str::to_string)
        .or_else(|| zerocode_core::clone::clone_dir_name(&url))
        .ok_or_else(|| "이 주소에서 폴더 이름을 읽지 못했습니다".to_string())?;
    let target = place_inside(&parent, &named)?;
    tauri::async_runtime::spawn_blocking(move || {
        let host = Host::for_workspace(&target);
        let mut say = |step: zerocode_core::clone::CloneStep| {
            let _ = app.emit(
                "project:clone-progress",
                CloneProgress {
                    phase: step.phase,
                    percent: step.percent,
                },
            );
        };
        match host.vcs().clone(&url, &target, &mut say) {
            Ok(()) => Ok(target.to_string_lossy().into_owned()),
            Err(said) => {
                // 반쯤 받아 온 디렉터리는 저장소도 아니고 빈 폴더도 아니다 —
                // 남겨 두면 다음 시도가 "이미 있습니다"로 막힌다.
                let _ = std::fs::remove_dir_all(&target);
                Err(said)
            }
        }
    })
    .await
    .map_err(|join| join.to_string())?
}

/// 빈 폴더 하나를 만들고 그 안에서 저장소를 시작한다.
///
/// `git init`도 [`Host`]의 git을 지난다. 여기서 `git`을 직접 띄우는 순간 그것이
/// 원격 호스트에서 조용히 로컬 디스크에 저장소를 만드는 첫 번째 자리가 된다
/// (`zerocode_core::host`의 계약 3).
#[tauri::command]
pub(crate) async fn create_project(parent: String, name: String) -> Result<String, String> {
    let target = place_inside(&parent, name.trim())?;
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
        let host = Host::for_workspace(&target);
        match host.vcs().text(&target, &["init"]) {
            Ok(_) => Ok(target.to_string_lossy().into_owned()),
            Err(said) => {
                // 방금 만든 폴더다. `git init`이 거절했다면 남길 이유가 없다.
                let _ = std::fs::remove_dir_all(&target);
                Err(said)
            }
        }
    })
    .await
    .map_err(|join| join.to_string())?
}

/// 한 단계를 끝냈다. 창이 다음 단계로 넘어갈 때마다 부른다.
///
/// 범위 밖 단계는 잘리지 않고 **거절된다**: 창이 못 알아들을 값을 보냈다는 것은
/// 창의 버그이고, 근사해서 저장하면 그 버그가 사람의 진행도로 굳는다.
#[tauri::command(async)]
pub(crate) fn save_onboarding_step(
    state: State<'_, AppState>,
    step: i32,
    chose_agent: bool,
) -> Result<zerocode_core::Onboarding, String> {
    let step = zerocode_core::onboarding::sanitized_step(step)
        .ok_or_else(|| format!("{step}은(는) 이 마법사의 단계가 아닙니다"))?;
    let next = zerocode_core::onboarding::stepped(
        &stored_onboarding(state.config_root()),
        step,
        chose_agent,
    );
    write_onboarding(state.config_root(), &next)?;
    Ok(next)
}

/// 마법사가 닫혔다 — 끝까지 갔거나(`completed`), 도중에 나갔거나(`dismissed`).
///
/// 닫힌 시각은 **여기서** 찍는다. 창이 보낸 시각을 믿으면 시계가 틀어진 기계
/// 하나가 "닫힌 적 없음"으로 읽히는 값을 저장할 수 있다.
#[tauri::command(async)]
pub(crate) fn close_onboarding(
    state: State<'_, AppState>,
    outcome: String,
) -> Result<zerocode_core::Onboarding, String> {
    let next = zerocode_core::onboarding::closing(
        &stored_onboarding(state.config_root()),
        &outcome,
        now_epoch_ms(),
    )
    .ok_or_else(|| format!("{outcome}은(는) 아는 결말이 아닙니다"))?;
    write_onboarding(state.config_root(), &next)?;
    Ok(next)
}

/// 다시 보기. 도움말의 숨은 항목 하나만 부른다(Orca도 Alt를 눌러야 나오는
/// 항목 하나뿐이다 — 일반 사용자에게 재실행 경로는 없다).
#[tauri::command(async)]
pub(crate) fn reopen_onboarding(
    state: State<'_, AppState>,
) -> Result<zerocode_core::Onboarding, String> {
    let next = zerocode_core::onboarding::reopened(&stored_onboarding(state.config_root()));
    write_onboarding(state.config_root(), &next)?;
    Ok(next)
}

/// 사람이 이미 한 일 하나를 적는다. 켜기만 하고, 모르는 이름은 거절한다.
#[tauri::command(async)]
pub(crate) fn mark_onboarding(
    state: State<'_, AppState>,
    mark: String,
) -> Result<zerocode_core::Onboarding, String> {
    let current = stored_onboarding(state.config_root());
    let Some(next) = zerocode_core::onboarding::marked(&current, &mark) else {
        return Err(format!("{mark}은(는) 이 목록의 칸이 아닙니다"));
    };
    // 이미 켜져 있으면 파일을 다시 쓰지 않는다. 같은 값을 적는 쓰기는 아무것도
    // 바꾸지 않으면서 실패할 수만 있고, 이 문은 클릭마다 두들겨진다.
    if next == current {
        return Ok(current);
    }
    write_onboarding(state.config_root(), &next)?;
    Ok(next)
}

/// 사이드바의 시작 체크리스트를 치우거나 되돌린다.
#[tauri::command(async)]
pub(crate) fn set_guide_dismissed(
    state: State<'_, AppState>,
    dismissed: bool,
) -> Result<zerocode_core::Onboarding, String> {
    let next = zerocode_core::onboarding::guide_dismissed(
        &stored_onboarding(state.config_root()),
        dismissed,
    );
    write_onboarding(state.config_root(), &next)?;
    Ok(next)
}

/// 투어·팁·기능 투어의 "봤다" 목록에 하나를 더한다.
///
/// 목록 셋이 문 하나를 쓴다. 셋 다 하는 일이 같기 때문이다 — 아는 이름인지 묻고,
/// 없으면 더하고, 있으면 아무것도 하지 않는다. 문을 셋으로 나누면 그중 하나는
/// 반드시 중복 검사를 잊는다.
#[tauri::command(async)]
pub(crate) fn mark_first_run_seen(
    state: State<'_, AppState>,
    kind: String,
    id: String,
) -> Result<zerocode_core::Onboarding, String> {
    use zerocode_core::guide;
    if !guide::known(&kind, &id) {
        return Err(format!("{kind}에 {id}(이)라는 것은 없습니다"));
    }
    let current = stored_onboarding(state.config_root());
    let held = match kind.as_str() {
        guide::SEEN_TOUR => &current.tours_seen,
        guide::SEEN_TIP => &current.tips_seen,
        _ => &current.wall_seen,
    };
    let Some(grown) = zerocode_core::onboarding::seen(held, &id) else {
        return Ok(current);
    };
    let mut next = current.clone();
    match kind.as_str() {
        guide::SEEN_TOUR => next.tours_seen = grown,
        guide::SEEN_TIP => next.tips_seen = grown,
        _ => next.wall_seen = grown,
    }
    write_onboarding(state.config_root(), &next)?;
    Ok(next)
}

#[tauri::command(async)]
pub(crate) fn setup_guide(
    state: State<'_, AppState>,
    ready: bool,
    projects: usize,
    side_worktrees: usize,
    github: bool,
) -> GuideReport {
    let onboarding = stored_onboarding(state.config_root());
    let facts = zerocode_core::GuideFacts {
        chose_agent: load_settings_for_boot(state.settings())
            .document
            .default_agent
            .agent_id()
            .is_some(),
        projects,
        side_worktrees,
        github,
    };
    let steps = zerocode_core::guide::steps(&onboarding, &facts);
    let complete = zerocode_core::guide::complete(&steps);
    GuideReport {
        entry: zerocode_core::guide::entry_visible(ready, complete, onboarding.guide_dismissed),
        steps,
        complete,
        dismissed: onboarding.guide_dismissed,
    }
}

#[tauri::command(async)]
pub(crate) fn tour_decision(state: State<'_, AppState>, ask: TourAsk) -> Option<String> {
    let onboarding = stored_onboarding(state.config_root());
    let asked = zerocode_core::guide::TourAsk {
        id: &ask.id,
        here: ask.here,
        ready: ask.ready,
        auto: onboarding.tours_auto,
        onboarding_open: zerocode_core::onboarding::should_show(&onboarding),
        modal_open: ask.modal_open,
        active_tour: ask.active_tour,
        spent_this_session: ask.spent_this_session,
        seen: &onboarding.tours_seen,
        has_target: ask.has_target,
    };
    // 거절 사유는 이름으로 나간다. 불리언 하나로 뭉치면 창의 로그가 "안 떴다"밖에
    // 말하지 못한다.
    match zerocode_core::guide::tour_decision(&asked) {
        Ok(()) => None,
        Err(why) => Some(format!("{why:?}")),
    }
}

#[tauri::command(async)]
pub(crate) fn tip_verdict(
    state: State<'_, AppState>,
    ready: bool,
    sealed: bool,
    spent_this_open: bool,
    modal_open: bool,
) -> TipReport {
    let onboarding = stored_onboarding(state.config_root());
    let asked = zerocode_core::guide::TipAsk {
        ready,
        onboarding_open: zerocode_core::onboarding::should_show(&onboarding),
        sealed,
        spent_this_open,
        modal_open,
    };
    match zerocode_core::guide::tip_verdict(&onboarding, &asked) {
        zerocode_core::guide::TipVerdict::SuppressForOnboarding => TipReport {
            verdict: "suppress",
            id: None,
            settle: Vec::new(),
        },
        zerocode_core::guide::TipVerdict::Skip => TipReport {
            verdict: "skip",
            id: None,
            settle: Vec::new(),
        },
        zerocode_core::guide::TipVerdict::Show { id, settle } => TipReport {
            verdict: "show",
            id: Some(id),
            settle,
        },
    }
}
