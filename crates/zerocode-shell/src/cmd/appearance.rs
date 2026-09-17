//! Appearance commands.

use crate::*;

/// The terminal's settings, as the window should draw them.
#[tauri::command(async)]
pub(crate) fn terminal_prefs(state: State<'_, AppState>) -> TerminalPrefs {
    load_settings_for_boot(state.settings())
        .document
        .terminal_prefs
}

/// Report only the native facts the Windows-only settings cannot know.
///
/// Keeping executable discovery here means the renderer never guesses from a
/// PATH string and never exposes a general-purpose process probe.
#[tauri::command(async)]
pub(crate) fn terminal_windows_status() -> WindowsTerminalStatus {
    WindowsTerminalStatus {
        supported: cfg!(windows),
        pwsh_available: cfg!(windows) && program_exists(POWERSHELL_7),
        git_bash_available: cfg!(windows) && installed_windows_git_bash().is_some(),
    }
}

/// Parse Warp YAML away from the renderer and away from Tauri's event loop.
///
/// `None` is a native-picker cancel, not an empty scan. Individual malformed
/// files are reported in the typed preview so one bad theme cannot hide the
/// valid themes next to it.
#[tauri::command]
pub(crate) async fn preview_warp_terminal_themes(
    app: AppHandle,
    source: WarpThemeImportSource,
) -> Result<Option<WarpThemePreview>, String> {
    enum PreviewTask {
        Auto,
        Files(Vec<PathBuf>),
        Folder(PathBuf),
    }

    let task = match source {
        WarpThemeImportSource::Auto => PreviewTask::Auto,
        WarpThemeImportSource::Files => {
            let picked = pick_paths(
                &app,
                zerocode_core::pick::PickRequest::new(zerocode_core::pick::PickKind::Files)
                    .filtered("Warp YAML theme", &["yaml", "yml"]),
            )
            .await?;
            if picked.is_empty() {
                return Ok(None);
            }
            PreviewTask::Files(picked)
        }
        WarpThemeImportSource::Folder => {
            let mut picked = pick_paths(
                &app,
                zerocode_core::pick::PickRequest::new(zerocode_core::pick::PickKind::Folder),
            )
            .await?;
            let Some(path) = picked.pop() else {
                return Ok(None);
            };
            PreviewTask::Folder(path)
        }
    };

    let preview = tauri::async_runtime::spawn_blocking(move || match task {
        PreviewTask::Auto => terminal_theme_import::preview_auto(),
        PreviewTask::Files(paths) => terminal_theme_import::preview_files(paths),
        PreviewTask::Folder(path) => terminal_theme_import::preview_folder(&path),
    });
    tokio::time::timeout(THEME_PREVIEW_BUDGET, preview)
        .await
        .map_err(|_| "테마 미리보기 제한 시간 5초를 넘었습니다")?
        .map(Some)
        .map_err(|error| format!("테마 미리보기를 만들지 못했습니다: {error}"))
}

/// Discover and parse Ghostty's native config without blocking Tauri's event
/// loop. The returned patch contains only values that differ from the latest
/// canonical settings snapshot, so the dialog never advertises a no-op.
#[tauri::command]
pub(crate) async fn preview_ghostty_import(
    state: State<'_, AppState>,
) -> Result<ghostty_import::GhosttyImportPreview, String> {
    let current = load_settings_resilient(state.settings()).document;
    let preview = tauri::async_runtime::spawn_blocking(ghostty_import::preview_auto);
    let preview = tokio::time::timeout(THEME_PREVIEW_BUDGET, preview)
        .await
        .map_err(|_| "Ghostty 설정 미리보기 제한 시간 5초를 넘었습니다")?
        .map_err(|error| format!("Ghostty 설정 미리보기를 만들지 못했습니다: {error}"))?;
    Ok(preview.against(&current))
}

