//! The file manager's native boundary. No file contents pass through a shell.
use crate::sftp_runtime::{ConnectionReport, JobReport, JobSpec, Location, SftpService};
use crate::sftp_sources::{self, Source};
use crate::*;
use zerocode_ssh::files::{Directory, Entry, TransferControl};

#[tauri::command]
pub(crate) async fn sftp_sources(
    webview: tauri::Webview,
    app: AppHandle,
) -> Result<Vec<Source>, String> {
    from_the_main_webview(&webview)?;
    let repository = Arc::clone(app.state::<AppState>().settings());
    run_remote_host_task(move || {
        let document = load_settings_resilient(&repository).document;
        let roots = document
            .remote_workspaces
            .iter()
            .map(|w| (w.host_id().to_string(), w.root().as_str().to_string()))
            .chain(
                document
                    .sftp_bookmarks
                    .iter()
                    .filter_map(|b| b.host_id.as_ref().map(|id| (id.clone(), b.path.clone()))),
            )
            .collect::<Vec<(String, String)>>();
        Ok(sftp_sources::fold(
            &document.ssh_hosts,
            &document.ssh_targets,
            roots.iter().map(|(id, path)| (id.as_str(), path.as_str())),
        ))
    })
    .await
}

#[tauri::command]
pub(crate) async fn sftp_bookmark(
    webview: tauri::Webview,
    app: AppHandle,
    location: Location,
    remove: bool,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    if location.host_id.is_none() {
        return Err("a remote host is required".to_string());
    }
    let fs = app
        .state::<SftpService>()
        .filesystem(&app, location.host_id.as_deref())
        .await?;
    let path = fs
        .list(&location.path)
        .await
        .map_err(|e| e.to_string())?
        .path;
    let repository = Arc::clone(app.state::<AppState>().settings());
    run_remote_host_task(move || {
        mutate_settings(&repository, |document| {
            document
                .sftp_bookmarks
                .retain(|b| b.host_id != location.host_id || b.path != path);
            if !remove {
                if document.sftp_bookmarks.len() >= 256 {
                    return Err("saved folder limit reached".to_string());
                }
                document.sftp_bookmarks.push(Location {
                    host_id: location.host_id.clone(),
                    path: path.clone(),
                });
            }
            Ok(())
        })
        .map(|_| ())
    })
    .await
}

#[tauri::command]
pub(crate) async fn sftp_connect(
    webview: tauri::Webview,
    app: AppHandle,
    host_id: String,
    reconnect: bool,
) -> Result<Vec<ConnectionReport>, String> {
    from_the_main_webview(&webview)?;
    let service = app.state::<SftpService>();
    if reconnect {
        service.disconnect_host(&app, &host_id).await;
    }
    service.connect(&app, &host_id).await?;
    Ok(service.connections().await)
}

#[tauri::command]
pub(crate) async fn sftp_disconnect(
    webview: tauri::Webview,
    app: AppHandle,
    host_id: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    app.state::<SftpService>()
        .disconnect_host(&app, &host_id)
        .await;
    Ok(())
}

#[tauri::command]
pub(crate) async fn sftp_connections(
    webview: tauri::Webview,
    app: AppHandle,
) -> Result<Vec<ConnectionReport>, String> {
    from_the_main_webview(&webview)?;
    Ok(app.state::<SftpService>().connections().await)
}

#[tauri::command]
pub(crate) async fn sftp_list(
    webview: tauri::Webview,
    app: AppHandle,
    location: Location,
) -> Result<Directory, String> {
    from_the_main_webview(&webview)?;
    let fs = app
        .state::<SftpService>()
        .filesystem(&app, location.host_id.as_deref())
        .await?;
    let path = if location.path.is_empty() && location.host_id.is_none() {
        dirs::home_dir()
            .ok_or("local home is unavailable")?
            .to_string_lossy()
            .into_owned()
    } else if location.path.is_empty() {
        ".".to_string()
    } else {
        location.path
    };
    fs.list(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_search(
    webview: tauri::Webview,
    app: AppHandle,
    location: Location,
    query: String,
) -> Result<Vec<Entry>, String> {
    from_the_main_webview(&webview)?;
    if query.trim().is_empty() {
        return Err("search text is required".to_string());
    }
    let fs = app
        .state::<SftpService>()
        .filesystem(&app, location.host_id.as_deref())
        .await?;
    let query = query.to_lowercase();
    let entries = fs
        .walk(&location.path, &TransferControl::default())
        .await
        .map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|e| e.name.to_lowercase().contains(&query))
        .collect())
}

#[tauri::command]
pub(crate) async fn sftp_mkdir(
    webview: tauri::Webview,
    app: AppHandle,
    location: Location,
    name: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let path = zerocode_ssh::files::child(&location.path, &name).map_err(|e| e.to_string())?;
    app.state::<SftpService>()
        .filesystem(&app, location.host_id.as_deref())
        .await?
        .mkdir(&path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_rename(
    webview: tauri::Webview,
    app: AppHandle,
    location: Location,
    name: String,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    let parent = std::path::Path::new(&location.path)
        .parent()
        .and_then(std::path::Path::to_str)
        .ok_or("invalid source path")?;
    let target = zerocode_ssh::files::child(parent, &name).map_err(|e| e.to_string())?;
    app.state::<SftpService>()
        .filesystem(&app, location.host_id.as_deref())
        .await?
        .rename(&location.path, &target)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_read_text(
    webview: tauri::Webview,
    app: AppHandle,
    location: Location,
) -> Result<String, String> {
    from_the_main_webview(&webview)?;
    app.state::<SftpService>()
        .filesystem(&app, location.host_id.as_deref())
        .await?
        .read_text(&location.path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_write_text(
    webview: tauri::Webview,
    app: AppHandle,
    location: Location,
    text: String,
    expected: Option<String>,
) -> Result<(), String> {
    from_the_main_webview(&webview)?;
    app.state::<SftpService>()
        .filesystem(&app, location.host_id.as_deref())
        .await?
        .write_text(&location.path, &text, expected.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn sftp_enqueue(
    webview: tauri::Webview,
    app: AppHandle,
    spec: JobSpec,
) -> Result<JobReport, String> {
    from_the_main_webview(&webview)?;
    let service = app.state::<SftpService>();
    service.restore(&app).await?;
    let report = service.enqueue(&app, spec)?;
    service.persist(&app).await?;
    Ok(report)
}

#[tauri::command]
pub(crate) async fn sftp_jobs(
    webview: tauri::Webview,
    app: AppHandle,
) -> Result<Vec<JobReport>, String> {
    from_the_main_webview(&webview)?;
    let service = app.state::<SftpService>();
    service.restore(&app).await?;
    Ok(service.reports())
}

#[tauri::command]
pub(crate) async fn sftp_job_control(
    webview: tauri::Webview,
    app: AppHandle,
    id: String,
    action: String,
) -> Result<Vec<JobReport>, String> {
    from_the_main_webview(&webview)?;
    let service = app.state::<SftpService>();
    service.control(&app, &id, &action)?;
    service.persist(&app).await?;
    Ok(service.reports())
}
