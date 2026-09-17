//! Integration prefs commands.

use crate::*;

/// Canonical, token-free GitHub integration state. `force` belongs only to an
/// explicit recheck: it re-reads the login shell PATH after somebody installs
/// `gh`; ordinary pane opens use the warmed value.
#[tauri::command]
pub(crate) async fn github_status(
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<gh::GithubStatus, String> {
    let (root, remote) = github_context(&state);
    tauri::async_runtime::spawn_blocking(move || {
        gh::integration_status(&root, remote.as_deref(), force.unwrap_or(false))
            .map_err(github_lifecycle_error)
    })
    .await
    .map_err(|_| "GitHub 계정 상태 확인이 중단되었습니다".to_string())?
}

#[tauri::command]
pub(crate) async fn github_select_account(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<gh::GithubStatus, String> {
    let (root, remote) = github_context(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let status = gh::select_account(&root, remote.as_deref(), &account_id)
            .map_err(github_lifecycle_error)?;
        readiness_runtime::gh_login_moved();
        Ok(status)
    })
    .await
    .map_err(|_| "GitHub 계정 전환이 중단되었습니다".to_string())?
}

#[tauri::command]
pub(crate) async fn github_disconnect(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<gh::GithubStatus, String> {
    let (root, remote) = github_context(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let status = gh::disconnect_account(&root, remote.as_deref(), &account_id)
            .map_err(github_lifecycle_error)?;
        readiness_runtime::gh_login_moved();
        Ok(status)
    })
    .await
    .map_err(|_| "GitHub 연결 해제가 중단되었습니다".to_string())?
}

#[tauri::command]
pub(crate) async fn github_test_connection(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<gh::GithubTestReport, String> {
    let (root, remote) = github_context(&state);
    tauri::async_runtime::spawn_blocking(move || {
        gh::test_connection(&root, remote.as_deref(), &account_id).map_err(github_lifecycle_error)
    })
    .await
    .map_err(|_| "GitHub 연결 검사가 중단되었습니다".to_string())?
}

/// The explicit login button opens the existing plain terminal with this safe
/// command. No token or login form crosses the renderer boundary.
#[tauri::command(async)]
pub(crate) fn github_login_intent(state: State<'_, AppState>) -> String {
    let (_, remote) = github_context(&state);
    gh::login_command(remote.as_deref())
}

/// 설치 여부와 로그인 여부, 그리고 로그인된 호스트. 토큰은 건너오지 않는다.
///
/// `force`는 명시적인 "다시 확인"의 몫이다: `glab`을 방금 깐 사람에게는 로그인
/// 셸 PATH를 다시 읽는 것이 곧 감지이고, 평범한 창 열기는 데워진 값을 쓴다.
#[tauri::command]
pub(crate) async fn gitlab_status(
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<glab::GlabStatus, String> {
    let root = state.active_root();
    tauri::async_runtime::spawn_blocking(move || {
        glab::integration_status(&root, force.unwrap_or(false)).map_err(gitlab_lifecycle_error)
    })
    .await
    .map_err(|_| "GitLab 연동 상태 확인이 중단되었습니다".to_string())?
}

/// 알림이 실제로 배달되는지, 물어보는 대신 **한 번 쏴 보고** 안다.
///
/// 권한 상태를 돌려주는 API가 없다는 것이 Orca가 이 방법을 쓰는 이유이고
/// (`probeDelivery`), 여기도 사정이 같다 — `ring_now`가 알림을 못 띄웠을 때
/// `notify:blocked`를 내보내는 그 판정을 사람이 직접 부를 수 있게 한 것이다.
///
/// 문구는 **창이 들고 온다**. 번역표는 `shell.js`에 있고, 백엔드가 문장을
/// 지어내면 그 알림만 사람이 고른 언어를 어긴다(1-dz의 팝아웃 제목과 같은
/// 이유).
#[tauri::command(async)]
pub(crate) fn notification_probe(app: AppHandle, title: String, body: String) -> bool {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .is_ok()
}

#[tauri::command(async)]
pub(crate) fn set_notification_preference(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    kind: NotificationPreferenceKind,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::NOTIFICATIONS],
        move |settings| {
            match kind {
                NotificationPreferenceKind::Enabled => {
                    settings.notifications.enabled = on;
                }
                NotificationPreferenceKind::AgentAttention => {
                    settings.notifications.agent_attention = on;
                }
                NotificationPreferenceKind::AgentCompletion => {
                    settings.notifications.agent_completion = on;
                }
            }
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_browser_home_page(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    home_page: String,
) -> Result<SettingsSnapshot, String> {
    let canonical = normalized_browser_url(&home_page)
        .ok_or("브라우저 홈은 http://, https:// 또는 about:blank 주소여야 합니다")?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| {
            settings.browser.home_page = canonical;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_browser_search_engine(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    search_engine: BrowserSearchEngine,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| {
            settings.browser.search_engine = search_engine;
            Ok(())
        },
    )
}

/// Change one link-routing choice against the latest Browser preferences.
/// The tagged patch keeps stale Settings windows from replacing the sibling
/// choice they have not re-read yet.
#[tauri::command(async)]
pub(crate) fn patch_browser_link_routing(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: BrowserLinkRoutingPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| {
            patch.apply(&mut settings.browser);
            Ok(())
        },
    )
}

/// Change one row of the per-site user-agent table against the latest
/// Browser preferences (browser-door-for-agents §2.5). A row patch, not a
/// table replace, for the reason link routing is a field patch: a stale
/// Settings window must not erase the row a sibling window just wrote. The
/// validation is the patch's own — a bad host or agent is refused before
/// anything is committed. A row reaches the NEXT pane born on that host; a
/// running pane keeps the agent it was built with.
#[tauri::command(async)]
pub(crate) fn patch_browser_user_agents(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: BrowserUserAgentPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| patch.apply(&mut settings.browser),
    )
}

#[tauri::command(async)]
pub(crate) fn set_browser_restore_tabs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| {
            settings.browser.restore_tabs = on;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_browser_default_zoom(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    zoom_level: f64,
) -> Result<SettingsSnapshot, String> {
    let canonical = normalize_zoom_level(zoom_level);
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| {
            settings.browser.default_zoom_level = canonical;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_browser_open_tabs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    tabs: Vec<StoredBrowserTab>,
) -> Result<SettingsSnapshot, String> {
    let canonical = tabs
        .into_iter()
        .map(|tab| {
            normalized_browser_url(&tab.url)
                .filter(|url| !url.is_empty())
                .map(|url| StoredBrowserTab { url, ..tab.clone() })
                .ok_or_else(|| format!("브라우저 탭 주소를 저장할 수 없습니다: {}", tab.url))
        })
        .collect::<Result<Vec<_>, _>>()?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| {
            settings.browser.open_tabs = canonical;
            Ok(())
        },
    )
}

/// The address bar's visit ledger, replaced whole. Only web addresses are
/// kept — a `file:` path in a suggestions file is a directory listing of the
/// machine — and the bound holds however long the window ran.
#[tauri::command(async)]
pub(crate) fn set_browser_visits(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    visits: Vec<StoredBrowserVisit>,
) -> Result<SettingsSnapshot, String> {
    let canonical = visits
        .into_iter()
        .filter(|visit| visit.url.starts_with("http://") || visit.url.starts_with("https://"))
        .take(BROWSER_VISITS_KEPT)
        .collect::<Vec<_>>();
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::BROWSER],
        move |settings| {
            settings.browser.visits = canonical;
            Ok(())
        },
    )
}
