//! Remote commands.

use crate::*;

#[tauri::command]
pub(crate) async fn ssh_hosts(
    state: State<'_, AppState>,
) -> Result<ssh_hosts::SshHostsReport, String> {
    let repository = Arc::clone(state.settings());
    let service = state.ssh_hosts().clone();
    run_remote_host_task(move || ssh_hosts_report(&repository, &service)).await
}

#[tauri::command]
pub(crate) async fn save_ssh_host(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    input: ssh_hosts::SshHostInput,
) -> Result<ssh_hosts::SshHostsReport, String> {
    let origin = webview.label().to_string();
    if let Some(id) = &input.id {
        app.state::<sftp_runtime::SftpService>()
            .disconnect_host(&app, id)
            .await;
    }
    let repository = Arc::clone(state.settings());
    let service = state.ssh_hosts().clone();
    let (snapshot, report) =
        run_remote_host_task(move || save_ssh_host_transaction(&repository, &service, input))
            .await?;
    emit_settings_changed(
        &app,
        &origin,
        snapshot.revision,
        &["sshHosts", "remoteWorkspaces"],
    );
    Ok(report)
}

#[tauri::command]
pub(crate) async fn remove_ssh_host(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    id: String,
) -> Result<ssh_hosts::SshHostsReport, String> {
    let origin = webview.label().to_string();
    app.state::<sftp_runtime::SftpService>()
        .disconnect_host(&app, &id)
        .await;
    let repository = Arc::clone(state.settings());
    let service = state.ssh_hosts().clone();
    let (snapshot, report) =
        run_remote_host_task(move || remove_ssh_host_transaction(&repository, &service, &id))
            .await?;
    emit_settings_changed(
        &app,
        &origin,
        snapshot.revision,
        &["sshHosts", "remoteWorkspaces"],
    );
    Ok(report)
}

#[tauri::command]
pub(crate) async fn ssh_targets(
    state: State<'_, AppState>,
) -> Result<Vec<ssh_store::SshTarget>, String> {
    let repository = Arc::clone(state.settings());
    run_remote_host_task(move || Ok(ssh_targets_list(&repository))).await
}

/// Insert or update by id. An empty id is a new target and mints one, which
/// keeps the form one shape rather than two commands the renderer has to
/// choose between.
#[tauri::command]
pub(crate) async fn ssh_save_target(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    target: ssh_store::SshTarget,
) -> Result<Vec<ssh_store::SshTarget>, String> {
    let origin = webview.label().to_string();
    let repository = Arc::clone(state.settings());
    let (snapshot, targets) =
        run_remote_host_task(move || save_ssh_target_transaction(&repository, target)).await?;
    emit_settings_changed(&app, &origin, snapshot.revision, &["sshTargets"]);
    Ok(targets)
}