/// Apply every compatible Ghostty field as one settings revision.
///
/// Terminal appearance, window material, and middle-click behavior must never
/// become three independently durable partial imports. The repository lock,
/// normalization, settings event, and returned canonical snapshot therefore
/// share this single command.
#[tauri::command(async)]
pub(crate) fn apply_ghostty_import(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: ghostty_import::GhosttyImportPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[
            setting_key::TERMINAL_PREFS,
            setting_key::WINDOW_MATERIAL,
            setting_key::EDITING_PREFS,
        ],
        move |settings| {
            patch.apply(settings);
            Ok(())
        },
    )
}

/// Store the compatible full record and apply its Rust-owned scrollback depth.
///
/// The other fields are renderer preferences and repaint from the returned
/// canonical snapshot. Scrollback is a Rust-side buffer, so every live
/// terminal is retold here as part of an intentional full-record replacement.
#[tauri::command(async)]
pub(crate) fn set_terminal_prefs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    prefs: TerminalPrefs,
) -> Result<SettingsSnapshot, String> {
    let kept = prefs.clamped();
    let scrollback = kept.scrollback;
    let snapshot = commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::TERMINAL_PREFS],
        move |settings| {
            settings.terminal_prefs = kept.clone();
            Ok(())
        },
    )?;
    for pty in state.terminals().handles() {
        lock_pty(&pty)
            .terminal_mut()
            .grid_mut()
            .set_scrollback_cap(scrollback);
    }
    Ok(snapshot)
}

/// Store exactly one terminal control against the latest canonical record.
/// This is the renderer path; the full-object command remains for compatible
/// callers that intentionally replace the complete preference record.
#[tauri::command(async)]
pub(crate) fn patch_terminal_prefs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: TerminalPrefsPatch,
) -> Result<SettingsSnapshot, String> {
    let changes_scrollback = matches!(&patch, TerminalPrefsPatch::Scrollback(_));
    let snapshot = commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::TERMINAL_PREFS],
        move |settings| {
            patch.apply(&mut settings.terminal_prefs);
            Ok(())
        },
    )?;
    if changes_scrollback {
        let scrollback = snapshot.document.terminal_prefs.scrollback;
        for pty in state.terminals().handles() {
            lock_pty(&pty)
                .terminal_mut()
                .grid_mut()
                .set_scrollback_cap(scrollback);
        }
    }
    Ok(snapshot)
}

/// Choose where a new workspace's setup terminal is placed. The command is a
/// typed enum, so invalid renderer strings are rejected by deserialization
/// instead of becoming a silent default.
#[tauri::command(async)]
pub(crate) fn set_setup_script_launch_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: SetupScriptLaunchMode,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SETUP_SCRIPT_LAUNCH_MODE],
        move |settings| {
            settings.setup_script_launch_mode = mode;
            Ok(())
        },
    )
}

/// Whether the machine is held awake while agents run — Orca's three-way
/// choice, applied to the live power hold the moment it is written.
#[tauri::command(async)]
pub(crate) fn set_computer_awake_mode(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: awake::ComputerAwakeMode,
) -> Result<SettingsSnapshot, String> {
    let snapshot = commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::COMPUTER_AWAKE_MODE],
        move |settings| {
            settings.computer_awake_mode = mode;
            Ok(())
        },
    )?;
    // After the write, so a refused commit changes nothing on the machine.
    state.awake().set_mode(mode);
    let (mode, active) = state.awake().status();
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        "computer-awake:changed",
        AwakeStatus { mode, active },
    );
    Ok(snapshot)
}

/// Choose which side receives a shortcut that both the app and the focused
/// terminal can interpret. Terminal-scoped actions remain available in both
/// modes; the renderer owns that live routing distinction.
/// The caffeinate segment's live reading. Poll-shaped on purpose: the two
/// hot gates below PUSH `computer-awake:changed`, and everything slower
/// (the keeper's staleness tick, a pane forgotten by the reaper) is caught
/// by the segment re-asking on the events it already hears.
#[tauri::command]
pub(crate) fn computer_awake_status(state: State<'_, AppState>) -> AwakeStatus {
    let _crumb = crate::crumbs::Command::enter("computer_awake_status");
    let (mode, active) = state.awake().status();
    AwakeStatus { mode, active }
}

