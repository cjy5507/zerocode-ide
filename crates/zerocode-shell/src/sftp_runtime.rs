//! Persistent SSH/SFTP channels and the native file job queue.
use crate::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use zerocode_ssh::{
    SshConnection,
    files::{self, Conflict, FileSystem, Progress, TransferControl},
};

const MAX_JOBS: usize = 1000;
// Serialize file mutations so overlapping transfers cannot overwrite each other.
const TRANSFER_CONCURRENCY: usize = 1;
pub(crate) const TARGET_PREFIX: &str = "target:";
const JOB_EVENT: &str = "sftp:job";
const CONNECTION_EVENT: &str = "sftp:connection";

pub(crate) struct Session {
    pub(crate) ssh: Option<Arc<SshConnection>>,
    process: Option<Arc<zerocode_ssh::ProcessSftp>>,
    pub(crate) files: Option<FileSystem>,
    pub(crate) home: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionReport {
    pub(crate) host_id: String,
    pub(crate) state: &'static str,
    pub(crate) home: Option<String>,
    pub(crate) error: Option<String>,
}

impl Session {
    pub(crate) fn native(&self) -> Result<Arc<SshConnection>, String> {
        self.ssh
            .clone()
            .ok_or_else(|| "this host uses the system SSH terminal".to_string())
    }
    fn is_closed(&self) -> bool {
        self.ssh.as_ref().map_or_else(
            || self.process.as_ref().is_none_or(|p| p.is_closed()),
            |ssh| ssh.is_closed(),
        )
    }
    fn report(&self, id: &str) -> ConnectionReport {
        ConnectionReport {
            host_id: id.to_string(),
            state: if self.is_closed() {
                "disconnected"
            } else if self.files.is_some() {
                "connected"
            } else {
                "error"
            },
            home: self.home.clone(),
            error: self.error.clone(),
        }
    }
}

type SessionSlot = Arc<AsyncMutex<Option<Arc<Session>>>>;

pub(crate) struct SftpService {
    sessions: Mutex<HashMap<String, SessionSlot>>,
    jobs: Mutex<Vec<Arc<Job>>>,
    permits: Arc<Semaphore>,
    saved: AsyncMutex<bool>,
    journal: AsyncMutex<()>,
}

impl Default for SftpService {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            jobs: Mutex::new(Vec::new()),
            permits: Arc::new(Semaphore::new(TRANSFER_CONCURRENCY)),
            saved: AsyncMutex::new(false),
            journal: AsyncMutex::new(()),
        }
    }
}

impl SftpService {
    pub(crate) async fn restore(&self, app: &AppHandle) -> Result<(), String> {
        let mut loaded = self.saved.lock().await;
        if *loaded {
            return Ok(());
        }
        let path = app
            .state::<AppState>()
            .local_data_root()
            .join("sftp-jobs.json");
        let reports: Vec<JobReport> = match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("could not read transfer history: {e}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.to_string()),
        };
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for mut report in reports.into_iter().take(MAX_JOBS) {
            if !matches!(report.state.as_str(), "completed" | "failed" | "cancelled") {
                report.state = "failed".to_string();
                report.error = Some("The window restarted during this job. Review the destination and retry or resume.".to_string());
            }
            jobs.push(Arc::new(Job {
                id: report.id.clone(),
                spec: report.spec.clone(),
                control: TransferControl::default(),
                report: Mutex::new(report),
            }));
        }
        *loaded = true;
        Ok(())
    }

