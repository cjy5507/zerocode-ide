//! Agent launch commands.

use crate::*;

#[tauri::command(async)]
pub(crate) fn agent_launch_plans(
    state: State<'_, AppState>,
) -> Result<Vec<AgentLaunchRow>, String> {
    Ok(agent_launch_rows(&stored_launch_overrides(
        state.settings(),
    )?))
}

/// The args half. `null` clears this half only — the command's name is the
/// scope, which is what lets `None` be unambiguous on each wire.
#[tauri::command(async)]
pub(crate) fn save_agent_launch(
    state: State<'_, AppState>,
    agent: String,
    args: Option<String>,
) -> Result<Vec<AgentLaunchRow>, String> {
    save_agent_launch_in(
        state.settings(),
        agent,
        LaunchEdit::from_half(args),
        LaunchEdit::Keep,
    )
}

/// The env half, as the line the person typed. Parsed HERE — `KEY=VALUE`
/// words, quote-aware — because the window has no splitter and two copies of
/// one grammar disagree the first time either changes. An empty line is an
/// opinion ("no variables"), absence of the override is the default; `null`
/// clears this half.
#[tauri::command(async)]
pub(crate) fn save_agent_launch_env(
    state: State<'_, AppState>,
    agent: String,
    env: Option<String>,
) -> Result<Vec<AgentLaunchRow>, String> {
    let parsed = match env {
        None => LaunchEdit::Clear,
        Some(line) => LaunchEdit::Set(parse_env_line(&line)?),
    };
    save_agent_launch_in(state.settings(), agent, LaunchEdit::Keep, parsed)
}

/// Both halves at once — the reset control's one press, atomic on the
/// document rather than two writes that can interleave with another window.
#[tauri::command(async)]
pub(crate) fn reset_agent_launch(
    state: State<'_, AppState>,
    agent: String,
) -> Result<Vec<AgentLaunchRow>, String> {
    save_agent_launch_in(
        state.settings(),
        agent,
        LaunchEdit::Clear,
        LaunchEdit::Clear,
    )
}

#[tauri::command(async)]
pub(crate) fn set_agent_permission_mode(
    state: State<'_, AppState>,
    mode: zerocode_core::AgentPermissionMode,
) -> Result<Vec<AgentLaunchRow>, String> {
    set_agent_permission_mode_in(state.settings(), mode)
}

#[tauri::command(async)]
pub(crate) fn list_quick_commands(state: State<'_, AppState>) -> Vec<QuickCommand> {
    stored_quick_commands(state.config_root())
}

#[tauri::command(async)]
pub(crate) fn save_quick_command(
    state: State<'_, AppState>,
    command: QuickCommand,
) -> Result<(), String> {
    validate_quick_command(&command)?;
    let mut rows = stored_quick_commands(state.config_root());
    match rows.iter_mut().find(|held| held.id == command.id) {
        Some(held) => *held = command,
        None => rows.push(command),
    }
    write_quick_commands(state.config_root(), &rows)
}

#[tauri::command(async)]
pub(crate) fn delete_quick_command(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut rows = stored_quick_commands(state.config_root());
    rows.retain(|held| held.id != id);
    write_quick_commands(state.config_root(), &rows)
}