#[tauri::command(async)]
pub(crate) fn set_terminal_shortcut_policy(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    policy: TerminalShortcutPolicy,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::TERMINAL_SHORTCUT_POLICY],
        move |settings| {
            settings.terminal_shortcut_policy = policy;
            Ok(())
        },
    )
}

/// What a new terminal runs, as the words it means.
///
/// The line form is for the settings field, where a person edits it. Anything
/// that needs the *program* asks for this instead: splitting the line again
/// in the window turned `"/opt/my tools/zo"` into `my`, which is the wrong
/// word on a button that claims to name what it will start.
#[tauri::command(async)]
pub(crate) fn terminal_command_argv(state: State<'_, AppState>) -> Vec<String> {
    split_command(
        &load_settings_for_boot(state.settings())
            .document
            .terminal_command,
    )
}

/// Something went wrong in the window, said where a person can read it.
///
/// A webview that throws during boot goes quiet: the window is up, the panels
/// are drawn, and nothing says why the stage never filled. This session lost
/// hours to exactly that. The window reports here instead, and the message
/// lands in the same stderr as everything else the process says.
/// How much memory this process is holding, in bytes.
///
/// Orca's status bar reports this and it is a real number rather than a
/// decoration: an IDE holding eight ptys and eight agents is a process worth
/// glancing at. Asked of the kernel directly — resident size is what a person
/// means by "how much is it using", and it is what Activity Monitor shows.
///
/// `0` where we cannot answer, which the window renders as nothing at all
/// rather than as a wrong number.
#[tauri::command(async)]
pub(crate) fn process_memory() -> u64 {
    resident_bytes()
}

