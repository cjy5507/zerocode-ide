//! The Skills·MCP page's commands: every one reads the window once and does
//! its disk work off the UI thread (`library_runtime`).
use crate::library_runtime::{self, Check, Context, Report};
use crate::*;
use zerocode_core::library::mcp::McpDef;

async fn off_thread<T: Send + 'static>(
    state: &State<'_, AppState>,
    work: impl FnOnce(Context) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let context = Context::from_state(state)?;
    tauri::async_runtime::spawn_blocking(move || work(context))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn library_report(state: State<'_, AppState>) -> Result<Report, String> {
    off_thread(&state, |context| Ok(library_runtime::report(&context))).await
}

/// Look now, and answer what the page shows.
#[tauri::command]
pub(crate) async fn library_look(state: State<'_, AppState>) -> Result<Report, String> {
    off_thread(&state, |context| {
        library_runtime::look(&context)?;
        Ok(library_runtime::report(&context))
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_server_save(
    state: State<'_, AppState>,
    name: String,
    server: McpDef,
    previous: Option<String>,
    shown: Option<String>,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::save_server(
            &context,
            name.trim(),
            server,
            previous.as_deref(),
            shown.as_deref(),
        )
    })
    .await
}

/// The skills a repository offers, for the add dialog's list.
#[tauri::command]
pub(crate) async fn library_skill_list(
    state: State<'_, AppState>,
    repository: String,
) -> Result<Vec<String>, String> {
    off_thread(&state, move |context| {
        library_runtime::list_repository(&context, repository.trim())
    })
    .await
}

/// One server a pasted block names, as the add dialog lists it.
#[derive(Debug, Serialize)]
pub(crate) struct PastedServer {
    pub name: String,
    pub kind: &'static str,
    pub summary: String,
    pub secrets: Vec<String>,
}

#[tauri::command(async)]
pub(crate) fn library_paste_read(text: String) -> Vec<PastedServer> {
    library_runtime::read_pasted(&text)
        .into_iter()
        .map(|(name, kind, summary, secrets)| PastedServer {
            name,
            kind,
            summary,
            secrets,
        })
        .collect()
}

#[tauri::command]
pub(crate) async fn library_paste_add(
    state: State<'_, AppState>,
    text: String,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::add_pasted(&context, &text)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_server_remove(
    state: State<'_, AppState>,
    name: String,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::remove_server_everywhere(&context, &name)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_server_switch(
    state: State<'_, AppState>,
    name: String,
    on: bool,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::switch_server(&context, &name, on)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_server_agent(
    state: State<'_, AppState>,
    name: String,
    agent: String,
    on: bool,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::switch_server_for(&context, &name, &agent, on)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_server_choose(
    state: State<'_, AppState>,
    name: String,
    fingerprint: String,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::choose_server(&context, &name, &fingerprint)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_server_share(
    state: State<'_, AppState>,
    name: String,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::share_server(&context, &name)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_server_check(
    state: State<'_, AppState>,
    name: String,
) -> Result<Check, String> {
    off_thread(&state, move |context| {
        Ok(library_runtime::check_server(&context, &name))
    })
    .await
}

/// The sign-in command for one agent, for the page to type into a terminal.
#[tauri::command(async)]
pub(crate) fn library_server_login(name: String, agent: String) -> Result<String, String> {
    library_runtime::login_command(&name, &agent)
}

#[tauri::command]
pub(crate) async fn library_skill_remove(
    state: State<'_, AppState>,
    name: String,
    copies: bool,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::remove_skill(&context, &name, copies)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_skill_import(
    state: State<'_, AppState>,
    path: String,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::import_skill(&context, Path::new(&path))
    })
    .await
}

/// Make a new skill in the store; the page opens the file it answers with.
#[tauri::command]
pub(crate) async fn library_skill_create(
    state: State<'_, AppState>,
    name: String,
    description: String,
) -> Result<String, String> {
    off_thread(&state, move |context| {
        library_runtime::make_store_skill(&context, name.trim(), &description)
            .map(|path| path.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_skill_choose(
    state: State<'_, AppState>,
    name: String,
    path: Option<String>,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::choose_skill(&context, &name, path.as_deref())
    })
    .await
}

/// 「저장소 것으로 맞추기」: an agent's copy of a skill set to the store's
/// text — kept, then to the Trash. The person's alone (`PERSON_ONLY_COMMANDS`):
/// it takes a folder out of an agent's home.
#[tauri::command]
pub(crate) async fn library_skill_align(
    state: State<'_, AppState>,
    name: String,
    agent: String,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::align_skill(&context, &name, &agent)
    })
    .await
}

#[tauri::command]
pub(crate) async fn library_undo(state: State<'_, AppState>, id: u64) -> Result<Report, String> {
    off_thread(&state, move |context| library_runtime::undo(&context, id)).await
}

#[tauri::command]
pub(crate) async fn library_pause(
    state: State<'_, AppState>,
    paused: bool,
) -> Result<Report, String> {
    off_thread(&state, move |context| {
        library_runtime::set_paused(&context, paused)
    })
    .await
}
