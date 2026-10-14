//! The composer's two lists: the slash commands a pane's CLI takes right now,
//! and the models its agent·model chip can offer (`slash_catalog`).

fn home() -> Result<std::path::PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "no home directory".to_string())
}

/// The commands `agent` takes in `cwd`, from the honest source this machine
/// has for it (the module says which). `refresh` skips the cached answer —
/// the palette's own refresh, after a person added a command.
#[tauri::command(async)]
pub(crate) fn slash_commands(
    agent: String,
    cwd: String,
    refresh: Option<bool>,
) -> Result<crate::slash_catalog::SlashCatalog, String> {
    let home = home()?;
    // The PATH the agent list itself is judged against (`detected_agents`).
    let path_var = crate::shell_path::launch_path();
    Ok(crate::slash_catalog::slash_catalog(
        &agent,
        std::path::Path::new(&cwd),
        &home,
        path_var.as_deref(),
        refresh == Some(true),
    ))
}

/// The models `agent` can switch to, from zo's live catalog.
#[tauri::command(async)]
pub(crate) fn agent_models(agent: String) -> Result<Vec<crate::slash_catalog::ModelRow>, String> {
    let home = home()?;
    Ok(crate::slash_catalog::agent_models(&agent, &home))
}

/// The releases today's lineup holds as superseded, by agent, and the one
/// that stands for each (t-46882) — peeked off the lineup the restore roads
/// judge by ([`crate::summon_lineup::snapshot`]), never fetched. The window
/// asks once as it starts; every later read says it again
/// (`summon-lineup:superseded`).
#[tauri::command(async)]
pub(crate) fn superseded_models()
-> std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
    crate::summon_lineup::superseded(&crate::summon_lineup::snapshot().lineups)
}
