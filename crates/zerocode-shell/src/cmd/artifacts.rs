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

/// What one listing answers, plus the retention the sweep is running with —
/// the drawer shows it beside the file's age.
#[derive(Serialize)]
pub(crate) struct ArtifactListing {
    pub(crate) rows: Vec<Artifact>,
    pub(crate) total: usize,
    pub(crate) truncated: bool,
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
    let path = path_of_version(&id, version)?;
    #[cfg(target_os = "macos")]
    let command = {
        let mut it = crate::proc::quiet_command("open");
        it.arg("-R").arg(&path);
        it
    };
    #[cfg(target_os = "linux")]
    let command = {
        let mut it = crate::proc::quiet_command("xdg-open");
        it.arg(path.parent().unwrap_or(&path));
        it
    };
    #[cfg(target_os = "windows")]
    let command = {
        let mut it = crate::proc::quiet_command("explorer");
        it.arg("/select,").arg(&path);
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
