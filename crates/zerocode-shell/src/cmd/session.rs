//! Session commands.

use crate::*;

#[tauri::command(async)]
pub(crate) fn boot_report(state: State<'_, AppState>) -> BootReport {
    let registry = state.registry();
    let (zo, bind, serve) = match state.supervisor() {
        Some(supervisor) => (
            Some(supervisor.zo_path().display().to_string()),
            supervisor.bind_addr().to_string(),
            match supervisor.probe() {
                ServeProbe::Ready => "ready",
                ServeProbe::Unauthorized => "unauthorized",
                ServeProbe::NotSessionServer => "foreign",
                ServeProbe::NotListening => "down",
            },
        ),
        None if state.supervisor_unavailable() => (None, String::new(), "error"),
        None => (None, String::new(), "down"),
    };
    let settings = load_settings_for_boot(state.settings());
    crumbs::record("boot", format_args!("report"));
    hang_watchdog::ready();
    let report = BootReport {
        zo,
        bind,
        serve,
        lane_error: state.supervisor_unavailable(),
        project: state
            .project_root()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        branch: current_branch(state.project_root()),
        lanes: registry.lanes().cloned().collect(),
        focused: registry.focused(),
        settings,
        window_blur_active: state.window_blur_active(),
        onboarding: stored_onboarding(state.config_root()),
        last_crash: crash::consume(state.local_data_root()),
    };
    // t-3014 §2.4: the incident the sheet is about to show is the one the
    // standing-order beat files as a ledger task — armed here, AFTER the
    // consume above, because a sweep that ran first would judge the last
    // boot's incident. The beat, not this site, presents the seat: no leader
    // pane has been cut when this report is built.
    crash_triage::arm();
    report
}

#[tauri::command]
pub(crate) async fn session_info(
    state: State<'_, AppState>,
    session: String,
) -> Result<SessionInfo, String> {
    let Some(supervisor) = state.supervisor().cloned() else {
        return Ok(SessionInfo {
            model: None,
            permission_mode: None,
        });
    };
    let addr = channel_addr(&state, &session)?;
    let token = supervisor.token().map(str::to_string);
    tauri::async_runtime::spawn_blocking(move || {
        with_client(addr, token, async move |client| {
            let info = client
                .call(method::INFO, json!({}))
                .await
                .map_err(|error| error.to_string())?;
            let field = |key: &str| {
                info.get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            };
            Ok(SessionInfo {
                model: field("model"),
                permission_mode: field("permission_mode"),
            })
        })
    })
    .await
    .map_err(|join| join.to_string())?
}

/// The workspace's re-enterable Claude conversations, newest first.
///
/// Reads at most the newest eight files' 256 KiB heads — the sidebar wants
/// three rows, not an index of the store — and returns only sessions a
/// person actually spoke in.
#[tauri::command(async)]
pub(crate) fn list_claude_sessions(path: String) -> Vec<ClaudeSessionRow> {
    let (Some(home), Some(key)) = (
        dirs::home_dir(),
        zerocode_core::session_key_of(zerocode_core::AgentKind::Claude),
    ) else {
        return Vec::new();
    };
    let store = claude_project_slugs(&path)
        .into_iter()
        .map(|slug| home.join(".claude").join("projects").join(slug))
        .find(|spot| spot.is_dir());
    let Some(store) = store else {
        return Vec::new();
    };
    let mut files: Vec<(std::path::PathBuf, u64)> = std::fs::read_dir(&store)
        .map(|walk| {
            walk.filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let stem = name.strip_suffix(".jsonl")?;
                if !is_claude_session_id(stem) {
                    return None;
                }
                let at = entry
                    .metadata()
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_millis() as u64;
                Some((entry.path(), at))
            })
            .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    let mut rows = Vec::new();
    for (file, at_ms) in files.into_iter().take(8) {
        let Ok(mut reader) = std::fs::File::open(&file) else {
            continue;
        };
        let mut head = vec![0u8; 262_144];
        let got = std::io::Read::read(&mut reader, &mut head).unwrap_or(0);
        head.truncate(got);
        let text = String::from_utf8_lossy(&head);
        let Some(title) = claude_session_title(&text) else {
            continue;
        };
        let id = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let session = zerocode_core::ProviderSession {
            key,
            id,
            transcript_path: Some(file.to_string_lossy().into_owned()),
        };
        rows.push(ClaudeSessionRow {
            session,
            title,
            at_ms,
        });
        if rows.len() >= 3 {
            break;
        }
    }
    rows
}

