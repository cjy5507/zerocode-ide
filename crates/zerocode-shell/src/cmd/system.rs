//! System commands: what this process is, and what the release lane left
//! for it. The gap map (B8) reserves this file for `update_status`,
//! `check_for_update` and `install_update`; these two are the read-only
//! front of that — the machine's own lane, no feed, no download.

use crate::*;

/// The three compile-time stamps (`build.rs::stamp_identity`), verbatim.
/// `--version` and the crash report read the same env values; this is the
/// window's door to them, which did not exist before the release lane
/// needed the window to say which build it is.
#[tauri::command]
pub(crate) fn build_stamp() -> update_runtime::BuildStamp {
    let _crumb = crate::crumbs::Command::enter("build_stamp");
    update_runtime::build_stamp()
}

/// The lane's `status.json` and `installed.json` as read just now, and the
/// notice judged over them (docs/design/release-lane-off-the-window.md
/// §2.3–2.4). Read-only: the window never writes under the lane's directory
/// (§3 rule 7), and a missing or malformed file is `null`, never an error —
/// the lane is another process, and mid-write is its ordinary state.
///
/// Polled on the status bar's period rather than watched: a release is a
/// minutes-scale event.
///
/// Off the main thread since t-6428: the census it answers with reads the
/// process table.
#[tauri::command(async)]
pub(crate) fn release_status(app: AppHandle) -> update_runtime::ReleaseStatus {
    let _crumb = crate::crumbs::Command::enter("release_status");
    let state = app.state::<AppState>();
    let running = update_runtime::build_stamp();
    let zo_running = zo_integration_runtime::running_zo_builds(&state);
    let mut answer = match update_runtime::release_dir() {
        Some(dir) => update_runtime::release_status_in(&dir, &running, &zo_running),
        None => update_runtime::ReleaseStatus {
            status: serde_json::Value::Null,
            installed: serde_json::Value::Null,
            notice: None,
            busy: crate::orchestration::restart_census::Busy::default(),
        },
    };
    // And who a restart would cut (t-3058, t-6428): the one census every
    // road out of the window asks — workers mid-turn, background jobs under
    // workers at rest, workers nobody could read — so the notice says it
    // beside its button in the question's own words.
    answer.busy = crate::cmd::appearance::take_census(&app).busy();
    answer
}
