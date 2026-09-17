//! Bind the path-operation owner to the window's active checkout and settings.
use crate::*;
use file_tree_ops::{History, TreeOp};

pub(crate) fn policy(state: &AppState) -> Result<explorer_policy::ExplorerPolicy, String> {
    Ok(explorer_policy::ExplorerPolicy::overlay(
        &load_settings(state.settings())?.document.explorer,
    ))
}

pub(crate) fn history(state: &AppState) -> MutexGuard<'_, History> {
    state
        .shell_runtime()
        .tree_ops
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn root(state: &AppState, asked: Option<&str>) -> Result<PathBuf, String> {
    let root = state
        .active_root()
        .canonicalize()
        .map_err(|_| "tree.invalidMove")?;
    if let Some(asked) = asked
        && Path::new(asked).canonicalize().ok().as_ref() != Some(&root)
    {
        return Err("tree.invalidMove".into());
    }
    Ok(root)
}

pub(crate) fn announce(app: &AppHandle, root: &Path, operation: &TreeOp) {
    let _ = app.emit(
        "tree:operation",
        json!({"root": root, "operation": operation}),
    );
}