/// CPU and resident-memory usage for this application and every native child
/// it owns, including Android emulators recovered after an application crash.
///
/// Renderer `seats` may group a known session under a workspace, but can never
/// introduce a process: roots come exclusively from the native terminal pool
/// and lane registry, while emulator roots come from the native durable owner
/// and are checked against this exact process-table sample's start identities.
/// Remote transports remain in the result with unavailable metrics rather than
/// borrowing a meaningless local pid.
#[tauri::command]
pub(crate) async fn resource_snapshot(
    state: State<'_, AppState>,
    seats: Vec<resource_usage::ResourceSeat>,
) -> Result<resource_usage::ResourceSnapshot, String> {
    let seats = resource_usage::seat_map(seats)?;
    let mut roots = Vec::new();
    {
        let entries = state.terminals().entries();
        roots.extend(entries.iter().map(|(term, held)| {
            let session = format!("term:{term}");
            resource_usage::ResourceRoot {
                worktree: seats.get(&session).cloned().flatten(),
                session,
                pid: lock_pty(held).pid(),
            }
        }));
    }
    {
        let registry = state.registry();
        roots.extend(registry.lanes().map(|lane| {
            let session = format!("lane:{}", lane.id);
            resource_usage::ResourceRoot {
                worktree: seats
                    .get(&session)
                    .cloned()
                    .flatten()
                    .or_else(|| lane.worktree_id.clone()),
                session,
                pid: registry.pid(lane.id),
            }
        }));
    }
    let local_data_root = state.local_data_root().to_path_buf();
    let (sample, native_roots) = tauri::async_runtime::spawn_blocking(move || {
        let sample = resource_usage::enumerate_processes()?;
        let native_roots = emulator::managed_resource_processes(&local_data_root, &sample);
        Ok::<_, String>((sample, native_roots))
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(state
        .resource_usage()
        .snapshot_with_native(sample, roots, std::process::id(), native_roots))
}

/// Every listening TCP socket on this machine.
///
/// The whole machine, not this window's children — measured, and it is the
/// surprising part. Orca runs a system-wide scan and then ATTRIBUTES what it
/// finds: a socket whose process sits in a checkout is that workspace's, and
/// everything else is `external` in its own section
/// (`scanDarwinLsofPorts`/`enrichPort`, out/main/index.js:148419, :148618).
/// There is no bookkeeping of processes the app started, which is what makes
/// the feature worth having — the dev server you started in a real terminal
/// an hour ago still shows up.
#[tauri::command(async)]
pub(crate) fn listening_ports(state: State<'_, AppState>) -> PortScan {
    let here = state.active();
    let platform = std::env::consts::OS.to_string();
    let Some(raw) = scan_listeners() else {
        return PortScan {
            ports: Vec::new(),
            // Said the way Orca says it: which platform, and what about it.
            unavailable: Some(format!("{platform}에서는 포트를 훑을 수 없습니다")),
            platform,
        };
    };

    let found = parse_lsof_listeners(&raw);
    let cwds = process_cwds(&found.iter().map(|row| row.pid).collect::<Vec<_>>());

    // The checkouts this window knows about, longest path first so a worktree
    // nested inside another is the one that claims a process standing in it.
    let mut roots: Vec<(String, String)> = here
        .orchestrator
        .and_then(|orchestrator| orchestrator.list().ok())
        .map(|worktrees| {
            worktrees
                .into_iter()
                .map(|worktree| {
                    let path = worktree.path.to_string_lossy().to_string();
                    let name = worktree
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    (path, name)
                })
                .collect()
        })
        .unwrap_or_default();
    roots.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));

    let mut ports: Vec<ListeningPort> = found
        .into_iter()
        .map(|row| {
            let owner = cwds
                .get(&row.pid)
                .and_then(|cwd| owning_workspace(cwd, &roots));
            ListeningPort {
                port: row.port,
                connect_host: connect_host(&row.bind_host),
                bind_host: row.bind_host,
                pid: row.pid,
                process: row.process,
                owner: owner.map(|(_, name)| name.clone()),
                owner_path: owner.map(|(path, _)| path.clone()),
            }
        })
        .collect();

    // A workspace's own ports first — they are why this was opened — and by
    // number within each group so the list does not reshuffle between scans.
    ports.sort_by(|left, right| {
        left.owner
            .is_none()
            .cmp(&right.owner.is_none())
            .then(left.port.cmp(&right.port))
            .then(left.pid.cmp(&right.pid))
    });
    ports.truncate(PORT_LIMIT);

    PortScan {
        ports,
        unavailable: None,
        platform,
    }
}

/// 워크스페이스 소유로 확인된 리스너만 멈춘다(Orca `killWorkspacePortForTarget`).
///
/// 렌더러가 준 pid는 요청 순간의 사실 — 지금의 사실로 재검증한다: 그 pid가
/// **여전히 그 포트를 리슨하고**, 그 cwd가 **여전히 이 창이 아는 체크아웃
/// 안**일 때만 SIGTERM. 스캔과 kill 사이에 pid가 재사용됐다면 첫 검증이
/// 걸러낸다. 강제(-9)는 없다 — 서버가 제 뒷정리를 할 기회는 서버의 것이다.
#[tauri::command(async)]
pub(crate) fn stop_workspace_port(
    webview: tauri::Webview,
    state: State<'_, AppState>,
    pid: u32,
    port: u16,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let raw = scan_listeners().ok_or("이 플랫폼에서는 포트를 훑을 수 없습니다")?;
    let rows = parse_lsof_listeners(&raw);
    if !rows.iter().any(|row| row.pid == pid && row.port == port) {
        return Err("그 프로세스는 더 이상 그 포트를 듣지 않습니다".to_string());
    }
    let cwds = process_cwds(&[pid]);
    let cwd = cwds
        .get(&pid)
        .ok_or("그 프로세스의 자리를 읽을 수 없습니다")?;
    let here = state.active();
    let mut roots: Vec<(String, String)> = here
        .orchestrator
        .and_then(|orchestrator| orchestrator.list().ok())
        .map(|worktrees| {
            worktrees
                .into_iter()
                .map(|worktree| {
                    let path = worktree.path.to_string_lossy().to_string();
                    let name = worktree
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    (path, name)
                })
                .collect()
        })
        .unwrap_or_default();
    roots.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
    if owning_workspace(cwd, &roots).is_none() {
        return Err("이 프로젝트의 프로세스가 아닙니다".to_string());
    }
    #[cfg(unix)]
    {
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0 {
            return Err("프로세스를 멈출 수 없습니다".to_string());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let out = crate::proc::quiet_command("taskkill")
            .args(["/PID", &pid.to_string()])
            .output()
            .map_err(|error| error.to_string())?;
        if !out.status.success() {
            return Err("프로세스를 멈출 수 없습니다".to_string());
        }
        Ok(())
    }
}

#[tauri::command(async)]
pub(crate) fn log_window_error(state: State<'_, AppState>, message: String) {
    eprintln!("window: {message}");
    note_window_event(state.local_data_root(), &format!("window: {message}"));
}

/// Remember the language. A code we do not ship is refused rather than
/// stored: the window would read it back at boot and resolve it to nothing.
#[tauri::command(async)]
pub(crate) fn set_locale(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    code: String,
) -> Result<SettingsSnapshot, String> {
    validate_locale(&code)?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::LOCALE],
        move |settings| {
            settings.locale = code;
            Ok(())
        },
    )
}

