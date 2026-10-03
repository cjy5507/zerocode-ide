//! Artifact commands (t-2720 §2): the window's six doors to the catalog.
//!
//! Every command here is a read but one. `artifact_delete` is the write, and
//! it carries the person's confirmation through to the store rather than
//! deciding anything itself — the store refuses an unconfirmed delete, so a
//! caller that forgot to ask cannot delete by accident. Nothing here touches
//! the disk: paths come from the store, launchers get the path, and the
//! store's fence (the app's own data root) is the only fence there is.

use crate::*;

use zerocode_core::artifact::Artifact;
use zerocode_core::artifact_publish::ExportFormat;

/// What one listing answers, plus the retention the sweep is running with —
/// the drawer shows it beside the file's age.
#[derive(Serialize)]
pub(crate) struct ArtifactListing {
    pub(crate) rows: Vec<Artifact>,
    pub(crate) total: usize,
    pub(crate) truncated: bool,
    pub(crate) missing: Vec<String>,
    pub(crate) missing_total: usize,
    pub(crate) by_kind: std::collections::BTreeMap<String, usize>,
    pub(crate) retention_days: u32,
    pub(crate) thumb: ThumbTable,
}

fn store_or_refuse() -> Result<Arc<artifact_runtime::Store>, String> {
    artifact_runtime::store().ok_or_else(|| "아티팩트 스토어가 아직 열리지 않았습니다".to_string())
}

/// The catalog, filtered — kind, origin fields and a body query — newest
/// first and bounded by the table.
#[tauri::command(async)]
pub(crate) fn artifacts_list(filter: artifact_runtime::Filter) -> Result<ArtifactListing, String> {
    let store = store_or_refuse()?;
    let listing = store.list(&filter);
    let limits = store.limits();
    Ok(ArtifactListing {
        rows: listing.rows,
        total: listing.total,
        truncated: listing.truncated,
        missing: listing.missing,
        missing_total: listing.missing_total,
        by_kind: listing.by_kind,
        retention_days: limits.retention_days,
        thumb: ThumbTable {
            width: limits.thumb_width,
            height: limits.thumb_height,
            queue_max: limits.thumb_queue_max,
        },
    })
}

/// Body search — the same road as the listing, with only a query. The
/// window filters kinds and origins itself once it holds the rows; this is
/// for a caller that has nothing yet.
#[tauri::command(async)]
pub(crate) fn artifact_search(query: String) -> Result<ArtifactListing, String> {
    artifacts_list(artifact_runtime::Filter {
        query,
        ..artifact_runtime::Filter::default()
    })
}

/// What the drawer draws for one row: bounded text, or a bounded `data:`
/// URL, through the store's byte-capped LRU.
#[tauri::command(async)]
pub(crate) fn artifact_preview(
    id: String,
) -> Result<Arc<artifact_runtime::PreviewPayload>, String> {
    store_or_refuse()?.preview(&id)
}

/// One gallery document for the window to open (t-16006): its text, bounded
/// like the drawer's preview, and whether the project's own file door would
/// read it — which decides whether the tab is an editable file tab or a tab
/// nobody can write.
#[derive(Serialize)]
pub(crate) struct ArtifactDocument {
    #[serde(flatten)]
    pub(crate) text: artifact_runtime::DocumentText,
    pub(crate) in_project: bool,
}

/// A document artifact's text, by id and — for a kept snapshot — version
/// number (t-16006). The gallery lists every project's documents and the
/// project's file door (`read_text_file`) refuses a path outside its root, as
/// it must; so a document outside the open project is read here, out of the
/// catalog's own row. No path comes from the window: the row supplies the
/// file, the store's own version list supplies a snapshot, and the store
/// refuses anything that is not a text document, a regular file and within
/// the preview's byte cap.
#[tauri::command(async)]
pub(crate) fn artifact_document(
    state: State<'_, AppState>,
    id: String,
    version: Option<u32>,
) -> Result<ArtifactDocument, String> {
    let store = store_or_refuse()?;
    let text = store.document_text(&id, version)?;
    let row = store
        .get(&id)
        .ok_or_else(|| "그 아티팩트가 없습니다".to_string())?;
    let in_project = crate::cmd::fs::opens_in_project(
        state.active_root(),
        crate::hooks::second_brain_vault().as_deref(),
        &row.path.to_string_lossy(),
    );
    Ok(ArtifactDocument { text, in_project })
}

