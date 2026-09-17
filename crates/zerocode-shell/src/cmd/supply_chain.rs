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