/// Remember the treatment. A name the tokens do not bind is refused for the
/// same reason a language we do not ship is: the window would read it back at
/// boot and resolve it to nothing.
#[tauri::command(async)]
pub(crate) fn set_theme(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    code: String,
) -> Result<SettingsSnapshot, String> {
    validate_theme(&code)?;
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::THEME],
        move |settings| {
            settings.theme = code;
            Ok(())
        },
    )
}

/// Apply Orca's logarithmic UI zoom to this renderer only. Persistence is a
/// separate field patch below, so every app-owned webview can consume the
/// canonical snapshot through the same live path without a global window
/// registry or broad renderer webview permission.
#[tauri::command]
pub(crate) fn apply_ui_zoom(webview: tauri::Webview, zoom_level: f64) -> Result<(), String> {
    let _crumb = crate::crumbs::Command::enter("apply_ui_zoom");
    webview
        .set_zoom(zoom_level_to_scale(zoom_level))
        .map_err(|_| "UI 확대/축소를 적용하지 못했습니다".to_string())
}

/// Persist the UI zoom level. The renderer owns the optimistic/native apply;
/// a failed write is reconciled by the canonical snapshot returned through
/// `commitSetting`, just like the other Appearance controls.
#[tauri::command(async)]
pub(crate) fn set_ui_zoom(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    zoom_level: f64,
) -> Result<SettingsSnapshot, String> {
    let canonical = normalize_zoom_level(zoom_level);
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::UI_ZOOM_LEVEL],
        move |settings| {
            settings.ui_zoom_level = canonical;
            Ok(())
        },
    )
}

/// Remember the face used by the application chrome. Empty input resets to
/// Orca's Geist default; installed-font discovery remains a UI convenience,
/// so a family typed by hand follows the same field patch.
#[tauri::command(async)]
pub(crate) fn set_app_font_family(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    family: String,
) -> Result<SettingsSnapshot, String> {
    let family = normalized_app_font_family(&family);
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::APP_FONT_FAMILY],
        move |settings| {
            settings.app_font_family = family;
            Ok(())
        },
    )
}

/// Show or hide the product name in the titlebar. The setting remains
/// reachable from Appearance after the name itself is hidden.
#[tauri::command(async)]
pub(crate) fn set_show_titlebar_app_name(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SHOW_TITLEBAR_APP_NAME],
        move |settings| {
            settings.show_titlebar_app_name = visible;
            Ok(())
        },
    )
}

