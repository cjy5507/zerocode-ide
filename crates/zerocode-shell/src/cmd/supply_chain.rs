//! The supply-chain layer's one command (docs/design/knowledge-supply-chain-20260917.md §5.3).

use crate::*;

/// The supply chain of a workspace — the active one unless `root` names
/// another: its lockfiles as components, OSV's answer as vulnerabilities, and
/// the edges between them. `refresh` asks OSV even while a day-fresh answer
/// about the same lockfiles is cached. Off the main thread: the first lookup
/// of a workspace is seconds of network.
#[tauri::command]
pub(crate) async fn supply_chain_graph(
    state: State<'_, AppState>,
    root: Option<String>,
    refresh: Option<bool>,
) -> Result<crate::supply_chain::SupplyChainAnswer, String> {
    let asked = root.map_or_else(|| state.active_root(), PathBuf::from);
    let cache_root = state.cache_root().to_path_buf();
    let local_data_root = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let root = crate::supply_chain::workspace_root(&asked)?;
        Ok(crate::supply_chain::supply_chain_answer(
            &root,
            &cache_root,
            &local_data_root,
            &crate::supply_chain::OsvWire::public(),
            refresh.unwrap_or(false),
            crate::project_runtime::now_epoch_ms(),
        ))
    })
    .await
    .map_err(|join| join.to_string())?
}

/// Where the report landed, and the shape of what it says — enough for the
/// window to name the file and say what it found without reading it back.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SupplyReportWritten {
    /// Workspace-relative, `/`-separated — what the window shows.
    pub(crate) path: String,
    /// Absolute, for opening it.
    pub(crate) absolute: String,
    pub(crate) advisories: usize,
    pub(crate) raise: usize,
    pub(crate) counted: Vec<(zerocode_core::supply_chain::Severity, usize)>,
}

/// The same supply chain, written down as work.
///
/// The lens answers "what is wrong" one triangle at a time. This answers the
/// other half — for each advisory, the direct dependency a member would raise,
/// and for each such dependency, everything raising it closes — and leaves a
/// file the person can read away from the window, share, or paste into a
/// ticket. The reading is [`zerocode_core::supply_chain::report`]'s, which is
/// pure: the document and the picture cannot disagree.
///
/// The file lands in the workspace's own `output/` so it sits beside the code
/// it is about. Off the main thread: it may ask OSV.
#[tauri::command]
pub(crate) async fn supply_chain_report(
    state: State<'_, AppState>,
    root: Option<String>,
    refresh: Option<bool>,
) -> Result<SupplyReportWritten, String> {
    let asked = root.map_or_else(|| state.active_root(), PathBuf::from);
    let cache_root = state.cache_root().to_path_buf();
    let local_data_root = state.local_data_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let root = crate::supply_chain::workspace_root(&asked)?;
        let now_ms = crate::project_runtime::now_epoch_ms();
        let answer = crate::supply_chain::supply_chain_answer(
            &root,
            &cache_root,
            &local_data_root,
            &crate::supply_chain::OsvWire::public(),
            refresh.unwrap_or(false),
            now_ms,
        );
        let report = zerocode_core::supply_chain::report(&answer.graph);
        let title = root.file_name().map_or_else(
            || root.display().to_string(),
            |name| name.to_string_lossy().to_string(),
        );
        let stamp = zerocode_core::civil::iso_utc_of(now_ms);
        let text = zerocode_core::supply_chain::markdown(&report, &title, &stamp);
        /* One name per day and workspace: a second reading on the same day
         * replaces the first rather than leaving a person to guess which of
         * two files is the one they just asked for. */
        let name = format!("supply-chain-{}.md", &stamp[..10]);
        let dir = root.join("output");
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let file = dir.join(&name);
        std::fs::write(&file, &text).map_err(|error| error.to_string())?;
        Ok(SupplyReportWritten {
            path: format!("output/{name}"),
            absolute: file.display().to_string(),
            advisories: report.advisories.len(),
            raise: report.raise.len(),
            counted: report.counted,
        })
    })
    .await
    .map_err(|join| join.to_string())?
}