/// Counts by origin, for the chips on cards, tasks and worktree rows.
#[tauri::command(async)]
pub(crate) fn artifact_counts() -> Result<artifact_runtime::Counts, String> {
    Ok(store_or_refuse()?.counts())
}

fn path_of(id: &str) -> Result<PathBuf, String> {
    let artifact = store_or_refuse()?
        .get(id)
        .ok_or_else(|| "그 아티팩트가 없습니다".to_string())?;
    if artifact.url.is_some() && artifact.path.as_os_str().is_empty() {
        return Err("claude.ai 아티팩트는 브라우저 탭에서 엽니다".into());
    }
    if !artifact.path.exists() {
        return Err("아티팩트 파일이 사라졌습니다".into());
    }
    Ok(artifact.path)
}

/// The file one row's action means: the row's own file, or — when the person
/// picked a numbered version — that immutable version's (t-3952), found in
/// the store's own list so no path is taken from the window.
fn path_of_version(id: &str, version: Option<u32>) -> Result<PathBuf, String> {
    let Some(version) = version else {
        return path_of(id);
    };
    store_or_refuse()?
        .versions(id)
        .into_iter()
        .find(|kept| kept.n == version)
        .map(|kept| kept.path)
        .ok_or_else(|| "그 버전은 더 이상 보관되지 않습니다".to_string())
}

/// The card's picture (t-3233 §3): from the cache, or from one render of
/// the hidden thumbnail pane — one at a time, no retry. A `data_url` of
/// `None` is a glyph, and final for this key.
#[tauri::command]
pub(crate) async fn artifact_thumbnail(
    app: AppHandle,
    webview: tauri::Webview,
    id: String,
) -> Result<artifact_thumbs::Thumb, String> {
    from_the_main_webview(&webview)?;
    let store = store_or_refuse()?;
    artifact_thumbs::thumbnail(&app, &store, &id).await
}

/// The listing's copy of the thumbnail numbers, so the window queues by the
/// table rather than by a number of its own.
#[derive(Serialize)]
pub(crate) struct ThumbTable {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) queue_max: usize,
}

/// The snapshots kept for one page or document (t-3233 §5), oldest first —
/// what the artifact page's version picker lists.
#[tauri::command(async)]
pub(crate) fn artifact_versions(id: String) -> Result<Vec<artifact_runtime::Version>, String> {
    Ok(store_or_refuse()?.versions(&id))
}

/// The publication a `file://` address a terminal printed points at — its
/// current file or one of its kept versions — so the window opens it as that
/// artifact, with its header band, rather than as a bare page. The store
/// judges ([`artifact_runtime::Store::page_at`]); the window never guesses
/// where the store is. `None` for anything else.
#[tauri::command(async)]
pub(crate) fn artifact_page_at(path: String) -> Result<Option<artifact_runtime::PageAt>, String> {
    Ok(store_or_refuse()?.page_at(Path::new(&path)))
}