#[tauri::command(async)]
pub(crate) fn set_show_menu_bar_icon(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<SettingsSnapshot, String> {
    state.native_tray().apply_menu_bar_preference(
        &app,
        visible,
        || {
            commit_setting(
                &app,
                &webview,
                &state,
                &[setting_key::SHOW_MENU_BAR_ICON],
                move |settings| {
                    settings.show_menu_bar_icon = visible;
                    Ok(())
                },
            )
        },
        || {
            load_settings_for_boot(state.settings())
                .document
                .show_menu_bar_icon
        },
    )
}

/// Keep the process alive on Windows when the main close button is pressed.
/// The native controller updates first so the next close sees the new choice;
/// a rejected settings patch restores the latest canonical value.
#[tauri::command(async)]
pub(crate) fn set_minimize_to_tray_on_close(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<SettingsSnapshot, String> {
    state.native_tray().apply_minimize_preference(
        enabled,
        || {
            commit_setting(
                &app,
                &webview,
                &state,
                &[setting_key::MINIMIZE_TO_TRAY_ON_CLOSE],
                move |settings| {
                    settings.minimize_to_tray_on_close = enabled;
                    Ok(())
                },
            )
        },
        || {
            load_settings_for_boot(state.settings())
                .document
                .minimize_to_tray_on_close
        },
    )
}

/// Choose Orca's detailed or compact workspace-card layout. The stored bool
/// mirrors Orca's `compactWorktreeCards`; `false` is the detailed default.
#[tauri::command(async)]
pub(crate) fn set_compact_worktree_cards(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    compact: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::COMPACT_WORKTREE_CARDS],
        move |settings| {
            settings.compact_worktree_cards = compact;
            Ok(())
        },
    )
}

/// 카드의 에이전트(워커) 행을 요약할 것인가, 전부 세울 것인가 — 표시 메뉴의
/// 「에이전트 활동 레이아웃」이 부르는 문. 모르는 낱말은 접지 않고 거절한다:
/// 조용히 기본으로 접으면 사람이 고른 것과 창이 그리는 것이 어긋난 채
/// 재시작 뒤에야 드러난다(`SidebarView::parsed`와 같은 규칙).
#[tauri::command(async)]
pub(crate) fn set_agent_activity_display(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: String,
) -> Result<SettingsSnapshot, String> {
    let mode = match mode.as_str() {
        "compact" => AgentActivityDisplay::Compact,
        "full" => AgentActivityDisplay::Full,
        other => return Err(format!("unknown agent activity display: {other}")),
    };
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::AGENT_ACTIVITY_DISPLAY],
        move |settings| {
            settings.agent_activity_display = mode;
            Ok(())
        },
    )
}

/// Include or omit paths matched by `.gitignore` in the file explorer.
/// Detailed ignored-state decoration remains renderer-owned; this patch owns
/// only Orca's persisted visibility choice.
#[tauri::command(async)]
pub(crate) fn set_show_git_ignored_files(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SHOW_GIT_IGNORED_FILES],
        move |settings| {
            settings.show_git_ignored_files = visible;
            Ok(())
        },
    )
}

/// Reorder the live source-control sections without making conflicts part of
/// the preference: unresolved work stays pinned first in every preset.
#[tauri::command(async)]
pub(crate) fn set_source_control_group_order(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    order: SourceControlGroupOrder,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SOURCE_CONTROL_GROUP_ORDER],
        move |settings| {
            settings.source_control_group_order = order;
            Ok(())
        },
    )
}

/// Choose the implicit base used only by committed-change comparisons.
/// Worktree/repository pins, pull-request targets, and rebase targets remain
/// separate contracts; the resolver below applies this value only after both
/// compare-base pins are absent.
#[tauri::command(async)]
pub(crate) fn set_source_control_compare_base(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    mode: SourceControlCompareBase,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::SOURCE_CONTROL_COMPARE_BASE],
        move |settings| {
            settings.source_control_compare_base = mode;
            Ok(())
        },
    )
}