#[tauri::command]
pub(crate) async fn ssh_remove_target(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<ssh_store::SshTarget>, String> {
    let origin = webview.label().to_string();
    let repository = Arc::clone(state.settings());
    let (snapshot, targets) =
        run_remote_host_task(move || remove_ssh_target_transaction(&repository, &id)).await?;
    emit_settings_changed(&app, &origin, snapshot.revision, &["sshTargets"]);
    Ok(targets)
}

/// The Test button's one question: does `ssh` reach this target today?
///
/// Reads the target fresh from the store rather than trusting a record the
/// renderer sends back — the card can only test what is actually saved.
#[tauri::command]
pub(crate) async fn ssh_probe_target(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let repository = Arc::clone(state.settings());
    run_remote_host_task(move || {
        let target = ssh_targets_list(&repository)
            .into_iter()
            .find(|target| target.id == id)
            .ok_or_else(|| "대상을 찾을 수 없습니다".to_string())?;
        ssh_probe::run_probe(&target)
    })
    .await
}

/// Every connection state this process is holding, for a window that just
/// woke up. After this one read, the event stream is the truth.
#[tauri::command]
pub(crate) async fn ssh_link_states(
    state: State<'_, AppState>,
) -> Result<Vec<ssh_link::LinkReport>, String> {
    Ok(state.ssh_links().values().cloned().collect())
}

/// Stand this target's connection up and say how it went. `connecting` is
/// broadcast before the dial and the outcome after it, so every card follows
/// the same story the caller gets as the reply.
#[tauri::command]
pub(crate) async fn ssh_link_connect(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<ssh_link::LinkReport, String> {
    let repository = Arc::clone(state.settings());
    let target = run_remote_host_task({
        let id = id.clone();
        move || {
            ssh_targets_list(&repository)
                .into_iter()
                .find(|target| target.id == id)
                .ok_or_else(|| "대상을 찾을 수 없습니다".to_string())
        }
    })
    .await?;
    let epoch = {
        // A dial already out — a master standing, a ladder climbing — is the
        // answer; a second dial on top would race the first one's outcome.
        // Lock order is epochs then links, everywhere they are held together.
        let mut epochs = state.ssh_link_epochs();
        let mut links = state.ssh_links();
        if let Some(report) = links.get(&id)
            && matches!(
                report.state,
                ssh_link::LinkState::Connecting
                    | ssh_link::LinkState::Connected
                    | ssh_link::LinkState::Reconnecting
            )
        {
            return Ok(report.clone());
        }
        let slot = epochs.entry(id.clone()).or_insert(0);
        *slot += 1;
        links.insert(
            id.clone(),
            ssh_link::LinkReport::new(&id, ssh_link::LinkState::Connecting),
        );
        *slot
    };
    emit_ssh_link(
        &app,
        &ssh_link::LinkReport::new(&id, ssh_link::LinkState::Connecting),
    );
    let watched = target.clone();
    let outcome = run_remote_host_task(move || Ok(ssh_link::open_link(&target))).await?;
    let report = match outcome {
        Ok(()) => ssh_link::LinkReport::new(&id, ssh_link::LinkState::Connected),
        Err(fall) => ssh_link::LinkReport {
            error: Some(fall.said),
            ..ssh_link::LinkReport::new(
                &id,
                if fall.auth {
                    ssh_link::LinkState::AuthFailed
                } else {
                    ssh_link::LinkState::Error
                },
            )
        },
    };
    if finish_link(&app, &id, epoch, report.clone())
        && report.state == ssh_link::LinkState::Connected
    {
        spawn_link_watcher(app.clone(), id.clone(), watched, epoch);
    }
    Ok(report)
}

/// Take this target's connection down. A target that has meanwhile been
/// removed still gets its state cleared — the point is the answer, not the
/// errand.
#[tauri::command]
pub(crate) async fn ssh_link_disconnect(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let epoch = {
        // The bump alone retires any watcher mid-climb: its next look at the
        // epoch is its last.
        let mut epochs = state.ssh_link_epochs();
        let slot = epochs.entry(id.clone()).or_insert(0);
        *slot += 1;
        *slot
    };
    let repository = Arc::clone(state.settings());
    let target = run_remote_host_task({
        let id = id.clone();
        move || {
            Ok(ssh_targets_list(&repository)
                .into_iter()
                .find(|target| target.id == id))
        }
    })
    .await?;
    if let Some(target) = target {
        run_remote_host_task(move || {
            ssh_link::close_link(&target);
            Ok(())
        })
        .await?;
    }
    finish_link(
        &app,
        &id,
        epoch,
        ssh_link::LinkReport::new(&id, ssh_link::LinkState::Disconnected),
    );
    Ok(())
}

/// A terminal ON the target, as one more tab: the tab's command is
/// `ssh -t` itself. With reuse on it rides the standing master — or stands
/// one for the tabs after it — and this PTY is deliberately the one road
/// without BatchMode, so ssh's own password or passphrase question reaches
/// the person exactly as ssh asks it.
#[tauri::command]
pub(crate) async fn ssh_open_remote_term(
    state: State<'_, AppState>,
    id: String,
    rows: u16,
    cols: u16,
) -> Result<TermId, String> {
    let repository = Arc::clone(state.settings());
    let target = run_remote_host_task({
        let id = id.clone();
        move || {
            ssh_targets_list(&repository)
                .into_iter()
                .find(|target| target.id == id)
                .ok_or_else(|| "대상을 찾을 수 없습니다".to_string())
        }
    })
    .await?;
    let term = state.take_term_id();
    let (pty, _) = spawn_shell(
        &state,
        term,
        rows,
        cols,
        ShellStartup::Remote(ssh_link::remote_term_args(&target)),
        None,
    )?;
    state.hold_terminal(term, pty);
    state.cadence().wake();
    Ok(term)
}

#[tauri::command]
pub(crate) async fn ssh_import_config(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    re_adopt: bool,
) -> Result<SshImportReport, String> {
    let origin = webview.label().to_string();
    let repository = Arc::clone(state.settings());
    let (snapshot, changed) =
        run_remote_host_task(move || import_ssh_config_transaction(&repository, re_adopt)).await?;
    // A sync that moved nothing is not news the other window has to repaint
    // for, and this one runs every time the pane opens.
    if changed > 0 {
        emit_settings_changed(&app, &origin, snapshot.revision, &["sshTargets"]);
    }
    Ok(SshImportReport { changed })
}

#[tauri::command]
pub(crate) async fn test_ssh_host(app: AppHandle, id: String) -> Result<(), String> {
    app.state::<sftp_runtime::SftpService>()
        .connect(&app, &id)
        .await?;
    Ok(())
}

/// The window's answer to `ssh:credential-request` — a value, or `null`
/// for a decline. The resolved broadcast happens HERE for answered
/// questions (Orca resolves in the submit handler too,
/// `ssh-passphrase.ts:52-64`); the ask road broadcasts only for the
/// questions it had to withdraw itself.
#[tauri::command]
pub(crate) async fn ssh_submit_credential(
    app: AppHandle,
    request_id: String,
    value: Option<String>,
) -> Result<(), String> {
    let submitted = app
        .state::<AppState>()
        .ssh_credentials()
        .submit(&request_id, value);
    if submitted {
        let _ = app.emit_to(
            MAIN_WINDOW_LABEL,
            "ssh:credential-resolved",
            ssh_prompt::CredentialResolved { request_id },
        );
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn probe_ssh_host(
    input: ssh_hosts::SshHostProbeInput,
) -> Result<ssh_hosts::SshHostKeyView, String> {
    let endpoint = input.endpoint().map_err(|error| error.to_string())?;
    let pin = zerocode_ssh::SshConnector::default()
        .probe_host_key(&endpoint)
        .await
        .map_err(|error| error.to_string())?;
    ssh_hosts::SshHostKeyView::from_pin(&pin).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn remote_workspaces(
    state: State<'_, AppState>,
) -> Result<remote_workspaces::RemoteWorkspacesReport, String> {
    let repository = Arc::clone(state.settings());
    run_remote_host_task(move || Ok(remote_workspaces_report(&repository))).await
}

#[tauri::command]
pub(crate) async fn probe_remote_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    input: remote_workspaces::RemoteWorkspaceProbeInput,
) -> Result<remote_workspaces::VerifiedRemoteRoot, String> {
    let (host_id, root) = input.prepare().map_err(|error| error.to_string())?;
    let repository = Arc::clone(state.settings());
    let service = state.ssh_hosts().clone();
    let (record, password) = run_remote_host_task(move || {
        let snapshot = load_settings_resilient(&repository);
        ssh_connection_material_for_id(&snapshot.document, &service, host_id)
    })
    .await?;
    let root = verify_remote_workspace_root(&app, record, password, root).await?;
    Ok(remote_workspaces::VerifiedRemoteRoot {
        root: root.to_string(),
    })
}

#[tauri::command]
pub(crate) async fn save_remote_workspace(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    input: remote_workspaces::RemoteWorkspaceInput,
) -> Result<remote_workspaces::RemoteWorkspacesReport, String> {
    let origin = webview.label().to_string();
    let repository = Arc::clone(state.settings());
    let lookup_repository = Arc::clone(&repository);
    let service = state.ssh_hosts().clone();
    let (prepared, record, password) = run_remote_host_task(move || {
        prepare_remote_workspace_save(&lookup_repository, &service, input)
    })
    .await?;
    let root =
        verify_remote_workspace_root(&app, record, password, prepared.entry.root().clone()).await?;
    let entry = prepared.entry.with_verified_root(root);
    let update = prepared.update;
    let (snapshot, report) =
        run_remote_host_task(move || persist_remote_workspace(&repository, entry, update)).await?;
    emit_settings_changed(&app, &origin, snapshot.revision, &["remoteWorkspaces"]);
    Ok(report)
}

#[tauri::command]
pub(crate) async fn remove_remote_workspace(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    id: String,
) -> Result<remote_workspaces::RemoteWorkspacesReport, String> {
    let origin = webview.label().to_string();
    let repository = Arc::clone(state.settings());
    let (snapshot, report) =
        run_remote_host_task(move || remove_remote_workspace_transaction(&repository, &id)).await?;
    emit_settings_changed(&app, &origin, snapshot.revision, &["remoteWorkspaces"]);
    Ok(report)
}

#[tauri::command]
pub(crate) async fn test_remote_workspace(app: AppHandle, id: String) -> Result<(), String> {
    let (repository, service) = {
        let state = app.state::<AppState>();
        (Arc::clone(state.settings()), state.ssh_hosts().clone())
    };
    let (entry, record, password) = run_remote_host_task(move || {
        remote_workspace_connection_material(&repository, &service, &id)
    })
    .await?;
    let expected = entry.root().clone();
    let actual = verify_remote_workspace_root(&app, record, password, expected.clone()).await?;
    if actual != expected {
        return Err("remote workspace root no longer resolves to its saved location".to_string());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn open_remote_workspace_terminal(
    app: AppHandle,
    id: String,
    rows: u16,
    cols: u16,
) -> Result<TermId, String> {
    let repository = Arc::clone(app.state::<AppState>().settings());
    let entry = run_remote_host_task(move || {
        let snapshot = load_settings_resilient(&repository);
        let id = remote_workspaces::parse_id(&id).map_err(|e| e.to_string())?;
        remote_workspaces::find_entry(&snapshot.document.remote_workspaces, id)
            .cloned()
            .map_err(|e| e.to_string())
    })
    .await?;
    let host_id = entry.host_id().to_string();
    let service = app.state::<sftp_runtime::SftpService>();
    let session = service.connect(&app, &host_id).await?;
    let fs = service.filesystem(&app, Some(&host_id)).await?;
    let actual = fs
        .list(entry.root().as_str())
        .await
        .map_err(|e| e.to_string())?;
    if actual.path != entry.root().as_str() {
        return Err("remote workspace root no longer resolves to its saved location".to_string());
    }
    hold_shared_ssh_terminal(&app, session.native()?, entry.root().clone(), rows, cols).await
}

#[tauri::command]
pub(crate) async fn remote_servers(
    state: State<'_, AppState>,
) -> Result<remote_servers::RemoteServersReport, String> {
    let repository = Arc::clone(state.settings());
    let service = state.remote_servers().clone();
    run_remote_host_task(move || Ok(remote_servers_report(&repository, &service))).await
}

#[tauri::command]
pub(crate) async fn save_remote_server(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    input: remote_servers::RemoteServerInput,
) -> Result<remote_servers::RemoteServersReport, String> {
    let origin = webview.label().to_string();
    let repository = Arc::clone(state.settings());
    let service = state.remote_servers().clone();
    let (snapshot, report) =
        run_remote_host_task(move || save_remote_server_transaction(&repository, &service, input))
            .await?;
    emit_settings_changed(&app, &origin, snapshot.revision, &["remoteServers"]);
    Ok(report)
}

#[tauri::command]
pub(crate) async fn remove_remote_server(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    id: String,
) -> Result<remote_servers::RemoteServersReport, String> {
    let origin = webview.label().to_string();
    let repository = Arc::clone(state.settings());
    let service = state.remote_servers().clone();
    let (snapshot, report) =
        run_remote_host_task(move || remove_remote_server_transaction(&repository, &service, &id))
            .await?;
    emit_settings_changed(&app, &origin, snapshot.revision, &["remoteServers"]);
    Ok(report)
}

#[tauri::command]
pub(crate) async fn test_remote_server(app: AppHandle, id: String) -> Result<(), String> {
    let (repository, service) = {
        let state = app.state::<AppState>();
        (Arc::clone(state.settings()), state.remote_servers().clone())
    };
    run_remote_host_task(move || {
        let (entry, token) = remote_server_connection_material(&repository, &service, &id)?;
        verify_remote_server(&entry, token.as_str())
    })
    .await
}

#[tauri::command]
pub(crate) async fn open_remote_server_session(
    app: AppHandle,
    id: String,
    rows: u16,
    cols: u16,
) -> Result<TermId, String> {
    let (repository, service) = {
        let state = app.state::<AppState>();
        (Arc::clone(state.settings()), state.remote_servers().clone())
    };
    let pty = run_remote_host_task(move || {
        let (entry, token) = remote_server_connection_material(&repository, &service, &id)?;
        let zo = ZoBinary::discover().ok_or_else(|| {
            "zo is not installed, so a remote session cannot be attached".to_string()
        })?;
        let supervisor =
            LaneSupervisor::new(zo, entry.endpoint().to_string(), Some(token.to_string()))
                .map_err(|error| error.to_string())?;
        supervisor
            .open_lane(Box::new(LocalPty), None, None, &[], rows, cols)
            .map_err(|error| error.to_string())
    })
    .await?;
    Ok(hold_terminal_handle(&app, pty))
}

/// Read the nine permissions on a blocking worker.
///
/// Each answer is a TCC round trip, so the whole refresh costs tens of
/// milliseconds; the page asks on open, on every window focus and after every
/// action, and a terminal streaming beside it must not feel any of that.
#[tauri::command]
pub(crate) async fn developer_permission_statuses()
-> Result<Vec<developer_permissions::PermissionState>, String> {
    tokio::task::spawn_blocking(developer_permissions::statuses)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn request_developer_permission(
    id: developer_permissions::PermissionId,
) -> Result<developer_permissions::PermissionRequestResult, String> {
    developer_permissions::request(id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn open_developer_permission_settings(
    id: developer_permissions::PermissionId,
) -> Result<(), String> {
    developer_permissions::open_settings(id).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn test_local_network_permission(
    host: String,
    port: u32,
) -> developer_permissions::LocalNetworkTestResult {
    developer_permissions::test_local_network(host, port).await
}

#[tauri::command]
pub(crate) async fn open_ssh_terminal(
    app: AppHandle,
    id: String,
    rows: u16,
    cols: u16,
) -> Result<TermId, String> {
    let session = app
        .state::<sftp_runtime::SftpService>()
        .connect(&app, &id)
        .await?;
    let root = zerocode_core::host::RemotePath::parse(session.home.as_deref().unwrap_or("/"))
        .map_err(|error| error.to_string())?;
    hold_shared_ssh_terminal(&app, session.native()?, root, rows, cols).await
}