/// 사람이 고른 판에 초안으로 넣은 주석을 그 페이지의 기록에 남긴다(t-11959).
/// 초안을 넣은 것은 창이고 이 문은 적기만 한다 — 판에 무엇을 보내지도, Enter를
/// 치지도 않는다. 줄은 스토어가 검사하고(`record_feedback`), 갤러리가 새 수를
/// 보도록 카탈로그가 움직였다고 알린다.
#[tauri::command(async)]
pub(crate) fn artifact_feedback_record(
    webview: tauri::Webview,
    feedback: artifact_runtime::FeedbackAsk,
) -> Result<artifact_runtime::FeedbackSummary, String> {
    from_the_main_webview(&webview)?;
    let summary = store_or_refuse()?.record_feedback(feedback, now_epoch_ms())?;
    if let Some(app) = artifact_runtime::window_handle() {
        let _ = app.emit(artifact_runtime::CHANGED_EVENT, ());
    }
    Ok(summary)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreferenceSnapshot {
    project_key: Option<String>,
    entries: Vec<zerocode_core::user_preferences::SavedPreference>,
    supported_agent: &'static str,
    max_text_bytes: usize,
    max_per_scope: usize,
}

fn preference_snapshot(state: &AppState) -> Result<PreferenceSnapshot, String> {
    use zerocode_core::user_preferences::{
        MAX_PER_SCOPE, MAX_TEXT_BYTES, PreferenceScope, project_key,
    };
    let current = project_key(&state.active().root);
    let entries = crate::preference_runtime::PreferenceStore::of_this_machine()?.list()?;
    Ok(PreferenceSnapshot {
        entries: entries
            .into_iter()
            .filter(|entry| match &entry.scope {
                PreferenceScope::Personal => true,
                PreferenceScope::Project { key } => Some(key) == current.as_ref(),
            })
            .collect(),
        project_key: current,
        supported_agent: "zo",
        max_text_bytes: MAX_TEXT_BYTES,
        max_per_scope: MAX_PER_SCOPE,
    })
}

#[tauri::command(async)]
pub(crate) fn artifact_preferences(
    webview: tauri::Webview,
    state: State<'_, AppState>,
) -> Result<PreferenceSnapshot, String> {
    from_the_main_webview(&webview)?;
    preference_snapshot(&state)
}

#[tauri::command(async)]
pub(crate) fn artifact_preference_save(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    feedback: artifact_runtime::FeedbackAsk,
    text: String,
    scope: String,
    expected_project: Option<String>,
) -> Result<PreferenceSnapshot, String> {
    use zerocode_core::user_preferences::{PreferenceScope, project_key};
    from_the_main_webview(&webview)?;
    let scope = match scope.as_str() {
        "personal" => PreferenceScope::Personal,
        "project" => {
            let current =
                project_key(&state.active().root).ok_or("the current project is unavailable")?;
            if expected_project.as_ref() != Some(&current) {
                return Err("the selected project changed; reopen the preference editor".into());
            }
            PreferenceScope::Project { key: current }
        }
        _ => return Err("unknown preference scope".into()),
    };
    let origin = store_or_refuse()?.feedback_origin(&feedback)?;
    crate::preference_runtime::PreferenceStore::of_this_machine()?.save(
        &text,
        scope,
        origin,
        now_epoch_ms(),
    )?;
    preference_snapshot(&state)
}

#[tauri::command(async)]
pub(crate) fn artifact_preference_revoke(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    id: String,
) -> Result<PreferenceSnapshot, String> {
    from_the_main_webview(&webview)?;
    let visible = preference_snapshot(&state)?;
    let store = crate::preference_runtime::PreferenceStore::of_this_machine()?;
    if !visible.entries.iter().any(|entry| entry.id == id)
        && store.list()?.iter().any(|entry| entry.id == id)
    {
        return Err("that preference is not in the selected project or personal scope".into());
    }
    store.revoke(&id)?;
    preference_snapshot(&state)
}

/// 머리띠의 「내보내기」: 발행물의 한 판을 사람이 고른 폴더에 새 파일로 쓴다 — HTML은
/// 복사하고, PDF와 PNG는 창의 WebKit이 숨은 판에서 그린다(t-18558). 폴더는 창이 폴더
/// 대화상자(`choose_project`)로 받아 온 것이고, 쓰는 것은 문의 내보내기와 같은 불변
/// 스냅샷이다. 있는 파일은 덮지 않는다. 형식(`html`·`pdf`·`png`)을 말하지 않으면 HTML이다.
#[tauri::command]
pub(crate) async fn artifact_export(
    webview: tauri::Webview,
    id: String,
    version: u32,
    folder: String,
    format: Option<String>,
) -> Result<zerocode_core::artifact_publish::ExportedFile, String> {
    from_the_main_webview(&webview)?;
    let format = match format.as_deref() {
        None => ExportFormat::Html,
        Some(word) => ExportFormat::from_word(word)
            .ok_or_else(|| format!("알 수 없는 내보내기 형식입니다: {word}"))?,
    };
    let store = store_or_refuse()?;
    // 그리는 일은 창의 스레드에서 도는 WebKit의 답을 기다린다. 그 기다림은 막아도 되는
    // 스레드에서만 한다 — 창의 스레드나 비동기 일꾼에서 기다리면 서로를 기다린다.
    tauri::async_runtime::spawn_blocking(move || {
        store.export_into(&id, version, Path::new(&folder), format)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 「내보내기」 메뉴가 묻는 것(t-18558): 형식마다 이 창이 쓸 수 있는가, 못 쓰면 이유의
/// 코드. 문이 거절하는 것과 같은 답이다 — 그릴 렌더러가 서 있는가.
#[tauri::command(async)]
pub(crate) fn artifact_export_formats(
    webview: tauri::Webview,
) -> Result<Vec<artifact_render::FormatChoice>, String> {
    from_the_main_webview(&webview)?;
    Ok(artifact_render::format_choices(
        store_or_refuse()?.renderer().as_deref(),
    ))
}

/// 방금 이 창이 내보낸 파일을 파일 관리자에서 보인다 — 알림의 「Finder에서 보기」.
/// 이 창이 쓴 파일만 보인다: 창이 다른 경로를 말해도 보이지 않는다.
#[tauri::command(async)]
pub(crate) fn artifact_export_reveal(webview: tauri::Webview, path: String) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let path = PathBuf::from(path);
    if !store_or_refuse()?.was_exported(&path) {
        return Err("이 창이 내보낸 파일만 보일 수 있습니다".into());
    }
    if !path.exists() {
        return Err("내보낸 파일이 사라졌습니다".into());
    }
    reveal_in_file_manager(&path)
}

/// The refresh button's road into the transcripts (t-3233 §2): the same
/// bounded, incremental pass the boot runs, on the person's ask.
#[tauri::command(async)]
pub(crate) fn artifact_import_transcripts(
    app: AppHandle,
) -> Result<artifact_transcripts::BackfillReport, String> {
    artifact_runtime::backfill_transcripts(&app)
        .ok_or_else(|| "아티팩트 스토어가 아직 열리지 않았습니다".to_string())
}

/// Open the file with the system's default application — Orca's Open.
#[tauri::command(async)]
pub(crate) fn artifact_open(id: String) -> Result<(), String> {
    let path = path_of(&id)?;
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    let mut command = crate::proc::quiet_command(launcher);
    command.arg(&path);
    zerocode_core::reap::spawn_forgotten(command)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Show the file in the file manager — Orca's Reveal in Finder. With a
/// `version`, the immutable file of that version rather than the current one.
#[tauri::command(async)]
pub(crate) fn artifact_reveal(id: String, version: Option<u32>) -> Result<(), String> {
    reveal_in_file_manager(&path_of_version(&id, version)?)
}

/// Show one file in the file manager: selected on macOS and Windows, its folder
/// on Linux. Both reveal roads (the store's file, an exported one) go through it.
fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let command = {
        let mut it = crate::proc::quiet_command("open");
        it.arg("-R").arg(path);
        it
    };
    #[cfg(target_os = "linux")]
    let command = {
        let mut it = crate::proc::quiet_command("xdg-open");
        it.arg(path.parent().unwrap_or(path));
        it
    };
    #[cfg(target_os = "windows")]
    let command = {
        let mut it = crate::proc::quiet_command("explorer");
        it.arg("/select,").arg(path);
        it
    };
    zerocode_core::reap::spawn_forgotten(command)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The absolute path, for the clipboard. The window writes the clipboard
/// through the one door it already has; this only answers the path.
#[tauri::command(async)]
pub(crate) fn artifact_copy_path(id: String) -> Result<String, String> {
    Ok(path_of(&id)?.display().to_string())
}

/// Remove one artifact — the row, and the file when it is the store's own.
/// `confirmed` is the person's answer to the dialog; without it the store
/// does nothing and answers `false`.
#[tauri::command(async)]
pub(crate) fn artifact_delete(id: String, confirmed: bool) -> Result<bool, String> {
    store_or_refuse()?.delete(&id, confirmed)
}

/// Register a file by hand — the road this feature proves itself by: the
/// worker's own report, put in the store from the window. Copied into the
/// manual bucket with no origin, because a hand-registered file has none the
/// ledger can vouch for.
#[tauri::command(async)]
pub(crate) fn artifact_register(path: String) -> Result<Artifact, String> {
    let source = PathBuf::from(&path);
    if !source.is_absolute() || !source.is_file() {
        return Err("절대 경로의 파일만 등록할 수 있습니다".into());
    }
    store_or_refuse()?.register_copy(
        &source,
        zerocode_core::artifact::Source::Manual,
        zerocode_core::artifact::Origin::default(),
        now_epoch_ms(),
    )
}