/// One row of the picker: what the machine has, and what the window last
/// saw of the agent's binary and login (t-3996) — read at display
/// freshness, so a settings row can say 「로그인 필요」 before a launch.
#[derive(serde::Serialize)]
pub(crate) struct AgentListRow {
    #[serde(flatten)]
    pub(crate) presence: zerocode_core::AgentPresence,
    pub(crate) readiness: zerocode_core::readiness::AgentReadinessSnapshot,
}

/// `refresh` re-reads the shell PATH first — the button that sends it exists
/// because the user has usually just installed something, which CHANGED the
/// PATH the last read captured. Blocks up to five seconds on a slow shell, so
/// it rides the blocking pool.
#[tauri::command]
pub(crate) async fn list_agents(refresh: Option<bool>) -> Vec<AgentListRow> {
    tauri::async_runtime::spawn_blocking(move || {
        detected_agents(refresh.unwrap_or(false))
            .into_iter()
            .map(|presence| AgentListRow {
                readiness: readiness_runtime::ensure_seen(
                    &presence,
                    zerocode_core::readiness::Purpose::Display,
                ),
                presence,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[tauri::command(async)]
pub(crate) fn set_default_agent(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    preference: zerocode_core::DefaultAgentPreference,
) -> Result<SettingsSnapshot, String> {
    let preference = validate_default_agent(preference)?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::DEFAULT_AGENT],
        move |settings| {
            settings.default_agent = preference;
            Ok(())
        },
    )
}

/// The recipes one scope sees, for the composer to seed its fields from.
///
/// Resolved, not raw: `scope` names a workspace and the answer is what that
/// repository would actually run — its own recipes, plus the global ones it
/// has not overridden. `None` asks the global scope by itself, which is how
/// the dialog can tell an inherited answer from an overridden one.
#[tauri::command(async)]
pub(crate) fn launch_recipes(
    state: State<'_, AppState>,
    scope: Option<String>,
) -> Result<BTreeMap<String, zerocode_core::source_control_ai::LaunchRecipe>, String> {
    let scope = recipe_scope_key(state.config_root(), scope)?;
    Ok(stored_recipe_book(state.config_root()).effective_all(scope.as_deref()))
}

/// Save one action's recipe in one scope — or clear it there by saving what
/// that scope already inherits, which keeps the file from accumulating entries
/// that say nothing.
#[tauri::command(async)]
pub(crate) fn save_launch_recipe(
    state: State<'_, AppState>,
    action: String,
    recipe: zerocode_core::source_control_ai::LaunchRecipe,
    scope: Option<String>,
) -> Result<(), String> {
    use zerocode_core::source_control_ai as ai;
    if !ai::is_recipe_action(&action) {
        return Err(format!("{action}은(는) 저장할 수 있는 액션이 아닙니다"));
    }
    // Refused at the door, not at launch: a recipe whose arguments cannot
    // ride an argv spawn would otherwise be saved now and refuse every night
    // after.
    ai::split_agent_args(&recipe.args)?;
    if let Some(agent) = recipe.agent.as_deref()
        && agent_spec(agent).is_none()
    {
        return Err(format!("{agent}은(는) 이 창이 아는 에이전트가 아닙니다"));
    }
    let scope = recipe_scope_key(state.config_root(), scope)?;
    let file = launch_recipes_file(state.config_root());
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut book = stored_recipe_book(state.config_root());
    book.set(&action, &recipe, scope.as_deref());
    let text = serde_json::to_string_pretty(&book).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// Resolve a launch action's agent, prompt and arguments — the one road every
/// source-control launch takes, direct button and composer alike.
///
/// The rules are core's, and the one with teeth is the HARD BLOCK: a saved
/// agent that is not installed refuses with its name, never falls back —
/// somebody chose it for this action on purpose, and quietly running a
/// different agent substitutes a decision nobody made (Orca:
/// `savedAgentUnavailable`, SourceControl-e46DLHZz.js:5807-5809). The
/// composer's overrides ride the same road so the two doors cannot disagree
/// about what a template or an argument means.
///
/// The scope is this window's, not the caller's: the recipe that applies is
/// the one the repository being looked at would run, and a window that took
/// somebody's word for which repository that is could launch one checkout's
/// recipe in another.
#[tauri::command(async)]
pub(crate) fn launch_plan_for_action(
    state: State<'_, AppState>,
    action: String,
    base_prompt: String,
    agent_override: Option<String>,
    template_override: Option<String>,
    args_override: Option<String>,
) -> Result<ResolvedLaunch, String> {
    use zerocode_core::source_control_ai as ai;
    if ai::LaunchAction::from_id(&action).is_none() {
        return Err(format!(
            "{action}은(는) 이 창이 아는 launch 액션이 아닙니다"
        ));
    }
    let scope = active_recipe_scope(&state);
    let recipe = stored_recipe_book(state.config_root()).effective(&action, scope.as_deref());
    let saved = agent_override.or(recipe.agent);
    let template = template_override.unwrap_or(recipe.template);
    let args = ai::split_agent_args(&args_override.unwrap_or(recipe.args))?;

    let installed: Vec<String> = detected_agents(false)
        .into_iter()
        .filter(|row| row.installed)
        .map(|row| row.id.to_string())
        .collect();
    let default_agent = load_settings(state.settings())?.document.default_agent;
    let agent = match ai::pick_launch_agent(saved.as_deref(), default_agent.agent_id(), &installed)
    {
        ai::AgentChoice::Agent(agent) => agent,
        ai::AgentChoice::SavedUnavailable(agent) => {
            return Err(format!(
                "저장된 에이전트 {agent}이(가) 이 기계에 없습니다 — 실행 설정에서 다른 에이전트를 고르세요"
            ));
        }
        ai::AgentChoice::NoneInstalled => {
            return Err("PATH에서 찾은 에이전트가 없습니다".to_string());
        }
    };
    let prompt = ai::render_command_template(&template, &[("basePrompt", base_prompt.as_str())])
        .trim()
        .to_string();
    if prompt.is_empty() {
        return Err("명령 입력이 비어 있습니다 — 템플릿을 확인하세요".to_string());
    }
    Ok(ResolvedLaunch {
        agent,
        prompt,
        args,
    })
}

/// Build only the local allowlisted folder, then show it in the file manager.
#[tauri::command(async)]
pub(crate) fn crash_bundle(state: State<'_, AppState>) -> Result<String, String> {
    let path = crash::bundle(state.local_data_root())?;
    crash::open_local(&path)?;
    Ok(path.display().to_string())
}

/// This door accepts no caller path; the log belongs to this window's root.
#[tauri::command(async)]
pub(crate) fn crash_open_log(state: State<'_, AppState>) -> Result<(), String> {
    let path = state.local_data_root().join("window-errors.log");
    durable_file::open_plain_file(&path).map_err(|error| error.to_string())?;
    crash::open_local(&path)
}

/// Field patches preserve another window's threshold change.
#[tauri::command(async)]
pub(crate) fn set_crash_watchdog(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> Result<SettingsSnapshot, String> {
    let fields = patch
        .as_object()
        .ok_or("crash settings require an object")?;
    if fields.iter().any(|(key, value)| match key.as_str() {
        "watchdog" => !value.is_boolean(),
        "first_ms" | "second_ms" => !value.is_u64(),
        _ => true,
    }) {
        return Err("invalid crash settings field".into());
    }
    let patch = fields.clone();
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::CRASH],
        move |settings| {
            let mut saved = settings.crash.as_object().cloned().unwrap_or_default();
            saved.extend(patch);
            let limits = crash::Limits::overlay(&serde_json::Value::Object(saved));
            settings.crash = json!({"watchdog":limits.watchdog,"first_ms":limits.first_ms,"second_ms":limits.second_ms});
            Ok(())
        },
    )
}
