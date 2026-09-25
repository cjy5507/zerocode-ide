//! Filesystem, local capability, and download commands.

use crate::*;

#[tauri::command]
pub(crate) async fn list_dir(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<DirEntry>, String> {
    let root = state
        .active_root()
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let target = root.join(&path);
    let target = target.canonicalize().map_err(|error| error.to_string())?;
    if !target.starts_with(&root) {
        return Err("path escapes the project".to_string());
    }
    let (entries, _) = read_dir_entries(&target, None).map_err(|error| error.to_string())?;
    Ok(entries)
}

/// One folder level anywhere on the disk, for the in-window folder browser
/// (t-2982) — the door 프로젝트 추가 → 폴더 찾아보기 opens instead of
/// AppKit's panel. A leading `~` is the home folder; the answer is cut at
/// the table's cap and says so. Names only: this reads no file's contents.
#[tauri::command]
pub(crate) async fn browse_dir(path: String) -> Result<BrowseAnswer, String> {
    let target = expand_home(&path, dirs::home_dir().as_deref());
    tauri::async_runtime::spawn_blocking(move || browse_folder(&target, FOLDER_BROWSE_ENTRY_CAP))
        .await
        .map_err(|join| join.to_string())?
}

/// Where the browser starts: the projects this machine opened last, the home
/// folder, the mounted volumes — capped by the same table.
#[tauri::command]
pub(crate) async fn browse_places(state: State<'_, AppState>) -> Result<BrowsePlaces, String> {
    let recents = stored_projects(state.config_root());
    tauri::async_runtime::spawn_blocking(move || {
        browse_places_of(
            dirs::home_dir().as_deref(),
            &mounted_volumes(),
            &recents,
            FOLDER_BROWSE_RECENT_CAP,
        )
    })
    .await
    .map_err(|join| join.to_string())
}

/// What each path the composer holds is — `file`, `folder` or `missing` —
/// for the chips' glyphs when a drop or a browser answer arrives, and for
/// the check just before a send (t-2993, docs/design/composer-attachments.md
/// §2.4). Names only; no file is read. Off the main thread like
/// [`browse_dir`]: a path on a sleeping volume must not hold the window.
#[tauri::command]
pub(crate) async fn path_kinds(paths: Vec<String>) -> Result<Vec<PathKind>, String> {
    tauri::async_runtime::spawn_blocking(move || path_kinds_of(&paths))
        .await
        .map_err(|join| join.to_string())
}

/// A file's text for the stage viewer, with the version the editor has to hand
/// back when it saves.
///
/// Same escape rule as [`list_dir`], plus two refusals of its own: a size cap
/// (a viewer that swallows a 2 GB log takes the window down with it) and a
/// binary check (NUL bytes render as garbage pretending to be text).
/// `allow_missing` is only for a restored file tab carrying a draft: omitted
/// and `false` preserve the existing read contract, while `true` may return an
/// empty, explicitly missing file at a safe address without creating it.
#[tauri::command]
pub(crate) async fn read_text_file(
    state: State<'_, AppState>,
    path: String,
    allow_missing: Option<bool>,
) -> Result<TextFile, String> {
    let root = root_of(
        state.active_root(),
        crate::hooks::second_brain_vault().as_deref(),
        &path,
    );
    read_text_in_project(&root, &path, allow_missing.unwrap_or(false))
}

/// The root a path the window hands over stands in: the project, or — when
/// one is saved and the path lives there — the second brain's vault.
///
/// Existing files keep first claim in the same order as before. Only after
/// neither root resolves an existing path do safe missing addresses take part,
/// so a relative vault file is not mistaken for a missing project file. The
/// fallback remains the project; the command's resolver then returns the same
/// containment refusal for a path in neither root.
pub(crate) fn root_of(active: PathBuf, vault: Option<&str>, path: &str) -> PathBuf {
    if resolve_in_project(&active, path).is_ok() {
        return active;
    }
    if let Some(vault) = vault
        && resolve_in_project(Path::new(vault), path).is_ok()
    {
        return PathBuf::from(vault);
    }
    if resolve_new_in_project(&active, path).is_ok() {
        return active;
    }
    match vault {
        Some(vault) if resolve_new_in_project(Path::new(vault), path).is_ok() => {
            PathBuf::from(vault)
        }
        _ => active,
    }
}

/// What a file is right now, without its text.
///
/// The narrow half of [`read_text_file`], for asking "has this moved" when a
/// tab is brought forward. It still reads the file — the version has to be
/// comparable with the one the tab is holding, and that one carries a digest —
/// but it hands back forty bytes instead of half a megabyte, and the file it
/// reads is the one already on screen.
#[tauri::command]
pub(crate) async fn file_version(
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let root = root_of(
        state.active_root(),
        crate::hooks::second_brain_vault().as_deref(),
        &path,
    );
    let target = resolve_in_project(&root, &path)?;
    stamp_of(&target)
}

/// Write a file back, for the editor surface.
///
/// The same escape rule and the same cap as the read: a path that could not
/// be read here cannot be written here either.
///
/// Written beside the file and renamed over it, rather than opened and
/// truncated. A write that fails halfway through the direct route leaves the
/// person's file half-gone — and the half that survives is the half they were
/// editing away from. The rename is atomic on both targets, so the file is
/// either the old one or the new one and never neither.
///
/// `create_if_missing` is the window reporting a deletion it was told about,
/// not a licence to create — see [`save_intent`]. It is a plain `bool` rather
/// than an optional one on purpose: a window that stopped sending it would be
/// refused loudly instead of quietly saving under whatever the default became.
#[tauri::command]
pub(crate) async fn write_text_file(
    state: State<'_, AppState>,
    path: String,
    text: String,
    version: String,
    create_if_missing: bool,
) -> Result<String, String> {
    let root = root_of(
        state.active_root(),
        crate::hooks::second_brain_vault().as_deref(),
        &path,
    );
    save_text_at(&root, &path, &text, &version, create_if_missing)
}

/// Replace the set of files being watched for outside writes.
///
/// The window sends the WHOLE set — every open file tab — each time any tab
/// opens or closes, rather than add/remove deltas: the set is derived state,
/// and a full replacement cannot drift from what is actually open.
///
/// Each path is resolved here, behind the same [`resolve_in_project`] rule as
/// every other file command, so an escaping path never enters the watch set.
/// A path that fails to resolve is handed over as address-less rather than
/// refused: the usual reason is a file deleted while its tab stayed open,
/// and [`file_watch::WatchSet::replace`] keeps the address such an entry
/// already had so the file coming back is still seen.
#[tauri::command(async)]
pub(crate) fn watch_files(state: State<'_, AppState>, paths: Vec<String>) {
    let root = state.active_root();
    state.watched().replace(
        paths
            .into_iter()
            .map(|path| {
                let resolved = resolve_in_project(&root, &path).ok();
                (path, resolved)
            })
            .collect(),
    );
}

/// An image from the project, for Orca's `ImageViewer` spot.
///
/// The cap is larger than the text one and smaller than a camera's output: a
/// `data:` URL costs a third again in base64 and lives in the webview's
/// memory until the tab closes, so this is the size at which showing the
/// picture is still cheaper than not having a viewer.
#[tauri::command]
pub(crate) async fn read_image_file(
    state: State<'_, AppState>,
    path: String,
) -> Result<ImageFile, String> {
    use base64::Engine as _;
    const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
    let target = resolve_in_project(&state.active_root(), &path)?;
    let extension = target
        .extension()
        .map(|held| held.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mime = IMAGE_TYPES
        .iter()
        .find(|(name, _)| *name == extension)
        .map(|(_, mime)| *mime)
        .ok_or("이 창이 그릴 수 있는 이미지 형식이 아닙니다")?;
    let meta = std::fs::metadata(&target).map_err(|error| error.to_string())?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "이미지가 너무 큽니다 ({} MB)",
            meta.len() / 1_000_000
        ));
    }
    let bytes = std::fs::read(&target).map_err(|error| error.to_string())?;
    Ok(ImageFile {
        mime,
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        bytes: meta.len(),
    })
}