/// Keep the ordinary local branch which matches a remote workspace base from
/// falling behind. The create path still proves fast-forwardability and a
/// clean checked-out worktree immediately before it moves the ref; this bool
/// grants no destructive fallback.
#[tauri::command(async)]
pub(crate) fn set_refresh_local_base_ref_on_worktree_create(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::REFRESH_LOCAL_BASE_REF_ON_WORKTREE_CREATE],
        move |settings| {
            settings.refresh_local_base_ref_on_worktree_create = enabled;
            Ok(())
        },
    )
}

/// Apply one field of Orca's left-sidebar surface treatment. A tagged patch
/// keeps mode, tint colour, and tint strength independently mergeable across
/// windows while giving the three values one validation owner.
#[tauri::command(async)]
pub(crate) fn patch_left_sidebar_appearance(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: LeftSidebarAppearancePatch,
) -> Result<SettingsSnapshot, String> {
    let changed = patch.setting_key();
    commit_setting(&app, &webview, &state, &[changed], move |settings| {
        patch.apply(settings);
        Ok(())
    })
}

/// Show or hide one status-bar field that this build can actually render.
///
/// This is an item patch rather than a replacement list, so two windows
/// toggling different indicators cannot overwrite one another with stale
/// sibling state.
#[tauri::command(async)]
pub(crate) fn set_status_bar_item(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    item: String,
    enabled: bool,
) -> Result<SettingsSnapshot, String> {
    if !STATUS_BAR_ITEMS.contains(&item.as_str()) {
        return Err(format!("unknown status-bar item: {item}"));
    }
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::STATUS_BAR_ITEMS],
        move |settings| {
            if enabled {
                if !settings.status_bar_items.contains(&item) {
                    settings.status_bar_items.push(item);
                }
            } else {
                settings.status_bar_items.retain(|held| held != &item);
            }
            settings.status_bar_items =
                normalize_status_bar_items(std::mem::take(&mut settings.status_bar_items));
            Ok(())
        },
    )
}

/// How see-through the terminal screen is. Orca's `terminalBackgroundOpacity`.
///
/// Clamped rather than refused, for the reason [`WindowMaterial::clamped`]
/// gives: a slider's value is not the sort of thing worth failing on.
#[tauri::command(async)]
pub(crate) fn set_terminal_opacity(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    value: f64,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WINDOW_MATERIAL],
        move |settings| {
            settings.window_material.terminal_opacity = value;
            Ok(())
        },
    )
}

/// Whether the window is blurred glass. Orca's `windowBackgroundBlur`.
///
/// Stored only. The material is asked for once, while the window is being
/// built, so this takes effect on the next launch — which is exactly what
/// Orca's own switch says ("Requires restart") and why it ships a relaunch
/// button beside it.
#[tauri::command(async)]
pub(crate) fn set_window_blur(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    on: bool,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::WINDOW_MATERIAL],
        move |settings| {
            settings.window_material.blur = on;
            Ok(())
        },
    )
}

/// Close and reopen this process, so a material asked for at startup can be
/// asked for again. Orca's `window.api.app.relaunch()` behind the same banner.
///
/// The one restart road, and therefore the one place a prepared update is
/// swapped in (t-3191, design §2.3): the staged bundle beside the running
/// one takes its name by rename, right here and never earlier, so no running
/// binary is ever overwritten (deploy-overwrite-kills-running-binary). A
/// swap that fails leaves the running bundle as it was and restarts it.
#[tauri::command]
pub(crate) fn relaunch_window(app: AppHandle) {
    let _crumb = crate::crumbs::Command::enter("relaunch_window");
    // The ledger's goodbye first (t-3058): every seated worker sleeps with
    // its dispatch open, so the panes this restart takes settle nothing.
    crate::orchestration::window_exiting(crate::now_epoch_ms());
    if let Some(staged) = app
        .try_state::<cmd::update::UpdateState>()
        .and_then(|held| held.staged())
        && let Some(bundle) = std::env::current_exe()
            .ok()
            .and_then(|exe| update_runtime::app_bundle_of(&exe))
    {
        let _ = update_store::swap_in(&bundle, &staged);
    }
    let _ = crash::clean_exit(app.state::<AppState>().local_data_root());
    app.restart();
}