    pub(crate) async fn persist(&self, app: &AppHandle) -> Result<(), String> {
        let _guard = self.journal.lock().await;
        let directory = app.state::<AppState>().local_data_root().to_path_buf();
        let data = serde_json::to_vec(&self.reports()).map_err(|e| e.to_string())?;
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|e| e.to_string())?;
        let stage = directory.join("sftp-jobs.json.tmp");
        tokio::fs::write(&stage, data)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::rename(stage, directory.join("sftp-jobs.json"))
            .await
            .map_err(|e| e.to_string())
    }
    fn slot(&self, id: &str) -> SessionSlot {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(None)))
            .clone()
    }

    pub(crate) async fn connect(&self, app: &AppHandle, id: &str) -> Result<Arc<Session>, String> {
        let slot = self.slot(id);
        let mut held = slot.lock().await;
        let live = held.as_ref().filter(|s| !s.is_closed()).cloned();
        if let Some(session) = live.as_ref().filter(|s| s.files.is_some()) {
            return Ok(Arc::clone(session));
        }
        // A session whose SFTP channel failed still holds its authenticated
        // transport. The retry reopens the channel on that transport instead
        // of dialing again — and instead of replaying the stored error, which
        // is what a cached transport-without-files used to do forever.
        let retained_ssh = live.as_ref().and_then(|s| s.ssh.clone());
        let _ = app.emit(
            CONNECTION_EVENT,
            ConnectionReport {
                host_id: id.to_string(),
                state: "connecting",
                home: None,
                error: None,
            },
        );
        if let Some(target_id) = id.strip_prefix(TARGET_PREFIX) {
            let repository = Arc::clone(app.state::<AppState>().settings());
            let target_id = target_id.to_string();
            let target = run_remote_host_task(move || {
                ssh_targets_list(&repository)
                    .into_iter()
                    .find(|target| target.id == target_id)
                    .ok_or_else(|| "SSH target was not found".to_string())
            })
            .await?;
            let process = crate::proc::quiet_tokio_command("ssh")
                .args(ssh_link::sftp_args(&target))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| e.to_string())?;
            let process = zerocode_ssh::ProcessSftp::open(process)
                .await
                .map_err(|e| e.to_string())?;
            let directory = process.files.list(".").await.map_err(|e| e.to_string())?;
            let session = Arc::new(Session {
                ssh: None,
                files: Some(process.files.clone()),
                home: Some(directory.path),
                error: None,
                process: Some(process),
            });
            *held = Some(session.clone());
            let _ = app.emit(CONNECTION_EVENT, session.report(id));
            return Ok(session);
        }
        let state = app.state::<AppState>();
        let ssh = match retained_ssh {
            Some(ssh) => ssh,
            None => {
                let repository = Arc::clone(state.settings());
                let service = state.ssh_hosts().clone();
                let key = id.to_string();
                let (record, password) = run_remote_host_task(move || {
                    ssh_connection_material(&repository, &service, &key)
                })
                .await?;
                match connect_with_prompts(app, record, password).await {
                    Ok(ssh) => Arc::new(ssh),
                    Err(error) => {
                        let _ = app.emit(
                            CONNECTION_EVENT,
                            ConnectionReport {
                                host_id: id.to_string(),
                                state: "error",
                                home: None,
                                error: Some(error.clone()),
                            },
                        );
                        return Err(error);
                    }
                }
            }
        };
        let opened = async {
            let sftp = ssh.open_sftp().await.map_err(|e| e.to_string())?;
            let fs = FileSystem::Remote(Arc::new(sftp));
            let directory = fs.list(".").await.map_err(|e| e.to_string())?;
            Ok::<_, String>((fs, directory.path))
        }
        .await;
        let (files, home, error) = match opened {
            Ok((fs, home)) => (Some(fs), Some(home), None),
            Err(error) => (None, None, Some(error)),
        };
        let session = Arc::new(Session {
            ssh: Some(ssh),
            files,
            home,
            error,
            process: None,
        });
        *held = Some(Arc::clone(&session));
        if let Some(home) = &session.home {
            // Mapping is a settings transaction, so two windows cannot mint
            // two workspace identities for the same canonical home.
            let repository = Arc::clone(state.settings());
            let key = id.to_string();
            let root = home.clone();
            match run_remote_host_task(move || {
                mutate_settings(&repository, |document| {
                    remote_workspaces::ensure_home(
                        &mut document.remote_workspaces,
                        &document.ssh_hosts,
                        &key,
                        &root,
                    )
                })
            })
            .await
            {
                Ok(snapshot) => emit_settings_changed(
                    app,
                    MAIN_WINDOW_LABEL,
                    snapshot.revision,
                    &["remoteWorkspaces"],
                ),
                Err(error) => {
                    let _ = app.emit(
                        CONNECTION_EVENT,
                        ConnectionReport {
                            error: Some(error),
                            ..session.report(id)
                        },
                    );
                }
            }
        }
        let _ = app.emit(CONNECTION_EVENT, session.report(id));
        Ok(session)
    }

    pub(crate) async fn filesystem(
        &self,
        app: &AppHandle,
        host: Option<&str>,
    ) -> Result<FileSystem, String> {
        match host {
            None => Ok(FileSystem::Local),
            Some(id) => {
                let session = self.connect(app, id).await?;
                session.files.clone().ok_or_else(|| {
                    session
                        .error
                        .clone()
                        .unwrap_or_else(|| "SFTP is unavailable".to_string())
                })
            }
        }
    }

    pub(crate) async fn disconnect_host(&self, app: &AppHandle, id: &str) {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for job in jobs {
            if job.spec.source.host_id.as_deref() == Some(id)
                || job
                    .spec
                    .destination
                    .as_ref()
                    .and_then(|d| d.host_id.as_deref())
                    == Some(id)
            {
                job.control.cancelled.store(true, Ordering::Relaxed);
            }
        }
        let slot = self.slot(id);
        if let Some(session) = slot.lock().await.take()
            && let Some(FileSystem::Remote(sftp)) = &session.files
        {
            let _ = sftp.close().await;
        }
        let _ = app.emit(
            CONNECTION_EVENT,
            ConnectionReport {
                host_id: id.to_string(),
                state: "disconnected",
                home: None,
                error: None,
            },
        );
    }

    pub(crate) async fn connections(&self) -> Vec<ConnectionReport> {
        let slots: Vec<_> = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(id, slot)| (id.clone(), slot.clone()))
            .collect();
        let mut reports = Vec::new();
        for (id, slot) in slots {
            if let Some(session) = slot.lock().await.as_ref() {
                reports.push(session.report(&id));
            }
        }
        reports
    }

    pub(crate) fn reports(&self) -> Vec<JobReport> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|job| job.report())
            .collect()
    }

    pub(crate) fn enqueue(&self, app: &AppHandle, spec: JobSpec) -> Result<JobReport, String> {
        if matches!(spec.operation, Operation::Copy | Operation::Move) && spec.destination.is_none()
        {
            return Err("transfer destination is required".to_string());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let report = JobReport {
            id: id.clone(),
            spec: spec.clone(),
            state: "queued".to_string(),
            progress: Progress::default(),
            error: None,
        };
        let control = TransferControl::default();
        control
            .bytes_per_second
            .store(spec.bytes_per_second, Ordering::Relaxed);
        let job = Arc::new(Job {
            id,
            spec,
            control,
            report: Mutex::new(report.clone()),
        });
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if jobs.len() >= MAX_JOBS {
            return Err("transfer queue is full; clear completed jobs".to_string());
        }
        jobs.push(job.clone());
        drop(jobs);
        let app = app.clone();
        let permits = Arc::clone(&self.permits);
        tauri::async_runtime::spawn(async move {
            let permit = permits.acquire_owned().await;
            let result = async {
                let _permit = permit.map_err(|_| "transfer queue stopped".to_string())?;
                job.control.check().await.map_err(|e| e.to_string())?;
                job.update(&app, "running", None, None);
                let service = app.state::<SftpService>();
                let source = service
                    .filesystem(&app, job.spec.source.host_id.as_deref())
                    .await?;
                let from = &job.spec.source.path;
                match job.spec.operation {
                    Operation::Copy | Operation::Move => {
                        let target = job.spec.destination.as_ref().ok_or("missing destination")?;
                        let destination =
                            service.filesystem(&app, target.host_id.as_deref()).await?;
                        files::transfer(
                            &source,
                            from,
                            &destination,
                            &target.path,
                            job.spec.conflict,
                            matches!(job.spec.operation, Operation::Move),
                            &job.control,
                            |progress| {
                                job.update(
                                    &app,
                                    if job.control.paused.load(Ordering::Relaxed) {
                                        "paused"
                                    } else {
                                        "running"
                                    },
                                    Some(progress),
                                    None,
                                )
                            },
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    }
                    Operation::Remove => source
                        .remove(from, &job.control)
                        .await
                        .map_err(|e| e.to_string())?,
                    Operation::Chmod => source
                        .chmod(
                            from,
                            job.spec.permissions.ok_or("permissions are required")?,
                            job.spec.recursive,
                            &job.control,
                        )
                        .await
                        .map_err(|e| e.to_string())?,
                }
                Ok::<(), String>(())
            }
            .await;
            match result {
                Ok(()) => job.update(&app, "completed", None, None),
                Err(error) => job.update(
                    &app,
                    if job.control.cancelled.load(Ordering::Relaxed) {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    None,
                    Some(error),
                ),
            }
            if let Err(error) = app.state::<SftpService>().persist(&app).await {
                job.update(
                    &app,
                    "failed",
                    None,
                    Some(format!("transfer history could not be saved: {error}")),
                );
            }
        });
        Ok(report)
    }

    pub(crate) fn control(&self, app: &AppHandle, id: &str, action: &str) -> Result<(), String> {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if action == "clear" {
            jobs.retain(|job| {
                !matches!(
                    job.report().state.as_str(),
                    "completed" | "failed" | "cancelled"
                )
            });
            return Ok(());
        }
        let job = jobs
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or("transfer not found")?;
        drop(jobs);
        let state = job.report().state;
        match action {
            "cancel" if matches!(state.as_str(), "queued" | "running" | "paused") => {
                job.control.cancelled.store(true, Ordering::Relaxed);
                job.update(app, "cancelling", None, None);
            }
            "pause" if matches!(state.as_str(), "queued" | "running") => {
                job.control.paused.store(true, Ordering::Relaxed);
                job.update(app, "paused", None, None);
            }
            "resume" if state == "paused" => {
                job.control.paused.store(false, Ordering::Relaxed);
                job.update(app, "queued", None, None);
            }
            "retry" if matches!(state.as_str(), "failed" | "cancelled") => {
                self.enqueue(app, job.spec.clone())?;
            }
            _ => return Err("invalid transfer action".to_string()),
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Location {
    pub(crate) host_id: Option<String>,
    pub(crate) path: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Operation {
    Copy,
    Move,
    Remove,
    Chmod,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct JobSpec {
    pub(crate) operation: Operation,
    pub(crate) source: Location,
    pub(crate) destination: Option<Location>,
    #[serde(default)]
    pub(crate) conflict: Conflict,
    pub(crate) permissions: Option<u32>,
    #[serde(default)]
    pub(crate) recursive: bool,
    #[serde(default)]
    pub(crate) bytes_per_second: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobReport {
    pub(crate) id: String,
    pub(crate) spec: JobSpec,
    pub(crate) state: String,
    pub(crate) progress: Progress,
    pub(crate) error: Option<String>,
}

struct Job {
    id: String,
    spec: JobSpec,
    control: TransferControl,
    report: Mutex<JobReport>,
}
impl Job {
    fn report(&self) -> JobReport {
        self.report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn update(
        &self,
        app: &AppHandle,
        state: &str,
        progress: Option<Progress>,
        error: Option<String>,
    ) {
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        report.state = state.to_string();
        if let Some(progress) = progress {
            report.progress = progress;
        }
        report.error = error;
        let _ = app.emit(JOB_EVENT, report.clone());
    }
}