/// New File / New Folder (:150-157) — 이미 있는 이름은 만들지 않는다:
/// 조용한 truncate는 방금 그 파일을 쓰던 손의 것을 지운다.
#[tauri::command(async)]
pub(crate) fn fs_create(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    name: String,
    kind: String,
) -> Result<String, String> {
    if !file_tree_ops::valid_leaf_name(&name) {
        return Err("파일 이름으로 쓸 수 없습니다".into());
    }
    let root = explorer_runtime::root(&state, None)?;
    let policy = explorer_runtime::policy(&state)?;
    let mut history = explorer_runtime::history(&state);
    let seat = file_tree_ops::checked_path(&root, &root.join(&dir).join(&name))?;
    if seat.exists() {
        return Err("같은 이름이 이미 있습니다".into());
    }
    match kind.as_str() {
        "folder" => std::fs::create_dir_all(&seat).map_err(|error| error.to_string())?,
        _ => {
            if let Some(parent) = seat.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::File::create_new(&seat).map_err(|error| error.to_string())?;
        }
    }
    let operation = history.created(&root, &seat, file_tree_ops::OpKind::Create, &policy)?;
    drop(history);
    explorer_runtime::announce(&app, &root, &operation);
    Ok(seat.display().to_string())
}

/// Rename (:296-304) — 같은 방 안에서 이름만. 목적지가 서 있으면 거절:
/// rename은 이동이 아니라 개명이고, 덮어쓰기는 개명이 아니다.
#[tauri::command(async)]
pub(crate) fn fs_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    name: String,
) -> Result<String, String> {
    if !file_tree_ops::valid_leaf_name(&name) {
        return Err("파일 이름으로 쓸 수 없습니다".into());
    }
    let root = explorer_runtime::root(&state, None)?;
    let policy = explorer_runtime::policy(&state)?;
    let from = file_tree_ops::checked_path(&root, Path::new(&path))?;
    let to = from
        .parent()
        .ok_or("루트는 이름을 바꿀 수 없습니다")?
        .join(&name);
    if to.exists() {
        return Err("같은 이름이 이미 있습니다".into());
    }
    let operation = explorer_runtime::history(&state).relocate(
        &root,
        &[(from, to.clone())],
        file_tree_ops::OpKind::Rename,
        file_tree_ops::Origin::Human,
        &policy,
    )?;
    explorer_runtime::announce(&app, &root, &operation);
    Ok(to.display().to_string())
}

