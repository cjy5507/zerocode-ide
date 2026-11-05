//! The build card's 「순서 정하기」 (the build card's plan, step 5): the order
//! editor opens on the checkout's own `.zo/build.toml`, or on what its root
//! suggests when it has none, previews the file the edited order would be, and
//! writes it only when the person says so — never over a file that changed
//! since it was opened (`zerocode_core::build_file`).

use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use zerocode_core::build_file::{self, BuildFile, Draft};
use zerocode_core::build_presets::Detector;
use zerocode_core::builds::BuildName;

use crate::*;

/// The window's one memory of what a checkout's root suggests: the build
/// card's look and the editor's draft read the same one.
static DETECTOR: LazyLock<Mutex<Detector>> = LazyLock::new(Mutex::default);

/// What `checkout`'s root suggests, drafted again only when a file it reads
/// moved.
pub(crate) fn suggestion(checkout: &Path) -> Arc<BuildFile> {
    DETECTOR
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .suggest(checkout)
}

/// What the order editor opens on for `worktree` — the checkout on screen,
/// the open one when none is named — with every build that keeps runs in its
/// repository. Read on the blocking pool; nothing runs.
#[tauri::command]
pub(crate) async fn build_order_draft(
    state: State<'_, AppState>,
    worktree: Option<String>,
) -> Result<Draft, String> {
    let checkout = worktree.map_or_else(|| state.active_root(), PathBuf::from);
    tauri::async_runtime::spawn_blocking(move || {
        let recorded: Vec<BuildName> =
            crate::cmd::board::builds_look(&checkout, crate::now_epoch_ms())
                .builds
                .iter()
                .map(BuildName::from)
                .collect();
        build_file::draft(
            &checkout,
            || BuildFile::clone(&suggestion(&checkout)),
            &recorded,
        )
    })
    .await
    .map_err(|error| error.to_string())
}

/// The text `file` would be as a `.zo/build.toml`, or why it cannot be run as
/// written. Nothing is written.
#[tauri::command]
pub(crate) fn build_order_preview(file: BuildFile) -> Result<String, String> {
    let _crumb = crate::crumbs::Command::enter("build_order_preview");
    build_file::check(&file).map(|()| build_file::to_toml(&file))
}

/// Writes `file` as `worktree`'s `.zo/build.toml` and answers the text
/// written, only over the text `version` was taken of (none: there was no
/// file).
#[tauri::command]
pub(crate) async fn build_order_write(
    state: State<'_, AppState>,
    worktree: Option<String>,
    file: BuildFile,
    version: Option<u64>,
) -> Result<String, String> {
    let checkout = worktree.map_or_else(|| state.active_root(), PathBuf::from);
    tauri::async_runtime::spawn_blocking(move || build_file::write(&checkout, &file, version))
        .await
        .map_err(|error| error.to_string())?
}