/// Delete (:305-309) — 휴지통으로, 영영이 아니라. Orca도 trash를 거치고,
/// 우클릭 한 번의 실수가 복구 불능이어서는 안 된다.
#[tauri::command(async)]
pub(crate) fn fs_trash(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<(), String> {
    let root = explorer_runtime::root(&state, None)?;
    let policy = explorer_runtime::policy(&state)?;
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let operation = explorer_runtime::history(&state).trash(&root, &paths, &policy)?;
    explorer_runtime::announce(&app, &root, &operation);
    Ok(())
}

/// One command and one history entry for the entire selection.
#[tauri::command(async)]
pub(crate) fn fs_move(
    state: State<'_, AppState>,
    paths: Vec<String>,
    dir: String,
    root: Option<String>,
) -> Result<file_tree_ops::TreeOp, String> {
    let root = explorer_runtime::root(&state, root.as_deref())?;
    let policy = explorer_runtime::policy(&state)?;
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let operation = explorer_runtime::history(&state).move_group(
        &root,
        &paths,
        Path::new(&dir),
        file_tree_ops::Origin::Human,
        &policy,
    )?;
    Ok(operation)
}

#[tauri::command(async)]
pub(crate) fn fs_undo(
    app: AppHandle,
    state: State<'_, AppState>,
    root: Option<String>,
) -> Result<file_tree_ops::TreeOp, String> {
    let root = explorer_runtime::root(&state, root.as_deref())?;
    file_tree_hooks::reconcile(&app, &state);
    explorer_runtime::history(&state).undo(&root)
}

#[tauri::command(async)]
pub(crate) fn fs_redo(
    state: State<'_, AppState>,
    root: Option<String>,
) -> Result<file_tree_ops::TreeOp, String> {
    let root = explorer_runtime::root(&state, root.as_deref())?;
    explorer_runtime::history(&state).redo(&root)
}

/// Duplicate (:189-194) — Finder의 사다리(`name copy.ext`)를 오르는 사본.
#[tauri::command(async)]
pub(crate) fn fs_duplicate(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let root = explorer_runtime::root(&state, None)?;
    let policy = explorer_runtime::policy(&state)?;
    let mut history = explorer_runtime::history(&state);
    let from = file_tree_ops::checked_path(&root, Path::new(&path))?;
    if !from.is_file() {
        return Err("파일만 복제할 수 있습니다".into());
    }
    let to = file_tree_ops::duplicate_name(&from, |candidate| candidate.exists())
        .ok_or("복제 이름을 지을 수 없습니다")?;
    let mut source = std::fs::File::open(&from).map_err(|error| error.to_string())?;
    let mut target = std::fs::File::create_new(&to).map_err(|error| error.to_string())?;
    if let Err(error) = std::io::copy(&mut source, &mut target) {
        let _ = std::fs::remove_file(&to);
        return Err(error.to_string());
    }
    let operation = history.created(&root, &to, file_tree_ops::OpKind::Duplicate, &policy)?;
    drop(history);
    explorer_runtime::announce(&app, &root, &operation);
    Ok(to.display().to_string())
}

/// Whether each of these paths is really there.
///
/// The original asks exactly this of every link candidate and **does not make
/// a link at all** when the answer is no (`pathExists: existsSync`,
/// main/index.ts:611, dropped at terminal-link-handlers.ts:170-181). That
/// filter is what makes its broad guessing safe, and we have been making links
/// for paths that do not exist — a link that opens nothing is worse than no
/// link, because it spends a person's click to say nothing.
///
/// Batched: the window asks about a screenful at once, and a round trip per
/// path would be a round trip per painted row.
///
/// Beyond the cap the answer is simply SHORTER than the question, and the
/// caller must read a missing answer as "not answered" rather than "not
/// there". Answering `false` was the first shape here and it was wrong: the
/// window records what it hears, so a manufactured `false` would be cached and
/// the path never asked again — an existing file would lose its link for the
/// rest of the session. A question re-asked next paint costs one stat; a lie
/// cached costs the feature.
///
/// No fence. `fenced_path` exists to stop a WRITE from leaving the worktree,
/// and this reads nothing: it asks the kernel a yes-or-no about a name the
/// screen already showed the person. Fencing it would refuse to answer for
/// `/usr/lib/...` printed by a compiler, and then that path could never be a
/// link — the fence would be doing the opposite of its job.
///
/// On the blocking pool because a stat can block for as long as a network
/// mount feels like, and this is on the way to a paint.
///
/// The stat follows symlinks, so a broken link answers `false` — the same
/// answer `existsSync` gives, and the honest one: opening it would fail.
/// One stat road with [`path_kinds`]: this is that answer, read as yes-or-no.
#[tauri::command]
pub(crate) async fn paths_exist(paths: Vec<String>) -> Vec<bool> {
    tauri::async_runtime::spawn_blocking(move || {
        let asked: Vec<String> = paths.into_iter().take(TERM_LINK_PROBE_CAP).collect();
        path_kinds_of(&asked)
            .into_iter()
            .map(|kind| kind != PathKind::Missing)
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Reveal (:272-294) — OS 파일 관리자에서 그 자리를 연다. reveal_skill과
/// 같은 launcher, 담장만 다르다(스킬 디렉터리가 아니라 워크트리).
#[tauri::command(async)]
pub(crate) fn fs_reveal(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let seat = fenced_path(&state, &path)?;
    if !seat.exists() {
        return Err("그 경로가 없습니다".into());
    }
    #[cfg(target_os = "macos")]
    let command = {
        let mut it = crate::proc::quiet_command("open");
        it.arg("-R").arg(&seat);
        it
    };
    #[cfg(target_os = "linux")]
    let command = {
        let mut it = crate::proc::quiet_command("xdg-open");
        it.arg(seat.parent().unwrap_or(&seat));
        it
    };
    #[cfg(target_os = "windows")]
    let command = {
        let mut it = crate::proc::quiet_command("explorer");
        it.arg("/select,").arg(&seat);
        it
    };
    // fire-and-forget 자식은 reap 경유 — 읽는 이 없는 종료 코드가 좀비가
    // 되지 않게(a_child_nobody_reads_is_still_collected의 계약).
    zerocode_core::reap::spawn_forgotten(command)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// 파일을 시스템 기본 앱으로 연다 — 터미널 파일 링크 팝오버의 alternate와
/// ⇧⌘ 직행이 부르는 문 (Orca `openDetectedFilePath`의
/// `openWithSystemDefault`, terminal-file-link-actions.ts:32-38,70-91).
///
/// [`fs_reveal`]과 같은 담장(워크트리 안), 같은 세 launcher, 같은 reap.
/// "여는" 문이 정당한 것도 같은 이유다: 사람이 자기 터미널 출력의 제
/// 경로를 눌렀고, 담장이 그 경로를 체크아웃 안으로 묶는다.
#[tauri::command(async)]
pub(crate) fn fs_open_default(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let seat = fenced_path(&state, &path)?;
    if !seat.exists() {
        return Err("그 경로가 없습니다".into());
    }
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    let mut command = crate::proc::quiet_command(launcher);
    command.arg(&seat);
    zerocode_core::reap::spawn_forgotten(command)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn open_download(app: AppHandle, path: String) -> Result<(), String> {
    let real = downloaded_file_checked(&app, &path)?;
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    let mut opener = crate::proc::quiet_command(launcher);
    opener.arg(&real);
    zerocode_core::reap::spawn_forgotten(opener)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn show_download(app: AppHandle, path: String) -> Result<(), String> {
    let real = downloaded_file_checked(&app, &path)?;
    #[cfg(target_os = "macos")]
    let (launcher, args): (&str, Vec<&str>) = ("open", vec!["-R"]);
    #[cfg(target_os = "linux")]
    let (launcher, args): (&str, Vec<&str>) = ("xdg-open", Vec::new());
    #[cfg(target_os = "windows")]
    let (launcher, args): (&str, Vec<&str>) = ("explorer", vec!["/select,"]);
    #[cfg(target_os = "linux")]
    let shown = real.parent().unwrap_or(&real).to_path_buf();
    #[cfg(not(target_os = "linux"))]
    let shown = real;
    let mut opener = crate::proc::quiet_command(launcher);
    opener.args(args).arg(&shown);
    zerocode_core::reap::spawn_forgotten(opener)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// One frame of the page, as a PNG data URL — the markup editor's base image
/// (B02 ②). wry exposes no snapshot API on macOS, but tauri's `with_webview`
/// hands over the raw WKWebView and `takeSnapshotWithConfiguration:` is
/// WebKit's own answer to "what does this page look like" — a nil
/// configuration means the visible viewport, as drawn. The completion lands
/// on the main thread; a channel carries the bytes back to this async
/// command with a deadline, so a wedged page cannot hold the invoke forever.
#[tauri::command]
pub(crate) async fn browser_snapshot(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    label: String,
) -> Result<String, String> {
    from_the_main_webview(&webview)?;
    let bytes = browser_snapshot_png(&app, &state, &label).await?;
    use base64::Engine as _;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// The caller is authenticated before selecting the pane's native snapshot engine.
pub(crate) async fn browser_snapshot_png(
    app: &AppHandle,
    state: &AppState,
    label: &str,
) -> Result<Vec<u8>, String> {
    let pane = browser_pane_of(app, state, label)?;
    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        pane.snapshot_png(move |answer| {
            let _ = tx.send(answer);
        })?;
        tokio::time::timeout(SNAPSHOT_BUDGET, rx)
            .await
            .map_err(|_| "스냅샷 응답 시간이 초과됐습니다".to_string())?
            .map_err(|_| "스냅샷 응답이 끊겼습니다".to_string())?
    }
    #[cfg(not(all(target_os = "macos", feature = "chromium-browser")))]
    snapshot_webview_png(pane, None).await
}

/// Snapshot an IDE-owned Tauri webview, including the artifact thumbnail pane.
/// `width` is in points; `None` preserves the view's current size.
pub(crate) async fn snapshot_webview_png(
    pane: tauri::Webview,
    width: Option<f64>,
) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u8>, String>>();
        pane.with_webview(move |platform| {
            use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
            use objc2_foundation::NSDictionary;
            // Main thread, by `with_webview`'s contract — WebKit requires it.
            let view: &objc2_web_kit::WKWebView = unsafe { &*platform.inner().cast() };
            let block = block2::RcBlock::new(
                move |image: *mut objc2_app_kit::NSImage, error: *mut objc2_foundation::NSError| {
                    let answer = if image.is_null() {
                        Err(if error.is_null() {
                            "스냅샷이 비어 왔습니다".to_string()
                        } else {
                            unsafe { &*error }.localizedDescription().to_string()
                        })
                    } else {
                        // TIFF is the representation NSImage always holds;
                        // the bitmap rep re-encodes it as the PNG the <img>
                        // side of the editor actually wants.
                        unsafe { &*image }
                            .TIFFRepresentation()
                            .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                            .and_then(|rep| unsafe {
                                rep.representationUsingType_properties(
                                    NSBitmapImageFileType::PNG,
                                    &NSDictionary::new(),
                                )
                            })
                            .map(|png| png.to_vec())
                            .ok_or_else(|| "PNG 인코딩에 실패했습니다".to_string())
                    };
                    let _ = tx.send(answer);
                },
            );
            // A width asked for is WebKit's own downscale — the thumbnail
            // road renders a page at reading size and takes it card-sized.
            // Main thread, by `with_webview`'s contract: the marker is the
            // compiler's word for what the closure already knows.
            let mtm = unsafe { objc2::MainThreadMarker::new_unchecked() };
            let configuration = width.map(|points| {
                let configuration = unsafe { objc2_web_kit::WKSnapshotConfiguration::new(mtm) };
                unsafe {
                    configuration
                        .setSnapshotWidth(Some(&objc2_foundation::NSNumber::new_f64(points)));
                }
                configuration
            });
            unsafe {
                view.takeSnapshotWithConfiguration_completionHandler(
                    configuration.as_deref(),
                    &block,
                );
            }
        })
        .map_err(|error| error.to_string())?;
        let bytes = tauri::async_runtime::spawn_blocking(move || {
            rx.recv_timeout(SNAPSHOT_BUDGET)
                .map_err(|_| "스냅샷이 10초 안에 오지 않았습니다".to_string())
                .and_then(|inner| inner)
        })
        .await
        .map_err(|error| error.to_string())??;
        Ok(bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pane, width);
        Err("이 플랫폼은 아직 페이지 스냅샷을 지원하지 않습니다".to_string())
    }
}

/// The composed markup, onto the clipboard as an image. The window sends raw
/// RGBA (base64) with its claimed dimensions — the clipboard plugin wants
/// pixels, not a PNG — and the length is checked against the claim, because
/// a mismatched buffer is how an image constructor reads out of bounds.
#[tauri::command(async)]
pub(crate) fn set_clipboard_image(
    app: AppHandle,
    webview: tauri::Webview,
    rgba: String,
    width: u32,
    height: u32,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(rgba.as_bytes())
        .map_err(|error| error.to_string())?;
    let wanted = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("이미지가 주소 공간보다 큽니다")?;
    if bytes.len() != wanted {
        return Err(format!(
            "픽셀 수가 주장과 다릅니다: {} != {wanted}",
            bytes.len()
        ));
    }
    use tauri_plugin_clipboard_manager::ClipboardExt as _;
    app.clipboard()
        .write_image(&tauri::image::Image::new_owned(bytes, width, height))
        .map_err(|error| error.to_string())
}

/// is revealable without a second list to keep in step.
#[tauri::command(async)]
pub(crate) fn reveal_vault_session(path: String) -> Result<(), String> {
    let target = std::path::PathBuf::from(&path);
    if !target.is_file() {
        return Err("그 경로에 파일이 없습니다".into());
    }
    let real = target.canonicalize().map_err(|error| error.to_string())?;
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or("HOME이 설정되어 있지 않습니다")?;
    let walked = zerocode_core::vault::AGENT_SOURCES
        .iter()
        .flat_map(|source| {
            zerocode_core::vault::roots(
                source,
                &home,
                zerocode_core::vault::env_home(source).as_deref(),
            )
        });
    // OpenCode's card names its database, which stands in a data directory
    // rather than under any session root — without this, the one agent whose
    // sessions are rows would be the one agent whose card cannot be revealed.
    let allowed = walked
        .chain(opencode_home::data_dirs())
        .any(|root| root.canonicalize().is_ok_and(|root| real.starts_with(root)));
    if !allowed {
        return Err("세션 디렉터리 안의 파일만 열 수 있습니다".into());
    }
    #[cfg(target_os = "macos")]
    let (launcher, args): (&str, Vec<&str>) = ("open", vec!["-R"]);
    #[cfg(target_os = "linux")]
    let (launcher, args): (&str, Vec<&str>) = ("xdg-open", Vec::new());
    #[cfg(target_os = "windows")]
    let (launcher, args): (&str, Vec<&str>) = ("explorer", vec!["/select,"]);
    #[cfg(target_os = "linux")]
    let shown = real.parent().unwrap_or(&real).to_path_buf();
    #[cfg(not(target_os = "linux"))]
    let shown = real;
    let mut opener = crate::proc::quiet_command(launcher);
    opener.args(args).arg(&shown);
    // The same reaping door as `open_url`, for the same zombie.
    zerocode_core::reap::spawn_forgotten(opener)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Every session these agents have ever had, as the vault panel shows it.
///
/// Orca's `AiVaultPanel`. Read-only in the strongest sense: no function in
/// `zerocode_core::vault` writes, so there is nothing here that could damage a
/// vendor's conversation history — see that module for why that is the design and
/// not a limitation.
///
/// The scan and the view are one command rather than two. A window that fetched a
/// list and then filtered it would hold two hundred sessions in the webview and
/// answer a keystroke with a JS sort; this way the window holds only what it
/// draws, and the header counts come from the same function as the rows.
///
/// `force` is the reload button, and only it. Every other ask — a keystroke, a
/// grouping, a sort, a filter — changes the QUERY and nothing on disk, and is
/// answered from the walk this process already has ([`vault_walk`]). The panel's
/// own comment promised that from the day it was written; what it kept was the
/// VIEW, which is the answer to one query, so the next keystroke walked
/// fourteen directory trees again to ask a different one.
#[tauri::command]
pub(crate) async fn vault_sessions(
    state: State<'_, AppState>,
    mut query: zerocode_core::vault::VaultQuery,
    force: bool,
) -> Result<zerocode_core::vault::VaultView, String> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or("HOME이 설정되어 있지 않습니다")?;
    // `scan` interprets its argument as HOME and Claude's source as
    // `.claude/projects`. The app config root therefore resolves exactly to
    // the isolated runtime, while the ordinary home remains the source for
    // every other agent.
    let app_home = state.config_root().to_path_buf();
    // The settings facts the rows need, read BEFORE the blocking hop: the
    // repository handle does not cross threads, and the bases are the same
    // for every session of an agent.
    let mut bases: HashMap<&'static str, String> = HashMap::new();
    for slug in zerocode_core::vault::shown_slugs() {
        if bases.contains_key(slug) {
            continue;
        }
        if let Some(base) = vault_resume_base(state.settings(), slug)? {
            bases.insert(slug, base);
        }
    }
    // The workspaces this window manages, so a card can be filed under the name
    // the sidebar gives its checkout rather than under a bare directory. Read
    // here for the reason the bases are: the catalog does not cross threads.
    let worktrees = managed_worktrees(&state);
    // The walk this process already has, unless the reload button asked for
    // another one. A walk is fourteen directory trees over stores that can hold
    // years of transcripts, plus a database — measured at ~400ms for two
    // hundred sessions on a machine with about eight thousand of them — and a
    // keystroke changes none of what is on disk.
    if query.limit.is_none() {
        query.limit = Some(
            load_settings(state.settings())?
                .document
                .vault_session_limit,
        );
    }
    let kept = vault_walk::kept(force);
    let roster = if kept.is_none() {
        vault_roster_snapshot(&state)
    } else {
        Vec::new()
    };
    tauri::async_runtime::spawn_blocking(move || {
        let walk = kept.unwrap_or_else(|| {
            let limit = zerocode_core::vault::Limits::DEFAULT.absolute_max;
            let (mut walked, mut issues, system_truncated) =
                zerocode_core::vault::scan(&home, limit);
            walked.retain(|session| session.agent != "claude");
            issues.retain(|issue| issue.agent != "claude");

            let (app_walked, app_issues, app_truncated) =
                zerocode_core::vault::scan(&app_home, limit);
            let app_claude = app_walked
                .into_iter()
                .filter(|session| session.agent == "claude")
                .collect();
            issues.extend(
                app_issues
                    .into_iter()
                    .filter(|issue| issue.agent == "claude"),
            );
            let (walked, claude_dropped) = zerocode_core::vault::merge(walked, app_claude, limit);
            // OpenCode keeps its sessions in a database rather than in files, so
            // it has no root to walk and cannot be found by the loop above — see
            // `zerocode_core::vault_opencode`. Read here rather than inside the
            // scan because the reader is this crate's: core carries no
            // `rusqlite`.
            let found = vault_opencode_scan::scan(limit);
            issues.extend(found.issues);
            let (mut sessions, dropped) =
                zerocode_core::vault::merge(walked, found.sessions, limit);
            zerocode_core::vault::read_roster_children(&mut sessions, &roster, &mut issues);
            let walk = vault_walk::Walk {
                sessions,
                issues,
                truncated: system_truncated || app_truncated || claude_dropped || dropped,
            };
            vault_walk::keep(walk.clone());
            walk
        });
        let vault_walk::Walk {
            mut sessions,
            issues,
            truncated,
        } = walk;
        // The card's command wears the resolved base. Rewritten here and not in
        // the scan because the scan is settings-blind on purpose — and on every
        // call rather than into the kept walk, so a launch override somebody
        // edits in settings reaches the next card without a reload.
        for session in &mut sessions {
            if let Some(base) = bases.get(session.agent.as_str()) {
                session.resume =
                    zerocode_core::vault::resume_command_with_base(session, Some(base));
            }
        }
        zerocode_core::vault::view(sessions, &query, issues, truncated, &worktrees)
    })
    .await
    .map_err(|error| error.to_string())
}

/// The retained, session-owned roster is the authority for zo children.
/// Worker ids come from the ledger's current seat, never from helper names.
fn vault_roster_snapshot(state: &AppState) -> Vec<zerocode_core::vault::ChildTranscript> {
    let workers = orchestration::ledger_agents();
    let sessions = state.pane_sessions().clone();
    let rosters = state.subagents();
    rosters
        .iter()
        .flat_map(|(term, rows)| {
            let parent = sessions.get(term);
            let worker = workers.iter().find(|worker| worker.term == Some(*term));
            rows.iter().filter_map(move |row| {
                let path = row.transcript.as_ref()?;
                let parent = row
                    .registry
                    .as_ref()
                    .or_else(|| parent.map(|session| &session.id))?;
                Some(zerocode_core::vault::ChildTranscript {
                    id: row.id.clone(),
                    parent: parent.clone(),
                    path: path.clone(),
                    origin: zerocode_core::vault::ChildOrigin {
                        pane: Some(format!("term-{term}")),
                        worker: worker.map(|worker| worker.worker.clone()),
                        tool_call_id: None,
                    },
                })
            })
        })
        .collect()
}

/// Open a past session in a new terminal, in the workspace it was dropped on.
///
/// The command comes from the SESSION, not from the window: `resume` was built by
/// the same code that knows each agent's spelling, so a window cannot compose
/// `codex --resume` (an error) out of a slug and a flag. A session with no
/// resumable command has no button, so there is nothing to refuse here — but the
/// check stays, because "the window only sends what we sent it" is an assumption
/// about a channel a webview can reach.
///
/// `worktree` is the workspace a card was dragged onto, and naming one is the
/// whole difference between the two gestures: the 다시 열기 button names none
/// and the conversation continues where it ran, a drop names one and it
/// continues THERE. The path is resolved through the catalog rather than taken
/// as given, for the reason [`open_lane`] resolves its own — this becomes a
/// process's working directory, and a webview naming a path must never be
/// enough to start a shell inside it.
#[tauri::command]
pub(crate) async fn resume_vault_session(
    state: State<'_, AppState>,
    session: zerocode_core::vault::VaultSession,
    worktree: Option<String>,
    rows: u16,
    cols: u16,
) -> Result<TermId, String> {
    // The same base the panel's rows wore (`vault_resume_base`), so the
    // equality check below compares like with like.
    let base = vault_resume_base(state.settings(), &session.agent)?;
    let expected = zerocode_core::vault::resume_command_with_base(&session, base.as_deref())
        .ok_or("이 세션은 이 창이 다시 열 수 없습니다")?;
    let command = session
        .resume
        .as_deref()
        .filter(|sent| *sent == expected)
        .ok_or("이 세션은 이 창이 다시 열 수 없습니다")?;

    let dropped = match worktree {
        Some(requested) => Some(
            match known_workspace_context(state.config_root(), &requested)? {
                KnownWorkspace::Git(_, chosen) => chosen.path,
                KnownWorkspace::Folder(folder) => folder,
            },
        ),
        None => None,
    };
    // The dropped workspace, then the session's own directory, then where this
    // window is standing — `zerocode_core::vault::resume_root`, where the order
    // and its fall-throughs are tested. A directory that has been deleted falls
    // through rather than failing: the `cd` inside the command will report it,
    // which is a message about the real problem.
    let target = dropped.as_ref().map(|path| path.to_string_lossy());
    let here = state.active_root().to_string_lossy().into_owned();
    let root = std::path::PathBuf::from(zerocode_core::vault::resume_root(
        target.as_deref(),
        session.cwd.as_deref(),
        &here,
        |path| std::path::Path::new(path).is_dir(),
    ));

    // A dropped workspace has to reach the COMMAND as well as the process. The
    // resume line opens with a `cd` into the session's own directory, so a shell
    // started in the dropped workspace would walk straight back out of it and
    // the drop would be a gesture that changed nothing anybody could see. (Orca
    // stops here — its drop moves the tab's workspace and leaves that `cd`
    // alone; a half-kept promise is the one shape this repository refuses.)
    //
    // Asked of the same builder with the workspace in place of the session's
    // directory, rather than edited into the validated string: a `cd` swapped by
    // hand is a second speller of a line `zerocode_core::vault` already knows
    // how to write, and the two would drift.
    let elsewhere = dropped.filter(|target| target.as_path() == root.as_path());
    let command = match elsewhere {
        Some(target) => {
            let mut moved = session.clone();
            moved.cwd = Some(target.to_string_lossy().into_owned());
            zerocode_core::vault::resume_command_with_base(&moved, base.as_deref())
                .ok_or("이 세션은 이 창이 다시 열 수 없습니다")?
        }
        None => command.to_string(),
    };

    let term = state.take_term_id();
    let launch_token = new_launch_token(term);
    let mut env = hooks::pty_env(&hooks::pane_key_of(term), Some(&launch_token), &root, None);
    env.extend(account_env_for(state.config_root(), &session.agent)?);
    env.extend(hooks::agent_launch_env(
        state.local_data_root(),
        &session.agent,
    ));
    // Through a login shell, because the command is a `cd … && agent …` line and
    // the agent's own binary may only be on the PATH a login shell builds.
    let pty = PtyLane::spawn(
        "/bin/sh",
        &["-lc".to_string(), command],
        Some(&root),
        &env,
        rows,
        cols,
    )
    .map_err(|error| error.to_string())?;
    note_window_event(
        state.local_data_root(),
        &format!("term {term} spawned /bin/sh -lc (vault resume)"),
    );
    state.hold_terminal(term, pty);
    if let Some(kind) = zerocode_core::AgentKind::from_slug(&session.agent) {
        state.agent_terms().insert(term, kind.slug());
    }
    note_pane_account(&state, term, &env);
    state.launch_tokens().insert(term, launch_token);
    state.cadence().wake();
    Ok(term)
}

#[tauri::command]
pub(crate) fn orchestration_runtime_state() -> Result<std::sync::Arc<serde_json::Value>, String> {
    let _crumb = crate::crumbs::Command::enter("orchestration_runtime_state");
    orchestration::runtime_report()
}

#[tauri::command]
pub(crate) async fn computer_use_permission_status()
-> Result<zerocode_core::computer_use::ComputerPermissionReport, String> {
    tauri::async_runtime::spawn_blocking(computer_use::permission_status)
        .await
        .map_err(|join| join.to_string())
}

#[tauri::command]
pub(crate) async fn open_computer_use_permission(
    id: Option<String>,
    reset: Option<bool>,
) -> Result<zerocode_core::computer_use::ComputerPermissionSetup, String> {
    let id = id
        .map(|id| id.parse::<zerocode_core::computer_use::ComputerPermissionId>())
        .transpose()?;
    tauri::async_runtime::spawn_blocking(move || {
        computer_use::setup_permission(id, reset.unwrap_or(false))
    })
    .await
    .map_err(|join| join.to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn reset_computer_use_permissions()
-> Result<zerocode_core::computer_use::ComputerPermissionReset, String> {
    tauri::async_runtime::spawn_blocking(computer_use::reset_permissions)
        .await
        .map_err(|join| join.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn computer_use_capabilities() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| computer_use::call("handshake", serde_json::json!({})))
        .await
        .map_err(|join| join.to_string())?
        .map_err(|error| error.to_string())
}

/// One image as it was at the last commit, beside the one on disk.
///
/// Orca's `ImageDiffViewer` (ImageDiffViewer--8SYSDMU.js): two labelled panes,
/// Original and Modified, each an image viewer — or the words "no preview" where
/// that side does not exist. A text diff of a PNG says nothing anybody can read,
/// which is the whole reason the surface exists.
///
/// Either side may be absent and that is not an error: an added file has no
/// committed version, a deleted one has nothing on disk. The window draws the
/// pane empty and says so.
#[tauri::command]
pub(crate) async fn image_diff(
    state: State<'_, AppState>,
    path: String,
) -> Result<ImageDiff, String> {
    use base64::Engine as _;
    const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
    let here = state.active();
    let orchestrator = here.orchestrator.ok_or(NOT_A_REPOSITORY)?;
    // The same escape rule as every other read: a path that could leave the
    // project is refused before git is asked about it.
    let target = resolve_in_project(&here.root, &path)?;
    let extension = target
        .extension()
        .map(|held| held.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mime = IMAGE_TYPES
        .iter()
        .find(|(name, _)| *name == extension)
        .map(|(_, mime)| *mime)
        .ok_or("이 창이 그릴 수 있는 이미지 형식이 아닙니다")?;
    let encode = |bytes: Vec<u8>| -> Option<String> {
        // The cap is per side, and a file over it is reported as absent rather
        // than truncated: half a PNG is a broken image, not a smaller one.
        (bytes.len() <= MAX_IMAGE_BYTES)
            .then(|| base64::engine::general_purpose::STANDARD.encode(&bytes))
    };
    let committed = orchestrator
        .show(&here.root, "HEAD", &path)
        .map_err(|error| error.to_string())?
        .and_then(encode);
    let working = std::fs::read(&target).ok().and_then(encode);
    Ok(ImageDiff {
        mime,
        committed,
        working,
    })
}

/// Lay out a Mermaid diagram for Orca's `MermaidViewer` spot.
///
/// Takes the text the window already holds rather than a path, because the same
/// call serves both places a diagram appears: a `.mmd` file's rich view, and a
/// ```mermaid fence inside a markdown file, which has no path of its own.
///
/// No state and no filesystem — it is a pure function behind a command so the
/// layout runs in Rust instead of in the webview. What comes back is a list of
/// placed marks, never markup: see `zerocode_core::mermaid`.
#[tauri::command]
pub(crate) async fn render_mermaid(source: String) -> zerocode_core::mermaid::MermaidRender {
    zerocode_core::mermaid::render(&source)
}
